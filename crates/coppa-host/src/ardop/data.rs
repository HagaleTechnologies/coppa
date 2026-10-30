//! ARDOP data port handler.
//!
//! Unlike the VARA data port (raw bytes), the ARDOP data port frames both
//! directions: host→TNC `<len:u16 BE><payload>`, TNC→host
//! `<len:u16 BE><tag:3><payload>` with tag `ARQ` for session data (the
//! framing Pat's `frame.go` reads; see `frame.rs`).

use anyhow::Result;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

use super::frame::{encode_tnc_frame, read_host_frame, ARQ_TAG};
use crate::HostEvent;

/// Payloads each data-port client's queue holds. A payload that finds the
/// queue full means the client has stopped reading, and the caller
/// disconnects it rather than skip the payload.
pub const DATA_QUEUE_DEPTH: usize = 1024;

/// A data-port client's data channel, [`DATA_QUEUE_DEPTH`] payloads deep.
/// Payloads are raw bytes; the writer adds the framing.
pub fn data_channel() -> (mpsc::Sender<Vec<u8>>, mpsc::Receiver<Vec<u8>>) {
    mpsc::channel(DATA_QUEUE_DEPTH)
}

/// What [`queue_data`] did with a payload.
#[derive(Debug, PartialEq)]
pub enum DataQueued {
    /// The payload is queued for the client.
    Sent,
    /// The queue is full and the payload, returned here, is not queued. The
    /// client has stopped reading; disconnect it by dropping its sender.
    Overflow(Vec<u8>),
    /// The client's connection has already ended.
    Closed,
}

/// Queue `data` for one data-port client without waiting.
///
/// Every payload is queued in order, or returned as [`DataQueued::Overflow`]
/// when the queue is full, so a client never sees a gap in its data while its
/// connection stays open.
pub fn queue_data(tx: &mpsc::Sender<Vec<u8>>, data: Vec<u8>) -> DataQueued {
    match tx.try_send(data) {
        Ok(()) => DataQueued::Sent,
        Err(TrySendError::Full(data)) => DataQueued::Overflow(data),
        Err(TrySendError::Closed(_)) => DataQueued::Closed,
    }
}

/// Handle a single data port client connection: read framed host→TNC
/// payloads (forwarding the stripped payload as `DataReceived`), write
/// TNC→host frames with the `ARQ` tag around everything queued on `data_rx`.
pub async fn handle_client(
    stream: TcpStream,
    client_id: u32,
    event_tx: mpsc::Sender<HostEvent>,
    mut data_rx: mpsc::Receiver<Vec<u8>>,
) -> Result<()> {
    let (mut reader, mut writer) = stream.into_split();

    let event_tx_clone = event_tx.clone();
    let (read_done_tx, mut read_done_rx) = tokio::sync::oneshot::channel::<()>();

    // Read framed data from client: the framing is stripped before the
    // payload becomes a `DataReceived` event.
    let read_task = tokio::spawn(async move {
        while let Ok(payload) = read_host_frame(&mut reader).await {
            let event = HostEvent::DataReceived {
                client_id,
                data: payload,
            };
            if event_tx_clone.send(event).await.is_err() {
                break;
            }
        }
        // EOF or a truncated frame ends the whole data connection, even
        // when the server still has its sender registered.
        drop(read_done_tx);
    });
    let read_abort = read_task.abort_handle();

    // Write framed data to client: every queued payload leaves as
    // `<len><ARQ><payload>`.
    let write_task = tokio::spawn(async move {
        loop {
            let data = tokio::select! {
                data = data_rx.recv() => match data {
                    Some(data) => data,
                    None => break,
                },
                _ = &mut read_done_rx => break,
            };
            let Ok(frame) = encode_tnc_frame(ARQ_TAG, &data) else {
                tracing::warn!(
                    client_id,
                    "ARDOP outbound frame exceeds wire length; closing"
                );
                break;
            };
            if writer.write_all(&frame).await.is_err() {
                break;
            }
        }
        // Writing has stopped, so stop reading too: the socket closes and
        // the client's writes stop reaching the daemon.
        read_abort.abort();
    });

    let _ = tokio::join!(read_task, write_task);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    /// Write `<00 03 61 62 63>` and a second frame in one TCP segment:
    /// two `DataReceived` events with the framing stripped.
    #[tokio::test]
    async fn data_port_decodes_frames_and_forwards_payload() {
        use tokio::io::AsyncWriteExt;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (server, client) = tokio::join!(listener.accept(), TcpStream::connect(addr));
        let (server, _) = server.unwrap();
        let (read_half, mut write_half) = client.unwrap().into_split();
        drop(read_half);

        let (event_tx, mut event_rx) = mpsc::channel(16);
        let (data_tx, data_rx) = data_channel();
        let handler = tokio::spawn(handle_client(server, 0xA000_0001, event_tx, data_rx));

        // Two frames in one write: both decode, in order, framing stripped.
        write_half
            .write_all(&[0x00u8, 0x03, b'a', b'b', b'c', 0x00, 0x02, b'd', b'e'])
            .await
            .unwrap();
        for expected in [b"abc".to_vec(), b"de".to_vec()] {
            let event = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
                .await
                .expect("timed out waiting for a DataReceived")
                .expect("event channel closed");
            match event {
                HostEvent::DataReceived { client_id, data } => {
                    assert_eq!(client_id, 0xA000_0001);
                    assert_eq!(data, expected);
                }
                other => panic!("expected DataReceived, got {other:?}"),
            }
        }
        drop(write_half);
        drop(data_tx);
        tokio::time::timeout(Duration::from_secs(5), handler)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    /// A payload pushed into the client's data channel leaves the socket as
    /// `<len><ARQ><payload>`.
    #[tokio::test]
    async fn data_port_writer_frames_with_arq_tag() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (accepted, client) = tokio::join!(listener.accept(), TcpStream::connect(addr));
        let (server, _) = accepted.unwrap();
        let (mut client_read, _client_write) = client.unwrap().into_split();

        let (event_tx, _event_rx) = mpsc::channel(16);
        let (data_tx, data_rx) = data_channel();
        let handler = tokio::spawn(handle_client(server, 0xA000_0001, event_tx, data_rx));

        assert_eq!(queue_data(&data_tx, b"demo".to_vec()), DataQueued::Sent);
        drop(data_tx);
        let mut frame = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), client_read.read_to_end(&mut frame))
            .await
            .expect("the client must see EOF")
            .unwrap();
        assert_eq!(
            frame,
            [0x00, 0x07, b'A', b'R', b'Q', b'd', b'e', b'm', b'o'],
            "len=7, tag ARQ, payload demo"
        );
        tokio::time::timeout(Duration::from_secs(5), handler)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    /// A truncated frame closes the socket even while the server retains
    /// the output sender. No partial payload may escape to the daemon.
    #[tokio::test]
    async fn truncated_frame_closes_both_halves() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let (events, mut rx) = mpsc::channel(4);
        let (_data_tx, data_rx) = data_channel();
        let handler = tokio::spawn(handle_client(server, 1, events, data_rx));
        client.write_all(&[0, 4, b'a']).await.unwrap();
        client.shutdown().await.unwrap();
        let mut received = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), client.read_to_end(&mut received))
            .await
            .expect("truncated input must close the output half")
            .unwrap();
        assert!(received.is_empty());
        tokio::time::timeout(Duration::from_secs(1), handler)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            rx.recv().await.is_none(),
            "partial payload must never be delivered"
        );
    }

    #[test]
    fn queue_data_reports_overflow_and_closed() {
        let (tx, mut rx) = data_channel();
        for i in 0..DATA_QUEUE_DEPTH {
            assert_eq!(queue_data(&tx, vec![i as u8]), DataQueued::Sent);
        }
        assert_eq!(
            queue_data(&tx, b"late".to_vec()),
            DataQueued::Overflow(b"late".to_vec())
        );
        while rx.try_recv().is_ok() {}
        drop(rx);
        assert_eq!(queue_data(&tx, b"after".to_vec()), DataQueued::Closed);
    }
}
