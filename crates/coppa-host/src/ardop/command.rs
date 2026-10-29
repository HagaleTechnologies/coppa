//! ARDOP command port handler.
//!
//! Line discipline mirrors the VARA command port (`\r`, `\n` and `\r\n` all
//! end a line, COP-25's tolerant-terminator precedent) with the two ARDOP
//! differences: no greeting on connect (the client speaks first), and the
//! echo/`WRONG` policy lives in the daemon, not here — the port forwards
//! every line except `ARQCALL`/`DISCONNECT`, which it turns into
//! `ConnectRequest`/`DisconnectRequest` the way the VARA port treats
//! `CONNECT`/`DISCONNECT`.

use anyhow::Result;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

use super::protocol::{ArdopCommand, ArdopResponse};
use crate::HostEvent;

/// Maximum length of a single command line, in bytes. Lines longer than this
/// cause the connection to be closed, bounding per-client memory use.
const MAX_LINE: usize = 4096;

/// Lines each command-port client's response queue holds. A state line that
/// finds the queue full means the client has stopped reading, and the caller
/// disconnects it rather than skip the line.
pub const COMMAND_QUEUE_DEPTH: usize = 1024;

/// Queued lines past which [`queue_response`] drops `BUSY`
/// ([`ArdopResponse::is_disposable`]), keeping the rest of the queue for
/// state lines and replies.
pub const TELEMETRY_QUEUE_DEPTH: usize = 64;

/// A command-port client's response channel, [`COMMAND_QUEUE_DEPTH`] lines
/// deep.
pub fn response_channel() -> (mpsc::Sender<ArdopResponse>, mpsc::Receiver<ArdopResponse>) {
    mpsc::channel(COMMAND_QUEUE_DEPTH)
}

/// What [`queue_response`] did with a line. Same policy as the VARA command
/// port's `Queued`: disposable telemetry may be dropped, every other line is
/// queued in order or the client is disconnected.
#[derive(Debug, PartialEq)]
pub enum Queued {
    /// The line is queued for the client.
    Sent,
    /// A disposable line (`BUSY`) was dropped: at least
    /// [`TELEMETRY_QUEUE_DEPTH`] lines were already waiting.
    Dropped,
    /// The queue is full and the line, returned here, is not disposable. The
    /// client has stopped reading; disconnect it by dropping its sender.
    Overflow(ArdopResponse),
    /// The client's connection has already ended.
    Closed,
}

/// Queue `response` for one command-port client without waiting.
///
/// `BUSY` is dropped once [`TELEMETRY_QUEUE_DEPTH`] lines are waiting. Every
/// other line is queued in order, or returned as [`Queued::Overflow`] when
/// the queue is full, so a client never misses a state line while its
/// connection stays open.
pub fn queue_response(tx: &mpsc::Sender<ArdopResponse>, response: ArdopResponse) -> Queued {
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

/// Handle a single client connection on the ARDOP command port.
///
/// ARDOP TNCs send no greeting, so the first byte on this connection is
/// always the client's.
pub async fn handle_client(
    stream: TcpStream,
    client_id: u32,
    event_tx: mpsc::Sender<HostEvent>,
    mut response_rx: mpsc::Receiver<ArdopResponse>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let _ = event_tx.send(HostEvent::Connected { client_id }).await;

    let event_tx_clone = event_tx.clone();

    // The connection's last `MYCALL`: `ARQCALL` carries no source callsign,
    // so the port fills `ConnectRequest`'s `source` from it (empty string
    // when none — the daemon fills the real local callsign from its own
    // state).
    let mut last_mycall = String::new();

    // Read commands — when this task ends, it drops read_done_tx, which
    // signals the write task to stop.
    let (read_done_tx, mut read_done_rx) = mpsc::channel::<()>(1);
    let read_task = tokio::spawn(async move {
        let mut line = Vec::new();
        loop {
            line.clear();
            match read_command(&mut reader, &mut line).await {
                Ok(false) => break, // EOF with nothing buffered
                Ok(true) => {
                    let Ok(text) = std::str::from_utf8(&line) else {
                        break;
                    };
                    let text = text.trim();
                    // A `\r\n` pair ends its line at the `\r`, leaving an
                    // empty line at the `\n`; drop empty lines instead of
                    // dispatching them.
                    if text.is_empty() {
                        continue;
                    }
                    let command = ArdopCommand::parse(text);
                    if let ArdopCommand::MyCall(call) = &command {
                        last_mycall = call.clone();
                    }
                    let event = match &command {
                        ArdopCommand::ArqCall { target, .. } => HostEvent::ConnectRequest {
                            client_id,
                            source: last_mycall.clone(),
                            destination: target.clone(),
                        },
                        ArdopCommand::Disconnect => HostEvent::DisconnectRequest { client_id },
                        _ => HostEvent::ArdopCommand {
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
                            "ARDOP command client sent an over-long line; closing"
                        );
                    }
                    break;
                }
            }
        }
        drop(read_done_tx); // signal write task
    });
    let read_abort = read_task.abort_handle();

    // Write responses — exits when response_rx closes OR read task signals
    // done.
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

/// Read one command line into `line`, without its terminator.
///
/// ARDOP clients end each command with a bare `\r`, so `\r`, `\n` and
/// `\r\n` all end a line (the COP-25 tolerant set). Returns `Ok(false)` at
/// EOF with nothing buffered. A line longer than `MAX_LINE` bytes is an
/// `InvalidData` error.
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// ARDOP TNCs send no greeting: a client that connects and waits hears
    /// nothing, and the first byte on the wire is its own. This is the
    /// anti-test for the probe's first observation against the VARA port.
    #[tokio::test]
    async fn no_greeting_on_command_connect() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let (event_tx, _event_rx) = mpsc::channel(4);
        let (_response_tx, response_rx) = mpsc::channel(4);
        let handler = tokio::spawn(handle_client(server, 1, event_tx, response_rx));
        let mut probe = [0; 1];
        assert!(
            tokio::time::timeout(Duration::from_millis(300), client.read(&mut probe))
                .await
                .is_err(),
            "ARDOP must stay silent until the host speaks"
        );
        // After the quiet window, close and verify that no bytes were sent.
        client.shutdown().await.unwrap();
        let mut sent = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut sent))
            .await
            .unwrap()
            .unwrap();
        assert!(sent.is_empty(), "expected no greeting, got {sent:?}");
        tokio::time::timeout(Duration::from_secs(5), handler)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    /// An ARDOP command-port client under test: its socket, the event
    /// stream `handle_client` dispatches to, the response sender (kept
    /// alive for the whole test so the write task does not wind down
    /// early), and the spawned handler's join handle.
    struct ArdopClient {
        stream: TcpStream,
        rx: mpsc::Receiver<HostEvent>,
        _response_tx: mpsc::Sender<ArdopResponse>,
        handler: tokio::task::JoinHandle<Result<()>>,
    }

    /// Connect a client to a real `handle_client` over a loopback socket
    /// and drain the `Connected` event. No greeting is read because there
    /// is none.
    async fn connect() -> ArdopClient {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();

        let (event_tx, mut rx) = mpsc::channel(16);
        let (response_tx, response_rx) = mpsc::channel(16);
        let handler = tokio::spawn(handle_client(server, 1, event_tx, response_rx));

        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for Connected")
            .expect("event channel closed");
        assert!(matches!(event, HostEvent::Connected { client_id: 1 }));

        // No greeting: nothing arrives before the client's first line. A
        // short quiet window proves the socket is not about to speak.
        let mut probe = [0u8; 1];
        use tokio::io::AsyncReadExt;
        assert!(
            tokio::time::timeout(Duration::from_millis(200), stream.read(&mut probe))
                .await
                .is_err(),
            "the server must send no greeting"
        );

        ArdopClient {
            stream,
            rx,
            _response_tx: response_tx,
            handler,
        }
    }

    /// Wait up to 5 s for the next event and return it.
    async fn next_event(rx: &mut mpsc::Receiver<HostEvent>) -> HostEvent {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for an event")
            .expect("event channel closed")
    }

    /// Wait up to 5 s for the next dispatched `ArdopCommand` and return its
    /// text.
    async fn next_command(rx: &mut mpsc::Receiver<HostEvent>) -> String {
        match next_event(rx).await {
            HostEvent::ArdopCommand { command, .. } => command,
            other => panic!("expected ArdopCommand, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn command_line_is_tolerant_of_lf_and_crlf() {
        let mut c = connect().await;
        c.stream.write_all(b"MYCALL N1CALL\n").await.unwrap();
        assert_eq!(next_command(&mut c.rx).await, "MYCALL N1CALL");
        c.stream.write_all(b"STATE\r\n").await.unwrap();
        assert_eq!(next_command(&mut c.rx).await, "STATE");
        // The empty line the \r\n pair leaves at its \n must not dispatch.
        let extra = tokio::time::timeout(Duration::from_millis(200), c.rx.recv()).await;
        assert!(extra.is_err(), "unexpected extra event: {extra:?}");
    }

    /// Every line except `ARQCALL`/`DISCONNECT` becomes an `ArdopCommand`
    /// event; `ARQCALL` becomes a `ConnectRequest` whose `source` is the
    /// connection's last `MYCALL` (empty when none), and `DISCONNECT`
    /// becomes a `DisconnectRequest`.
    #[tokio::test]
    async fn every_line_becomes_an_ardop_command_event() {
        let mut c = connect().await;
        c.stream.write_all(b"STATE\r").await.unwrap();
        assert_eq!(next_command(&mut c.rx).await, "STATE");

        c.stream.write_all(b"MYCALL N1CALL\r").await.unwrap();
        assert_eq!(next_command(&mut c.rx).await, "MYCALL N1CALL");

        c.stream
            .write_all(b"ARQCALL N0CALL 3\rDISCONNECT\r")
            .await
            .unwrap();
        match next_event(&mut c.rx).await {
            HostEvent::ConnectRequest {
                client_id: 1,
                source,
                destination,
            } => {
                assert_eq!(source, "N1CALL");
                assert_eq!(destination, "N0CALL");
            }
            other => panic!("expected ConnectRequest, got {other:?}"),
        }
        assert!(matches!(
            next_event(&mut c.rx).await,
            HostEvent::DisconnectRequest { client_id: 1 }
        ));

        // With no MYCALL on the connection, the source is the empty string.
        let mut fresh = connect().await;
        fresh.stream.write_all(b"ARQCALL N0CALL 3\r").await.unwrap();
        match next_event(&mut fresh.rx).await {
            HostEvent::ConnectRequest { source, .. } => assert_eq!(source, ""),
            other => panic!("expected ConnectRequest, got {other:?}"),
        }
    }

    /// Queue semantics match the VARA command port: `BUSY` drops past the
    /// telemetry depth, a state line that does not fit overflows (the
    /// client is disconnected, never shown a gap), and a closed channel
    /// reports `Closed`.
    #[test]
    fn slow_command_client_is_evicted_like_vara() {
        // BUSY drops past the telemetry depth; state lines do not.
        let (tx, rx) = response_channel();
        for i in 0..TELEMETRY_QUEUE_DEPTH {
            assert_eq!(
                queue_response(&tx, ArdopResponse::Ptt(i % 2 == 0)),
                Queued::Sent
            );
        }
        assert_eq!(
            queue_response(&tx, ArdopResponse::Busy(true)),
            Queued::Dropped
        );
        for line in [
            ArdopResponse::NewState("DISCONNECTED".into()),
            ArdopResponse::Disconnected,
            ArdopResponse::Fault("busy".into()),
            ArdopResponse::Wrong,
        ] {
            assert_eq!(queue_response(&tx, line.clone()), Queued::Sent, "{line:?}");
        }
        drop(rx);

        // Fill to the hard depth with non-disposable lines, then prove the
        // next state line overflows rather than being skipped.
        let (tx2, mut rx2) = response_channel();
        for _ in 0..COMMAND_QUEUE_DEPTH {
            assert_eq!(queue_response(&tx2, ArdopResponse::Ptt(true)), Queued::Sent);
        }
        assert_eq!(
            queue_response(&tx2, ArdopResponse::Disconnected),
            Queued::Overflow(ArdopResponse::Disconnected)
        );
        let mut queued = 0;
        while let Ok(line) = rx2.try_recv() {
            assert_eq!(line, ArdopResponse::Ptt(true));
            queued += 1;
        }
        assert_eq!(queued, COMMAND_QUEUE_DEPTH);
        drop(rx2);
        assert_eq!(
            queue_response(&tx2, ArdopResponse::Disconnected),
            Queued::Closed,
            "a closed channel reports Closed"
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
}
