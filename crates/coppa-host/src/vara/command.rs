//! VARA command port handler (TCP port 8300).

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use super::protocol::{VaraCommand, VaraResponse};
use crate::HostEvent;

/// Maximum length of a single command line, in bytes. Lines longer than this
/// cause the connection to be closed, bounding per-client memory use.
const MAX_LINE: u64 = 4096;

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
        let version_resp = VaraResponse::Version("Coppa 0.1.0".to_string());
        writer.write_all(version_resp.format().as_bytes()).await?;

        // Notify connection
        let _ = event_tx.send(HostEvent::Connected { client_id }).await;

        let event_tx_clone = event_tx.clone();

        // Read commands — when this task ends, it drops read_done_tx,
        // which signals the write task to stop.
        let (read_done_tx, mut read_done_rx) = mpsc::channel::<()>(1);
        let read_task = tokio::spawn(async move {
            let mut line = String::new();
            loop {
                line.clear();
                // Bounded read: cap each line at MAX_LINE bytes so a client cannot
                // force unbounded buffer growth by never sending a newline.
                let mut limited = (&mut reader).take(MAX_LINE);
                match limited.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        // If we hit the cap without a trailing newline, the line was
                        // over-long (or truncated mid-line): close the connection.
                        if n as u64 == MAX_LINE && !line.ends_with('\n') {
                            eprintln!("Command client {} sent over-long line, closing", client_id);
                            break;
                        }
                        let cmd = VaraCommand::parse(&line);
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
                                command: line.trim().to_string(),
                            },
                        };
                        if event_tx_clone.send(event).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_port_new() {
        let _port = VaraCommandPort::new(8300);
    }
}
