//! VARA command port handler (TCP port 8300).

use anyhow::Result;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use super::protocol::{VaraCommand, VaraResponse};
use crate::HostEvent;

/// Maximum length of a single command line, in bytes. Lines longer than this
/// cause the connection to be closed, bounding per-client memory use.
const MAX_LINE: usize = 4096;

/// VARA command port TCP listener.
pub struct VaraCommandPort {
    _port: u16,
}

impl VaraCommandPort {
    /// Create a new command port handler on the given TCP port.
    pub fn new(port: u16) -> Self {
        Self { _port: port }
    }

    /// Handle a single client connection on the command port.
    pub async fn handle_client(
        stream: TcpStream,
        client_id: u32,
        event_tx: mpsc::Sender<HostEvent>,
        mut response_rx: mpsc::Receiver<VaraResponse>,
    ) -> Result<()> {
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        // Send initial version
        let version_resp =
            VaraResponse::Version(concat!("Coppa ", env!("CARGO_PKG_VERSION")).to_string());
        writer.write_all(version_resp.format().as_bytes()).await?;

        // Notify connection
        let _ = event_tx.send(HostEvent::Connected { client_id }).await;

        let event_tx_clone = event_tx.clone();

        // Read commands — when this task ends, it drops read_done_tx,
        // which signals the write task to stop.
        let (read_done_tx, mut read_done_rx) = mpsc::channel::<()>(1);
        let read_task = tokio::spawn(async move {
            let mut line = Vec::new();
            loop {
                line.clear();
                match read_command(&mut reader, &mut line).await {
                    Ok(false) => break, // EOF with nothing buffered
                    Ok(true) => {
                        // Invalid UTF-8 closes the connection, as read_line's
                        // InvalidData error did before.
                        let Ok(text) = std::str::from_utf8(&line) else {
                            break;
                        };
                        let text = text.trim();
                        // A `\r\n` pair ends its line at the `\r`, leaving an
                        // empty line at the `\n`; drop empty (and
                        // whitespace-only) lines instead of dispatching them.
                        if text.is_empty() {
                            continue;
                        }
                        let cmd = VaraCommand::parse(text);
                        let event = match &cmd {
                            VaraCommand::Connect {
                                source,
                                destination,
                            } => HostEvent::ConnectRequest {
                                client_id,
                                source: source.clone(),
                                destination: destination.clone(),
                            },
                            VaraCommand::Disconnect => HostEvent::DisconnectRequest { client_id },
                            _ => HostEvent::VaraCommand {
                                client_id,
                                command: text.to_string(),
                            },
                        };
                        if event_tx_clone.send(event).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::InvalidData {
                            eprintln!("Command client {} sent over-long line, closing", client_id);
                        }
                        break;
                    }
                }
            }
            drop(read_done_tx); // signal write task
        });

        // Write responses — exits when response_rx closes OR read task signals done
        let write_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    resp = response_rx.recv() => {
                        match resp {
                            Some(response) => {
                                if writer.write_all(response.format().as_bytes()).await.is_err() {
                                    break;
                                }
                            }
                            None => break, // response channel closed
                        }
                    }
                    _ = read_done_rx.recv() => {
                        // Read task finished (client disconnected), stop writing
                        break;
                    }
                }
            }
        });

        let _ = tokio::try_join!(read_task, write_task);

        let _ = event_tx.send(HostEvent::Disconnected { client_id }).await;

        Ok(())
    }
}

/// Read one command line into `line`, without its terminator.
///
/// VARA clients (Pat among them) end each command with a bare `\r`, so `\r`,
/// `\n` and `\r\n` all end a line. Returns `Ok(false)` at EOF with nothing
/// buffered. A line longer than `MAX_LINE` bytes is an `InvalidData` error.
async fn read_command<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    line: &mut Vec<u8>,
) -> std::io::Result<bool> {
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(!line.is_empty());
        }
        let (chunk, used, done) = match available.iter().position(|&b| b == b'\r' || b == b'\n') {
            Some(i) => (&available[..i], i + 1, true),
            None => (available, available.len(), false),
        };
        if line.len() + chunk.len() > MAX_LINE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "command line too long",
            ));
        }
        line.extend_from_slice(chunk);
        reader.consume(used);
        if done {
            return Ok(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn greeting_reports_package_version() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let (event_tx, _event_rx) = mpsc::channel(4);
        let (_response_tx, response_rx) = mpsc::channel(4);
        let handler = tokio::spawn(VaraCommandPort::handle_client(
            server,
            1,
            event_tx,
            response_rx,
        ));
        let mut reader = BufReader::new(client);
        let mut greeting = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            reader.read_line(&mut greeting),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            greeting,
            format!("VERSION Coppa {}\r\n", env!("CARGO_PKG_VERSION"))
        );
        drop(reader);
        tokio::time::timeout(std::time::Duration::from_secs(5), handler)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[test]
    fn test_command_port_new() {
        let _port = VaraCommandPort::new(8300);
    }

    /// A command-port client under test: its socket, the event stream
    /// `handle_client` dispatches to, the response sender (kept alive for the
    /// whole test so the write task does not wind down early), and the spawned
    /// handler's join handle.
    struct CommandClient {
        stream: TcpStream,
        rx: mpsc::Receiver<HostEvent>,
        _response_tx: mpsc::Sender<VaraResponse>,
        handler: tokio::task::JoinHandle<Result<()>>,
    }

    /// Connect a client to a real `handle_client` over a loopback socket,
    /// read the greeting, and drain the `Connected` event.
    async fn connect() -> CommandClient {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();

        let (event_tx, mut rx) = mpsc::channel(16);
        let (response_tx, response_rx) = mpsc::channel(16);
        let handler = tokio::spawn(VaraCommandPort::handle_client(
            server,
            1,
            event_tx,
            response_rx,
        ));

        // The greeting is `\n`-terminated, so read_line drains it exactly.
        let mut greeting = String::new();
        BufReader::new(&mut stream)
            .read_line(&mut greeting)
            .await
            .unwrap();
        assert!(!greeting.is_empty(), "no greeting received");

        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for Connected")
            .expect("event channel closed");
        assert!(matches!(event, HostEvent::Connected { client_id: 1 }));

        CommandClient {
            stream,
            rx,
            _response_tx: response_tx,
            handler,
        }
    }

    /// Wait up to 5 s for the next dispatched `VaraCommand` and return its text.
    async fn next_command(rx: &mut mpsc::Receiver<HostEvent>) -> String {
        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for a command")
            .expect("event channel closed");
        match event {
            HostEvent::VaraCommand { command, .. } => command,
            other => panic!("expected VaraCommand, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn bare_cr_terminated_command_is_dispatched() {
        let mut c = connect().await;
        c.stream.write_all(b"VERSION\r").await.unwrap();
        // The socket stays open: the bare \r alone must terminate the command.
        assert_eq!(next_command(&mut c.rx).await, "VERSION");
    }

    #[tokio::test]
    async fn pat_startup_sequence_dispatches_each_command() {
        let mut c = connect().await;
        c.stream
            .write_all(b"PUBLIC ON\rCWID ON\rCOMPRESSION TEXT\rMYCALL W5AU\rLISTEN OFF\r")
            .await
            .unwrap();
        for expected in [
            "PUBLIC ON",
            "CWID ON",
            "COMPRESSION TEXT",
            "MYCALL W5AU",
            "LISTEN OFF",
        ] {
            assert_eq!(next_command(&mut c.rx).await, expected);
        }

        c.stream.write_all(b"CONNECT W5AU K7XYZ\r").await.unwrap();
        let event = tokio::time::timeout(Duration::from_secs(5), c.rx.recv())
            .await
            .expect("timed out waiting for ConnectRequest")
            .expect("event channel closed");
        match event {
            HostEvent::ConnectRequest {
                client_id: 1,
                source,
                destination,
            } => {
                assert_eq!(source, "W5AU");
                assert_eq!(destination, "K7XYZ");
            }
            other => panic!("expected ConnectRequest, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn lf_and_crlf_terminators_dispatch_once_each() {
        let mut c = connect().await;
        c.stream.write_all(b"VERSION\r").await.unwrap();
        assert_eq!(next_command(&mut c.rx).await, "VERSION");

        // The leading \n completes the \r\n pair across the two writes and is
        // dropped as an empty line, not dispatched.
        tokio::time::sleep(Duration::from_millis(50)).await;
        c.stream
            .write_all(b"\nMYCALL W5AU\nLISTEN ON\r\n")
            .await
            .unwrap();
        assert_eq!(next_command(&mut c.rx).await, "MYCALL W5AU");
        assert_eq!(next_command(&mut c.rx).await, "LISTEN ON");

        // The empty line the \r\n pair leaves at its \n must not dispatch.
        let extra = tokio::time::timeout(Duration::from_millis(200), c.rx.recv()).await;
        assert!(
            extra.is_err(),
            "unexpected extra event: {:?}",
            extra.ok().flatten()
        );
    }

    #[tokio::test]
    async fn over_long_line_closes_the_connection() {
        let mut c = connect().await;
        let buf = vec![b'A'; MAX_LINE + 1];
        c.stream.write_all(&buf).await.unwrap();

        let event = tokio::time::timeout(Duration::from_secs(5), c.rx.recv())
            .await
            .expect("timed out waiting for Disconnected")
            .expect("event channel closed");
        assert!(matches!(event, HostEvent::Disconnected { client_id: 1 }));
        let result = c.handler.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn read_command_handles_terminators_split_across_reads() {
        let mut reader = BufReader::with_capacity(1, &b"A\r\nB\rC\n"[..]);
        let mut line = Vec::new();

        for expected in ["A", "", "B", "C"] {
            line.clear();
            assert!(read_command(&mut reader, &mut line).await.unwrap());
            assert_eq!(line.as_slice(), expected.as_bytes());
        }
        line.clear();
        assert!(!read_command(&mut reader, &mut line).await.unwrap());
    }
}
