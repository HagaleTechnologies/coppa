//! VARA data port handler (TCP port 8301).

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use crate::HostEvent;

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
    ) -> Result<()> {
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

        // Write data to client
        let write_task = tokio::spawn(async move {
            while let Some(data) = data_rx.recv().await {
                if writer.write_all(&data).await.is_err() {
                    break;
                }
            }
        });

        let _ = tokio::try_join!(read_task, write_task);

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
}
