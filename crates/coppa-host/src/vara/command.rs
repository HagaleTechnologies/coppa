//! VARA command port handler (TCP port 8300).

use anyhow::Result;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

use super::protocol::{VaraCommand, VaraResponse};
use crate::HostEvent;

/// Maximum length of a single command line, in bytes. Lines longer than this
/// cause the connection to be closed, bounding per-client memory use.
const MAX_LINE: usize = 4096;

/// Lines each command-port client's response queue holds. A state line that
/// finds the queue full means the client has stopped reading, and the caller
/// disconnects it rather than skip the line.
pub const COMMAND_QUEUE_DEPTH: usize = 1024;

/// Queued lines past which [`queue_response`] drops `BUSY` and `SNR`
/// ([`VaraResponse::is_disposable`]), keeping the rest of the queue for state
/// lines and replies.
pub const TELEMETRY_QUEUE_DEPTH: usize = 64;

/// A command-port client's response channel, [`COMMAND_QUEUE_DEPTH`] lines
/// deep.
pub fn response_channel() -> (mpsc::Sender<VaraResponse>, mpsc::Receiver<VaraResponse>) {
    mpsc::channel(COMMAND_QUEUE_DEPTH)
}

/// What [`queue_response`] did with a line.
#[derive(Debug, PartialEq)]
pub enum Queued {
    /// The line is queued for the client.
    Sent,
    /// A disposable line (`BUSY`/`SNR`) was dropped: at least
    /// [`TELEMETRY_QUEUE_DEPTH`] lines were already waiting.
    Dropped,
    /// The queue is full and the line, returned here, is not disposable. The
    /// client has stopped reading; disconnect it by dropping its sender.
    Overflow(VaraResponse),
    /// The client's connection has already ended.
    Closed,
}

/// Queue `response` for one command-port client without waiting.
///
/// `BUSY` and `SNR` are dropped once [`TELEMETRY_QUEUE_DEPTH`] lines are
/// waiting. Every other line is queued in order, or returned as
/// [`Queued::Overflow`] when the queue is full, so a client never misses a
/// state line while its connection stays open.
pub fn queue_response(tx: &mpsc::Sender<VaraResponse>, response: VaraResponse) -> Queued {
    if response.is_disposable() && tx.max_capacity() - tx.capacity() >= TELEMETRY_QUEUE_DEPTH {
        return Queued::Dropped;
    }
    match tx.try_send(response) {
        Ok(()) => Queued::Sent,
        Err(TrySendError::Full(line)) if line.is_disposable() => Queued::Dropped,
        Err(TrySendError::Full(line)) => Queued::Overflow(line),
        Err(TrySendError::Closed(_)) => Queued::Closed,
    }
}

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
        let version_resp = VaraResponse::coppa_version();
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
                            tracing::warn!(
                                client_id,
                                "VARA command client sent an over-long line; closing"
                            );
                        }
                        break;
                    }
                }
            }
            drop(read_done_tx); // signal write task
        });
        let read_abort = read_task.abort_handle();

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
            // Writing has stopped, so stop reading too: the socket closes and
            // the daemon stops acting on commands whose replies go nowhere.
            read_abort.abort();
        });

        let _ = tokio::join!(read_task, write_task);

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
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn greeting_reports_package_version() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap())
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
        // Closing our write side ends the session, so the handler closes its
        // end and read_to_end returns every byte it sent: the greeting alone.
        client.shutdown().await.unwrap();
        let mut sent = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut sent))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            String::from_utf8(sent).unwrap(),
            format!("VERSION Coppa {}\r", env!("CARGO_PKG_VERSION"))
        );
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
        response_tx: mpsc::Sender<VaraResponse>,
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

        // The greeting is `\r`-terminated, so read_response drains it exactly.
        let greeting = read_response(&mut BufReader::new(&mut stream)).await;
        assert!(
            greeting.starts_with("VERSION "),
            "got greeting {:?}",
            greeting
        );

        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for Connected")
            .expect("event channel closed");
        assert!(matches!(event, HostEvent::Connected { client_id: 1 }));

        CommandClient {
            stream,
            rx,
            response_tx,
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

    /// Read one response, up to and including its `\r`, waiting up to 5 s.
    async fn read_response<R: AsyncBufRead + Unpin>(reader: &mut R) -> String {
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), reader.read_until(b'\r', &mut buf))
            .await
            .expect("timed out waiting for a response")
            .unwrap();
        String::from_utf8(buf).unwrap()
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
    async fn back_to_back_responses_each_end_in_a_bare_cr() {
        let mut c = connect().await;
        c.response_tx.send(VaraResponse::Busy(true)).await.unwrap();
        c.response_tx.send(VaraResponse::Buffer(0)).await.unwrap();
        // Pat splits on `\r`: a `\n` left after one response would arrive as
        // the start of the next one ("\nBUFFER 0").
        let mut reader = BufReader::new(&mut c.stream);
        assert_eq!(read_response(&mut reader).await, "BUSY ON\r");
        assert_eq!(read_response(&mut reader).await, "BUFFER 0\r");
    }

    #[test]
    fn only_busy_and_snr_are_disposable() {
        assert!(VaraResponse::Busy(true).is_disposable());
        assert!(VaraResponse::Busy(false).is_disposable());
        assert!(VaraResponse::Snr(-3).is_disposable());
        assert!(!VaraResponse::Ok.is_disposable());
        assert!(!VaraResponse::Wrong.is_disposable());
        assert!(!VaraResponse::coppa_version().is_disposable());
        assert!(!VaraResponse::Pending.is_disposable());
        assert!(!VaraResponse::CancelPending.is_disposable());
        assert!(!VaraResponse::Connected("W5AU".into(), "K7XYZ".into(), 2300).is_disposable());
        assert!(!VaraResponse::Disconnected.is_disposable());
        assert!(!VaraResponse::Ptt(true).is_disposable());
        assert!(!VaraResponse::Ptt(false).is_disposable());
        assert!(!VaraResponse::Buffer(0).is_disposable());
        assert!(!VaraResponse::IAmAlive.is_disposable());
    }

    /// A response channel with `TELEMETRY_QUEUE_DEPTH` `PTT` lines waiting.
    fn channel_at_telemetry_depth() -> (mpsc::Sender<VaraResponse>, mpsc::Receiver<VaraResponse>) {
        let (tx, rx) = response_channel();
        for i in 0..TELEMETRY_QUEUE_DEPTH {
            assert_eq!(
                queue_response(&tx, VaraResponse::Ptt(i % 2 == 0)),
                Queued::Sent
            );
        }
        (tx, rx)
    }

    #[test]
    fn queue_response_drops_only_busy_and_snr_past_telemetry_depth() {
        let (tx, _rx) = channel_at_telemetry_depth();
        assert_eq!(
            queue_response(&tx, VaraResponse::Busy(true)),
            Queued::Dropped
        );
        assert_eq!(queue_response(&tx, VaraResponse::Snr(12)), Queued::Dropped);
        for line in [
            VaraResponse::Ptt(false),
            VaraResponse::Connected("W5AU".into(), "K7XYZ".into(), 2300),
            VaraResponse::Disconnected,
            VaraResponse::Buffer(0),
            VaraResponse::IAmAlive,
            VaraResponse::Ok,
            VaraResponse::Wrong,
            VaraResponse::coppa_version(),
        ] {
            assert_eq!(
                queue_response(&tx, line.clone()),
                Queued::Sent,
                "{:?}",
                line
            );
        }
        assert_eq!(tx.max_capacity() - tx.capacity(), TELEMETRY_QUEUE_DEPTH + 8);
    }

    #[test]
    fn busy_and_snr_are_queued_below_telemetry_depth() {
        let (tx, mut rx) = response_channel();
        assert_eq!(queue_response(&tx, VaraResponse::Busy(true)), Queued::Sent);
        assert_eq!(queue_response(&tx, VaraResponse::Snr(7)), Queued::Sent);
        assert_eq!(rx.try_recv().unwrap(), VaraResponse::Busy(true));
        assert_eq!(rx.try_recv().unwrap(), VaraResponse::Snr(7));
    }

    #[test]
    fn queue_response_reports_overflow_when_a_state_line_does_not_fit() {
        let (tx, mut rx) = response_channel();
        for _ in 0..COMMAND_QUEUE_DEPTH {
            assert_eq!(queue_response(&tx, VaraResponse::Ptt(true)), Queued::Sent);
        }
        assert_eq!(
            queue_response(&tx, VaraResponse::Disconnected),
            Queued::Overflow(VaraResponse::Disconnected)
        );
        // The rejected line is not queued: the queue holds only what fitted.
        let mut queued = 0;
        while let Ok(line) = rx.try_recv() {
            assert_eq!(line, VaraResponse::Ptt(true));
            queued += 1;
        }
        assert_eq!(queued, COMMAND_QUEUE_DEPTH);

        drop(rx);
        assert_eq!(
            queue_response(&tx, VaraResponse::Disconnected),
            Queued::Closed
        );
    }

    #[tokio::test]
    async fn dropping_the_response_sender_closes_the_connection() {
        let c = connect().await;
        let CommandClient {
            stream,
            mut rx,
            response_tx,
            handler,
        } = c;
        assert_eq!(
            queue_response(&response_tx, VaraResponse::Ptt(true)),
            Queued::Sent
        );
        assert_eq!(
            queue_response(&response_tx, VaraResponse::Ptt(false)),
            Queued::Sent
        );
        drop(response_tx);

        // The client keeps its write half open: only the server can end this.
        let (read_half, _write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        assert_eq!(read_response(&mut reader).await, "PTT ON\r");
        assert_eq!(read_response(&mut reader).await, "PTT OFF\r");
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), reader.read_to_end(&mut rest))
            .await
            .expect("timed out waiting for EOF")
            .unwrap();
        assert!(
            rest.is_empty(),
            "unexpected bytes after the queue: {:?}",
            rest
        );

        tokio::time::timeout(Duration::from_secs(5), handler)
            .await
            .expect("handle_client did not return after its sender was dropped")
            .unwrap()
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for Disconnected")
            .expect("event channel closed");
        assert!(matches!(event, HostEvent::Disconnected { client_id: 1 }));
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
