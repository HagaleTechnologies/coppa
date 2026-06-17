//! KISS TNC protocol implementation over TCP.
//!
//! KISS (Keep It Simple, Stupid) is the standard protocol used by packet radio
//! applications (Pat, Xastir, YAAC) to communicate with TNCs. This module
//! implements KISS framing and a TCP server that accepts multiple client
//! connections simultaneously.
//!
//! Reference: <http://www.ax25.net/kiss.aspx>

use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, Semaphore};

// ---------------------------------------------------------------------------
// KISS framing constants
// ---------------------------------------------------------------------------

const FEND: u8 = 0xC0;
const FESC: u8 = 0xDB;
const TFEND: u8 = 0xDC;
const TFESC: u8 = 0xDD;
const CMD_DATA: u8 = 0x00;
const CMD_TX_DELAY: u8 = 0x01;
const CMD_PERSISTENCE: u8 = 0x02;
const CMD_SLOT_TIME: u8 = 0x03;
const CMD_RETURN: u8 = 0xFF;

/// Maximum number of concurrent KISS client connections.
const MAX_CONCURRENT_CONNECTIONS: usize = 16;

/// Maximum bytes buffered while waiting for a closing FEND. A client that never
/// sends a frame delimiter cannot grow this buffer without bound; the connection
/// is closed once the cap is exceeded.
const MAX_KISS_FRAME_LEN: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// KissFrame
// ---------------------------------------------------------------------------

/// A decoded KISS frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KissFrame {
    /// AX.25 data payload (port 0 CMD_DATA).
    Data(Vec<u8>),
    /// TX delay setting (in units of 10 ms).
    TxDelay(u8),
    /// Persistence parameter (P = value/256).
    Persistence(u8),
    /// Slot time (in units of 10 ms).
    SlotTime(u8),
    /// Return to KISS mode from host-mode (CMD_RETURN).
    Return,
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

/// Encode `data` as a KISS data frame.
///
/// The resulting bytes are:
/// `FEND CMD_DATA <escaped data> FEND`
///
/// Escape rules applied to the data payload:
/// - `FEND` (0xC0) → `FESC TFEND` (0xDB 0xDC)
/// - `FESC` (0xDB) → `FESC TFESC` (0xDB 0xDD)
pub fn kiss_encode_data(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 4);
    out.push(FEND);
    out.push(CMD_DATA);
    for &b in data {
        match b {
            FEND => {
                out.push(FESC);
                out.push(TFEND);
            }
            FESC => {
                out.push(FESC);
                out.push(TFESC);
            }
            _ => out.push(b),
        }
    }
    out.push(FEND);
    out
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// Decode raw bytes into zero or more [`KissFrame`]s.
///
/// Handles:
/// - Multiple frames in a single buffer
/// - Escape sequences within data payloads
/// - FEND-only separators (empty frames are skipped)
pub fn kiss_decode(data: &[u8]) -> Vec<KissFrame> {
    let mut frames = Vec::new();
    let mut i = 0;

    while i < data.len() {
        // Seek to the opening FEND
        if data[i] != FEND {
            i += 1;
            continue;
        }
        i += 1; // consume opening FEND

        // Collect frame bytes until the closing FEND (or end of buffer)
        let mut payload: Vec<u8> = Vec::new();
        let mut escape_next = false;

        while i < data.len() && data[i] != FEND {
            let b = data[i];
            i += 1;

            if escape_next {
                escape_next = false;
                match b {
                    TFEND => payload.push(FEND),
                    TFESC => payload.push(FESC),
                    // Malformed escape — pass through raw byte
                    _ => {
                        payload.push(FESC);
                        payload.push(b);
                    }
                }
            } else if b == FESC {
                escape_next = true;
            } else {
                payload.push(b);
            }
        }
        // `i` now points at the closing FEND (or past end of buffer)

        // Empty payload between two FENDs — skip
        if payload.is_empty() {
            continue;
        }

        // First byte is the command/port nibble
        let cmd = payload[0];
        let body = &payload[1..];

        let frame = match cmd {
            CMD_DATA => KissFrame::Data(body.to_vec()),
            CMD_TX_DELAY => {
                if let Some(&v) = body.first() {
                    KissFrame::TxDelay(v)
                } else {
                    KissFrame::TxDelay(0)
                }
            }
            CMD_PERSISTENCE => {
                if let Some(&v) = body.first() {
                    KissFrame::Persistence(v)
                } else {
                    KissFrame::Persistence(0)
                }
            }
            CMD_SLOT_TIME => {
                if let Some(&v) = body.first() {
                    KissFrame::SlotTime(v)
                } else {
                    KissFrame::SlotTime(0)
                }
            }
            CMD_RETURN => KissFrame::Return,
            // Unknown command — skip
            _ => continue,
        };

        frames.push(frame);
    }

    frames
}

// ---------------------------------------------------------------------------
// KissServer
// ---------------------------------------------------------------------------

/// TCP server that speaks the KISS protocol.
///
/// - Frames arriving from KISS clients are forwarded to the transmit pipeline
///   via [`KissServer::take_tx_receiver`].
/// - Frames received over the air (pushed via [`KissServer::rx_sender`]) are
///   broadcast to all connected KISS clients.
pub struct KissServer {
    /// Send AX.25 payloads from KISS clients to the TX pipeline.
    tx_sender: mpsc::Sender<Vec<u8>>,
    tx_receiver: Option<mpsc::Receiver<Vec<u8>>>,
    /// Broadcast AX.25 payloads from the air to all connected KISS clients.
    rx_broadcast: broadcast::Sender<Vec<u8>>,
    port: u16,
    bind_addr: String,
}

impl KissServer {
    /// Create a new `KissServer` that will bind to `port` on loopback (127.0.0.1).
    pub fn new(port: u16) -> Self {
        Self::with_bind_addr(port, "127.0.0.1".to_string())
    }

    /// Create a new `KissServer` binding `port` on the given address.
    pub fn with_bind_addr(port: u16, bind_addr: String) -> Self {
        let (tx_sender, tx_receiver) = mpsc::channel(256);
        let (rx_broadcast, _) = broadcast::channel(64);
        Self {
            tx_sender,
            tx_receiver: Some(tx_receiver),
            rx_broadcast,
            port,
            bind_addr,
        }
    }

    /// Take the TX receiver (may only be called once; returns `None` thereafter).
    ///
    /// The caller should consume payloads from this receiver and pass them to
    /// the radio TX pipeline.
    pub fn take_tx_receiver(&mut self) -> Option<mpsc::Receiver<Vec<u8>>> {
        self.tx_receiver.take()
    }

    /// Clone the broadcast sender used to push received frames to all clients.
    ///
    /// Send raw AX.25 payloads; the server will KISS-encode them before
    /// forwarding to each connected client.
    pub fn rx_sender(&self) -> broadcast::Sender<Vec<u8>> {
        self.rx_broadcast.clone()
    }

    /// Bind the TCP listener and begin accepting KISS clients.
    ///
    /// This future runs indefinitely.  Spawn it with `tokio::spawn`.
    pub async fn start(&self) -> anyhow::Result<()> {
        let addr = format!("{}:{}", self.bind_addr, self.port);
        let listener = TcpListener::bind(&addr).await?;
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));

        loop {
            let (stream, peer) = listener.accept().await?;
            // Cap concurrent connections; drop the client if at capacity.
            let permit = match semaphore.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    eprintln!("KISS connection limit reached, dropping client {}", peer);
                    drop(stream);
                    continue;
                }
            };
            let tx_sender = self.tx_sender.clone();
            let rx_broadcast = self.rx_broadcast.clone();

            tokio::spawn(async move {
                let _permit = permit;
                handle_kiss_client(stream, peer.to_string(), tx_sender, rx_broadcast).await;
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Per-client handler
// ---------------------------------------------------------------------------

async fn handle_kiss_client(
    mut stream: tokio::net::TcpStream,
    peer: String,
    tx_sender: mpsc::Sender<Vec<u8>>,
    rx_broadcast: broadcast::Sender<Vec<u8>>,
) {
    let mut rx_receiver = rx_broadcast.subscribe();
    let mut buf = [0u8; 4096];
    // Per-client accumulation buffer for partial KISS frames across TCP reads.
    let mut accum: Vec<u8> = Vec::new();

    loop {
        tokio::select! {
            // Air → client: received AX.25 payload → encode as KISS → write
            result = rx_receiver.recv() => {
                match result {
                    Ok(payload) => {
                        let encoded = kiss_encode_data(&payload);
                        if stream.write_all(&encoded).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
            // Client → TX pipeline: read raw bytes → decode KISS → forward Data payloads
            result = stream.read(&mut buf) => {
                match result {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        accum.extend_from_slice(&buf[..n]);
                        let frames = kiss_decode(&accum);
                        for frame in frames {
                            if let KissFrame::Data(payload) = frame {
                                if tx_sender.send(payload).await.is_err() {
                                    // TX pipeline has gone away — keep serving client
                                }
                            }
                        }
                        // Retain any bytes after the last FEND (incomplete trailing frame).
                        // If the buffer ends with FEND all frames are complete; clear it.
                        // Otherwise keep everything after the last FEND so it can be
                        // completed by the next read.
                        if let Some(last_fend) = accum.iter().rposition(|&b| b == FEND) {
                            accum.drain(..=last_fend);
                        }
                        // If there is no FEND at all the data is pre-frame garbage or a
                        // very large partial payload. Bound the buffer: if it grows past
                        // MAX_KISS_FRAME_LEN without a delimiter, the peer is misbehaving
                        // (or hostile) — close the connection rather than buffer unboundedly.
                        if accum.len() > MAX_KISS_FRAME_LEN {
                            eprintln!(
                                "KISS client {} exceeded max frame size ({} bytes), closing",
                                peer,
                                accum.len()
                            );
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }

    drop(peer); // suppress unused warning; could log here
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Encoding tests ---

    #[test]
    fn test_kiss_escape_no_special_bytes() {
        let data = b"Hello, AX.25!";
        let encoded = kiss_encode_data(data);
        // FEND + CMD_DATA + data bytes unchanged + FEND
        assert_eq!(encoded[0], FEND);
        assert_eq!(encoded[1], CMD_DATA);
        assert_eq!(&encoded[2..encoded.len() - 1], data);
        assert_eq!(*encoded.last().unwrap(), FEND);
    }

    #[test]
    fn test_kiss_escape_fend_in_data() {
        let data = &[0x01, FEND, 0x02];
        let encoded = kiss_encode_data(data);
        // payload portion (between CMD_DATA byte and closing FEND): 0x01 FESC TFEND 0x02
        let payload = &encoded[2..encoded.len() - 1];
        assert_eq!(payload, &[0x01, FESC, TFEND, 0x02]);
    }

    #[test]
    fn test_kiss_escape_fesc_in_data() {
        let data = &[0x01, FESC, 0x02];
        let encoded = kiss_encode_data(data);
        let payload = &encoded[2..encoded.len() - 1];
        assert_eq!(payload, &[0x01, FESC, TFESC, 0x02]);
    }

    // --- Decoding tests ---

    #[test]
    fn test_kiss_decode_data_frame() {
        let input = &[FEND, CMD_DATA, 0xAA, 0xBB, 0xCC, FEND];
        let frames = kiss_decode(input);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], KissFrame::Data(vec![0xAA, 0xBB, 0xCC]));
    }

    #[test]
    fn test_kiss_decode_escaped_bytes() {
        // Data contains a literal FEND and literal FESC, both escaped
        let input = &[FEND, CMD_DATA, FESC, TFEND, FESC, TFESC, FEND];
        let frames = kiss_decode(input);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], KissFrame::Data(vec![FEND, FESC]));
    }

    #[test]
    fn test_kiss_decode_tx_delay() {
        let input = &[FEND, CMD_TX_DELAY, 50, FEND];
        let frames = kiss_decode(input);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], KissFrame::TxDelay(50));
    }

    #[test]
    fn test_kiss_decode_multiple_frames() {
        let frame1 = [FEND, CMD_DATA, 0x01, 0x02, FEND];
        let frame2 = [FEND, CMD_DATA, 0x03, 0x04, FEND];
        let mut input = Vec::new();
        input.extend_from_slice(&frame1);
        input.extend_from_slice(&frame2);

        let frames = kiss_decode(&input);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], KissFrame::Data(vec![0x01, 0x02]));
        assert_eq!(frames[1], KissFrame::Data(vec![0x03, 0x04]));
    }

    #[test]
    fn test_kiss_roundtrip() {
        let original = b"VK2ABC>APRS:Hello World!";
        let encoded = kiss_encode_data(original);
        let frames = kiss_decode(&encoded);
        assert_eq!(frames.len(), 1);
        match &frames[0] {
            KissFrame::Data(payload) => assert_eq!(payload.as_slice(), original),
            other => panic!("Expected Data frame, got {:?}", other),
        }
    }

    // --- Async server integration test ---

    #[tokio::test]
    async fn test_kiss_server_client_roundtrip() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        // Bind on port 0 so the OS picks a free port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Build a KissServer using that same port (we re-bind below, so we
        // just need the channels — we'll drive accept ourselves)
        let (tx_sender, mut tx_receiver) = mpsc::channel::<Vec<u8>>(64);
        let (rx_broadcast, _) = broadcast::channel::<Vec<u8>>(64);
        let rx_broadcast_clone = rx_broadcast.clone();

        // Spawn the accept loop using our pre-bound listener
        tokio::spawn(async move {
            loop {
                let (stream, peer) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                let tx = tx_sender.clone();
                let rx = rx_broadcast_clone.clone();
                tokio::spawn(async move {
                    handle_kiss_client(stream, peer.to_string(), tx, rx).await;
                });
            }
        });

        // Connect a client
        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();

        // ---- Client → Server ----
        // Send a KISS data frame from the client
        let ax25_payload = b"VK2ABC>APRS:Test";
        let frame = kiss_encode_data(ax25_payload);
        client.write_all(&frame).await.unwrap();

        // Verify the server forwarded the AX.25 payload via tx_receiver
        let received = tokio::time::timeout(std::time::Duration::from_secs(2), tx_receiver.recv())
            .await
            .expect("timed out waiting for tx payload")
            .expect("channel closed");

        assert_eq!(received.as_slice(), ax25_payload);

        // ---- Server → Client (air RX) ----
        // Push a frame from "the air" via the broadcast channel
        let air_payload = b"W1AW>BEACON:Hello";
        rx_broadcast.send(air_payload.to_vec()).unwrap();

        // The client should receive it KISS-encoded
        let mut recv_buf = vec![0u8; 256];
        let n = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.read(&mut recv_buf),
        )
        .await
        .expect("timed out waiting for client receive")
        .expect("read error");

        let decoded = kiss_decode(&recv_buf[..n]);
        assert_eq!(decoded.len(), 1);
        match &decoded[0] {
            KissFrame::Data(payload) => assert_eq!(payload.as_slice(), air_payload),
            other => panic!("Expected Data, got {:?}", other),
        }
    }
}
