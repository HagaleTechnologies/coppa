//! VARA data port handler (TCP port 8301).

use crate::HostError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

use crate::HostEvent;

/// Payloads each data-port client's queue holds. A payload that finds the
/// queue full means the client has stopped reading, and the caller
/// disconnects it rather than skip the payload.
pub const DATA_QUEUE_DEPTH: usize = 1024;

/// A data-port client's data channel, [`DATA_QUEUE_DEPTH`] payloads deep.
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

/// VARA data port TCP handler.
pub struct VaraDataPort {
    #[allow(dead_code)]
    port: u16,
}

impl VaraDataPort {
    /// Create a new data port handler on the given TCP port.
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    /// Handle a single data port client connection.
    pub async fn handle_client(
        stream: TcpStream,
        client_id: u32,
        event_tx: mpsc::Sender<HostEvent>,
        mut data_rx: mpsc::Receiver<Vec<u8>>,
    ) -> Result<(), HostError> {
        let (mut reader, mut writer) = stream.into_split();

        let event_tx_clone = event_tx.clone();

        // Read data from client
        let read_task = tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let event = HostEvent::DataReceived {
                            client_id,
                            data: buf[..n].to_vec(),
                        };
                        if event_tx_clone.send(event).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let read_abort = read_task.abort_handle();

        // Write data to client
        let write_task = tokio::spawn(async move {
            while let Some(data) = data_rx.recv().await {
                if writer.write_all(&data).await.is_err() {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_port_new() {
        let port = VaraDataPort::new(8301);
        assert_eq!(port.port, 8301);
    }

    fn numbered(n: usize) -> Vec<Vec<u8>> {
        (0..n).map(|i| (i as u32).to_be_bytes().to_vec()).collect()
    }

    #[test]
    fn queue_data_queues_every_payload_up_to_the_depth_in_order() {
        let (tx, mut rx) = data_channel();
        let payloads = numbered(DATA_QUEUE_DEPTH);
        for payload in &payloads {
            assert_eq!(queue_data(&tx, payload.clone()), DataQueued::Sent);
        }
        let mut got = Vec::new();
        while let Ok(payload) = rx.try_recv() {
            got.push(payload);
        }
        assert_eq!(got, payloads);
    }

    #[test]
    fn queue_data_reports_overflow_when_the_queue_is_full() {
        let (tx, mut rx) = data_channel();
        let payloads = numbered(DATA_QUEUE_DEPTH);
        for payload in &payloads {
            assert_eq!(queue_data(&tx, payload.clone()), DataQueued::Sent);
        }
        assert_eq!(
            queue_data(&tx, b"late".to_vec()),
            DataQueued::Overflow(b"late".to_vec())
        );
        let mut got = Vec::new();
        while let Ok(payload) = rx.try_recv() {
            got.push(payload);
        }
        assert_eq!(got, payloads, "the late payload must not be queued");
        drop(rx);
        assert_eq!(queue_data(&tx, b"after".to_vec()), DataQueued::Closed);
    }

    #[tokio::test]
    async fn dropping_the_data_sender_closes_the_data_connection() {
        use std::time::Duration;
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (accepted, client) = tokio::join!(listener.accept(), TcpStream::connect(addr));
        let (server, _) = accepted.unwrap();
        let (mut client_read, _client_write) = client.unwrap().into_split();
        let (event_tx, _event_rx) = mpsc::channel(16);
        let (data_tx, data_rx) = data_channel();
        let handler = tokio::spawn(VaraDataPort::handle_client(
            server,
            0x8000_0001,
            event_tx,
            data_rx,
        ));
        assert_eq!(queue_data(&data_tx, b"one".to_vec()), DataQueued::Sent);
        assert_eq!(queue_data(&data_tx, b"two".to_vec()), DataQueued::Sent);
        drop(data_tx);
        let mut got = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), client_read.read_to_end(&mut got))
            .await
            .expect("the client must see EOF")
            .unwrap();
        assert_eq!(got, b"onetwo");
        tokio::time::timeout(Duration::from_secs(5), handler)
            .await
            .expect("the handler must return while the client still holds its write half")
            .unwrap()
            .unwrap();
    }
}
