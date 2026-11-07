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
/// Only frames closed by FEND are returned. This function is stateless: an
/// incomplete trailing frame is ignored, so callers must retain it across reads.
///
/// Handles:
/// - Multiple frames in a single buffer
/// - Escape sequences within data payloads
/// - FEND-only separators (empty frames are skipped)
///
/// Stream readers should use [`KissDecoder`] to retain incomplete frames.
pub fn kiss_decode(data: &[u8]) -> Vec<KissFrame> {
    decode_closed_frames(data).0
}

/// Return complete frames and the prefix to discard, preserving the last FEND
/// as a possible opening delimiter for the next frame.
fn decode_closed_frames(data: &[u8]) -> (Vec<KissFrame>, usize) {
    let mut frames = Vec::new();
    let mut consumed = 0;
    let mut i = 0;

    while i < data.len() {
        // Seek to the opening FEND
        if data[i] != FEND {
            i += 1;
            continue;
        }
        consumed = i;
        let start = i + 1;
        let Some(end) = data[start..].iter().position(|&b| b == FEND) else {
            break;
        };
        let end = start + end;
        i = start;

        // Decode only after locating a closing raw delimiter.
        let mut payload: Vec<u8> = Vec::new();
        let mut escape_next = false;

        while i < end {
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
        // `i` now points at the closing FEND, also the next opening FEND.

        // Empty payload between two FENDs — skip
        if payload.is_empty() {
            continue;
        }

        if let Some(frame) = frame_from_payload(&payload) {
            frames.push(frame);
        }
    }

    (frames, consumed)
}

fn frame_from_payload(payload: &[u8]) -> Option<KissFrame> {
    let (&cmd, body) = payload.split_first()?;
    Some(match cmd {
        CMD_DATA => KissFrame::Data(body.to_vec()),
        CMD_TX_DELAY => KissFrame::TxDelay(body.first().copied().unwrap_or(0)),
        CMD_PERSISTENCE => KissFrame::Persistence(body.first().copied().unwrap_or(0)),
        CMD_SLOT_TIME => KissFrame::SlotTime(body.first().copied().unwrap_or(0)),
        CMD_RETURN => KissFrame::Return,
        _ => return None,
    })
}

/// Streaming KISS decoder that retains partial frames and escapes across reads.
///
/// Frames larger than 64 KiB stop decoding. Their pending length remains above
/// the limit so the TCP server can close the connection without buffering more.
#[derive(Debug, Default)]
pub struct KissDecoder {
    inside_frame: bool,
    escape_next: bool,
    payload: Vec<u8>,
    oversized: bool,
}

impl KissDecoder {
    /// Create an empty decoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode complete, delimiter-terminated frames from the next stream bytes.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<KissFrame> {
        let mut frames = Vec::new();
        for &b in bytes {
            if self.oversized {
                break;
            }
            if b == FEND {
                if let Some(frame) = frame_from_payload(&self.payload) {
                    frames.push(frame);
                }
                self.payload.clear();
                self.escape_next = false;
                self.inside_frame = true;
            } else if self.inside_frame {
                if self.escape_next {
                    self.escape_next = false;
                    match b {
                        TFEND => self.payload.push(FEND),
                        TFESC => self.payload.push(FESC),
                        _ => {
                            self.payload.push(FESC);
                            self.payload.push(b);
                        }
                    }
                } else if b == FESC {
                    self.escape_next = true;
                } else {
                    self.payload.push(b);
                }
                self.oversized = self.pending_len() > MAX_KISS_FRAME_LEN;
            }
        }
        frames
    }

    /// Bytes retained for the incomplete frame, including a pending escape.
    pub fn pending_len(&self) -> usize {
        self.payload.len() + usize::from(self.escape_next)
    }
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
    pub async fn start(&self) -> Result<(), crate::HostError> {
        let addr = format!("{}:{}", self.bind_addr, self.port);
        let listener = TcpListener::bind(&addr).await?;
        self.run_with_listener(listener).await
    }

    /// Accept KISS clients using a listener already bound by the caller.
    pub async fn run_with_listener(&self, listener: TcpListener) -> Result<(), crate::HostError> {
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));

        let mut clients = tokio::task::JoinSet::new();
        loop {
            let accepted = tokio::select! {
                result = listener.accept() => result,
                _ = clients.join_next(), if !clients.is_empty() => continue,
            };
            let (stream, peer) = accepted?;
            // Cap concurrent connections; drop the client if at capacity.
            let permit = match semaphore.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    tracing::warn!(%peer, "KISS connection limit reached, dropping client");
                    drop(stream);
                    continue;
                }
            };
            let tx_sender = self.tx_sender.clone();
            let rx_broadcast = self.rx_broadcast.clone();

            clients.spawn(async move {
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
    let mut decoder = KissDecoder::new();

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
                        let frames = decoder.push(&buf[..n]);
                        for frame in frames {
                            if let KissFrame::Data(payload) = frame {
                                if tx_sender.send(payload).await.is_err() {
                                    // TX pipeline has gone away — keep serving client
                                }
                            }
                        }
                        if decoder.pending_len() > MAX_KISS_FRAME_LEN {
                            tracing::warn!(
                                %peer,
                                len = decoder.pending_len(),
                                "KISS client exceeded max frame size, closing"
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

    #[test]
    fn test_kiss_decode_requires_closing_delimiter() {
        let wire = kiss_encode_data(&[0x41; 200]);
        for split in 1..wire.len() {
            assert!(kiss_decode(&wire[..split]).is_empty(), "split {split}");
        }
        let mut wire = kiss_encode_data(b"complete");
        wire.extend_from_slice(&[FEND, CMD_DATA, 0x41, FESC]);
        assert_eq!(
            kiss_decode(&wire),
            vec![KissFrame::Data(b"complete".to_vec())]
        );
    }

    #[test]
    fn test_kiss_decoder_every_split_and_byte_at_a_time() {
        for payload in [vec![0x41; 200], vec![0x41, FEND, FESC, 0x42]] {
            let wire = kiss_encode_data(&payload);
            // Includes 120 and 121 wire-byte prefixes and both split escapes.
            for split in 0..wire.len() {
                let mut decoder = KissDecoder::new();
                assert!(decoder.push(&wire[..split]).is_empty(), "split {split}");
                assert_eq!(
                    decoder.push(&wire[split..]),
                    vec![KissFrame::Data(payload.clone())],
                    "split {split}"
                );
                // The closing delimiter never replays the completed frame.
                assert_eq!(decoder.pending_len(), 0, "split {split}");
                assert!(decoder.push(&[]).is_empty());
            }
            let mut decoder = KissDecoder::new();
            for (i, byte) in wire.iter().enumerate() {
                let frames = decoder.push(&[*byte]);
                if i + 1 == wire.len() {
                    assert_eq!(frames, vec![KissFrame::Data(payload.clone())]);
                } else {
                    assert!(frames.is_empty());
                }
            }
        }
    }

    fn special_payload() -> Vec<u8> {
        (0..500)
            .map(|i| [FEND, FESC, TFEND, TFESC, 0][i % 5])
            .collect()
    }

    #[test]
    fn kiss_decoder_reassembles_a_frame_split_at_every_offset() {
        let payload = special_payload();
        let wire = kiss_encode_data(&payload);
        for split in 1..wire.len() {
            let mut decoder = KissDecoder::new();
            assert!(decoder.push(&wire[..split]).is_empty());
            assert_eq!(
                decoder.push(&wire[split..]),
                vec![KissFrame::Data(payload.clone())]
            );
        }
    }

    #[test]
    fn test_kiss_decoder_shared_delimiters_and_partial_tail() {
        let mut decoder = KissDecoder::new();
        let first = [
            0x99, FEND, FEND, CMD_DATA, 1, FEND, CMD_DATA, 2, FEND, FEND, CMD_DATA, FESC,
        ];
        assert_eq!(
            decoder.push(&first),
            vec![KissFrame::Data(vec![1]), KissFrame::Data(vec![2])]
        );
        assert_eq!(decoder.pending_len(), 2);
        assert_eq!(
            decoder.push(&[TFEND, FEND]),
            vec![KissFrame::Data(vec![FEND])]
        );
        // The closing delimiter from the previous read opens this empty data frame.
        assert_eq!(
            decoder.push(&[CMD_DATA, FEND, FEND]),
            vec![KissFrame::Data(vec![])]
        );
        assert!(decoder.push(&[FEND]).is_empty());
    }

    #[test]
    fn test_kiss_closed_command_and_malformed_escape_behavior() {
        assert_eq!(
            kiss_decode(&[
                FEND,
                CMD_PERSISTENCE,
                7,
                FEND,
                CMD_SLOT_TIME,
                FEND,
                CMD_RETURN,
                FEND,
                0x80,
                1,
                FEND,
                CMD_TX_DELAY,
                FEND
            ]),
            vec![
                KissFrame::Persistence(7),
                KissFrame::SlotTime(0),
                KissFrame::Return,
                KissFrame::TxDelay(0)
            ]
        );
        assert_eq!(
            kiss_decode(&[FEND, CMD_DATA, FESC, 0x12, FESC, FEND]),
            vec![KissFrame::Data(vec![FESC, 0x12])]
        );
    }

    #[test]
    fn kiss_decoder_returns_two_frames_from_one_push() {
        let wire = [kiss_encode_data(b"one"), kiss_encode_data(b"two")].concat();
        assert_eq!(
            KissDecoder::new().push(&wire),
            vec![
                KissFrame::Data(b"one".to_vec()),
                KissFrame::Data(b"two".to_vec())
            ]
        );
    }

    #[test]
    fn kiss_decoder_skips_bytes_before_the_first_fend() {
        assert_eq!(
            KissDecoder::new().push(&[1, 2, FEND, CMD_DATA, 3, FEND]),
            vec![KissFrame::Data(vec![3])]
        );
    }

    #[test]
    fn test_kiss_decoder_pending_cap_boundary() {
        let mut decoder = KissDecoder::new();
        decoder.push(&[FEND]);
        // Pending bytes at the cap remain allowed.
        assert!(decoder.push(&vec![0; MAX_KISS_FRAME_LEN]).is_empty());
        assert_eq!(decoder.pending_len(), MAX_KISS_FRAME_LEN);
        // One more pending byte exceeds the cap and stops decoding.
        assert!(decoder.push(&[0]).is_empty());
        assert!(decoder.pending_len() > MAX_KISS_FRAME_LEN);
        assert!(decoder.push(&[FEND]).is_empty());

        // A frame exactly at the cap still completes when it closes.
        let mut decoder = KissDecoder::new();
        decoder.push(&[FEND]);
        assert!(decoder.push(&vec![0; MAX_KISS_FRAME_LEN]).is_empty());
        assert_eq!(
            decoder.push(&[FEND]),
            vec![KissFrame::Data(vec![0; MAX_KISS_FRAME_LEN - 1])]
        );
    }

    async fn test_client() -> (
        tokio::net::TcpStream,
        mpsc::Receiver<Vec<u8>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (stream, peer) = listener.accept().await.unwrap();
        let (tx, receiver) = mpsc::channel(64);
        let (air, _) = broadcast::channel(64);
        let task = tokio::spawn(handle_kiss_client(stream, peer.to_string(), tx, air));
        (client, receiver, task)
    }

    #[tokio::test]
    async fn test_kiss_handler_split_frames_and_eof() {
        use std::time::Duration;
        use tokio::time::timeout;
        let (mut client, mut tx, task) = test_client().await;
        for (payload, split) in [
            (vec![0x41; 200], 120),
            (vec![0x41; 200], 121),
            (vec![0x41, FEND, 0x42], 4),
            (vec![0x41, FESC, 0x42], 4),
        ] {
            let wire = kiss_encode_data(&payload);
            client.write_all(&wire[..split]).await.unwrap();
            assert!(
                timeout(Duration::from_millis(50), tx.recv()).await.is_err(),
                "unfinished prefix reached TX"
            );
            client.write_all(&wire[split..]).await.unwrap();
            assert_eq!(
                timeout(Duration::from_secs(2), tx.recv()).await.unwrap(),
                Some(payload)
            );
            assert!(
                timeout(Duration::from_millis(50), tx.recv()).await.is_err(),
                "duplicate payload"
            );
        }
        client
            .write_all(&[FEND, CMD_DATA, 0x41, FESC])
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tx.recv().await, None, "EOF must discard the partial frame");
    }

    #[tokio::test]
    async fn test_kiss_handler_pending_limit() {
        use std::time::Duration;
        use tokio::time::timeout;
        let (mut client, mut tx, mut task) = test_client().await;
        client.write_all(&[FEND]).await.unwrap();
        client
            .write_all(&vec![0; MAX_KISS_FRAME_LEN])
            .await
            .unwrap();
        // At the cap, neither TX nor connection closure is allowed.
        assert!(timeout(Duration::from_millis(50), tx.recv()).await.is_err());
        assert!(timeout(Duration::from_millis(50), &mut task).await.is_err());
        client.write_all(&[0]).await.unwrap();
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            tx.recv().await,
            None,
            "overflow must not forward a partial frame"
        );
    }

    #[test]
    fn kiss_decoder_keeps_an_escape_split_across_pushes() {
        let mut decoder = KissDecoder::new();
        assert!(decoder.push(&[FEND, CMD_DATA, FESC]).is_empty());
        assert_eq!(
            decoder.push(&[TFEND, FEND]),
            vec![KissFrame::Data(vec![FEND])]
        );
    }

    #[test]
    fn kiss_decoder_drops_data_on_a_nonzero_kiss_port() {
        assert!(KissDecoder::new().push(&[FEND, 0x10, 1, FEND]).is_empty());
    }

    #[test]
    fn kiss_decoder_reports_a_pending_frame_length() {
        let mut decoder = KissDecoder::new();
        decoder.push(&[FEND, CMD_DATA, 1]);
        assert_eq!(decoder.pending_len(), 2);
        decoder.push(&[2]);
        assert_eq!(decoder.pending_len(), 3);
        decoder.push(&[FEND]);
        assert_eq!(decoder.pending_len(), 0);
    }

    #[test]
    fn kiss_decoder_rejects_an_oversized_complete_frame() {
        let wire = kiss_encode_data(&vec![1; MAX_KISS_FRAME_LEN + 1]);
        let mut decoder = KissDecoder::new();
        assert!(decoder.push(&wire).is_empty());
        assert!(decoder.pending_len() > MAX_KISS_FRAME_LEN);
    }

    #[test]
    fn kiss_decoder_keeps_malformed_escape_passthrough() {
        assert_eq!(
            KissDecoder::new().push(&[FEND, CMD_DATA, FESC, 1, FEND]),
            vec![KissFrame::Data(vec![FESC, 1])]
        );
    }

    async fn prebound_server() -> (
        tokio::net::TcpStream,
        mpsc::Receiver<Vec<u8>>,
        broadcast::Sender<Vec<u8>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut server = KissServer::new(0);
        let receiver = server.take_tx_receiver().unwrap();
        let sender = server.rx_sender();
        let task = tokio::spawn(async move {
            server.run_with_listener(listener).await.unwrap();
        });
        let client = tokio::net::TcpStream::connect(addr).await.unwrap();
        (client, receiver, sender, task)
    }

    #[tokio::test]
    async fn split_tcp_write_reaches_tx_whole() {
        let (mut client, mut receiver, _, task) = prebound_server().await;
        let payload = special_payload();
        let wire = kiss_encode_data(&payload);
        for split in [
            wire.len() / 2,
            wire.iter().position(|&b| b == FESC).unwrap() + 1,
        ] {
            client.write_all(&wire[..split]).await.unwrap();
            client.flush().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            assert!(receiver.try_recv().is_err());
            client.write_all(&wire[split..]).await.unwrap();
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
                    .await
                    .unwrap()
                    .unwrap(),
                payload
            );
            assert!(receiver.try_recv().is_err());
        }
        task.abort();
    }

    #[tokio::test]
    async fn oversized_partial_frame_closes_the_client() {
        let (mut client, _, _, task) = prebound_server().await;
        let mut wire = vec![FEND, CMD_DATA];
        wire.extend(vec![1; MAX_KISS_FRAME_LEN + 1]);
        client.write_all(&wire).await.unwrap();
        let mut buf = [0];
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), client.read(&mut buf))
            .await
            .unwrap();
        assert!(matches!(result, Ok(0)) || result.is_err());
        task.abort();
    }

    #[tokio::test]
    async fn run_with_listener_serves_a_prebound_listener() {
        let (mut client, mut receiver, sender, task) = prebound_server().await;
        client.write_all(&kiss_encode_data(b"out")).await.unwrap();
        assert_eq!(receiver.recv().await.unwrap(), b"out");
        sender.send(b"in".to_vec()).unwrap();
        let mut wire = vec![0; kiss_encode_data(b"in").len()];
        client.read_exact(&mut wire).await.unwrap();
        assert_eq!(kiss_decode(&wire), vec![KissFrame::Data(b"in".to_vec())]);
        task.abort();
    }

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

        // Drive the handler directly with the server channels.
        let (tx_sender, mut tx_receiver) = mpsc::channel::<Vec<u8>>(64);
        let (rx_broadcast, _) = broadcast::channel::<Vec<u8>>(64);
        let rx_broadcast_clone = rx_broadcast.clone();

        let task = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            handle_kiss_client(stream, peer.to_string(), tx_sender, rx_broadcast_clone).await;
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
        let mut recv_buf = vec![0u8; kiss_encode_data(air_payload).len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.read_exact(&mut recv_buf),
        )
        .await
        .expect("timed out waiting for client receive")
        .expect("read error");

        let decoded = kiss_decode(&recv_buf);
        assert_eq!(decoded.len(), 1);
        match &decoded[0] {
            KissFrame::Data(payload) => assert_eq!(payload.as_slice(), air_payload),
            other => panic!("Expected Data, got {:?}", other),
        }
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    }
}
