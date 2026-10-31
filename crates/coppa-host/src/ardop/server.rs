//! ARDOP TCP server orchestrating command and data ports — the same
//! two-socket shape as `vara::server`, with the ARDOP differences (no
//! greeting, framed data port) and two client-ID namespaces of its own.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex, Semaphore};

use super::command::response_channel;
use super::data::data_channel;
use super::protocol::ArdopResponse;
use crate::HostEvent;

/// Maximum number of concurrent client connections accepted per port.
const MAX_CONCURRENT_CONNECTIONS: usize = 16;

/// ARDOP TNC host server with command (default 8515) and data (default
/// 8516) ports.
pub struct ArdopServer {
    command_port: u16,
    data_port: u16,
    bind_addr: String,
    event_tx: mpsc::Sender<HostEvent>,
    event_rx: Option<mpsc::Receiver<HostEvent>>,
    /// Counter for command port client IDs (`0xC000_xxxx` namespace).
    next_cmd_client_id: AtomicU32,
    /// Counter for data port client IDs (`0xA000_xxxx` namespace).
    next_data_client_id: AtomicU32,
    response_senders: Arc<Mutex<HashMap<u32, mpsc::Sender<ArdopResponse>>>>,
    data_senders: Arc<Mutex<HashMap<u32, mpsc::Sender<Vec<u8>>>>>,
}

impl ArdopServer {
    /// Create a new ARDOP server on the specified ports.
    pub fn new(command_port: u16, data_port: u16) -> Self {
        Self::with_bind_addr(command_port, data_port, "127.0.0.1".to_string())
    }

    /// Create a new ARDOP server with a custom bind address.
    pub fn with_bind_addr(command_port: u16, data_port: u16, bind_addr: String) -> Self {
        let (event_tx, event_rx) = mpsc::channel(256);
        Self {
            command_port,
            data_port,
            bind_addr,
            event_tx,
            event_rx: Some(event_rx),
            next_cmd_client_id: AtomicU32::new(super::FIRST_ARDOP_CMD_CLIENT_ID),
            next_data_client_id: AtomicU32::new(super::FIRST_ARDOP_DATA_CLIENT_ID),
            response_senders: Arc::new(Mutex::new(HashMap::new())),
            data_senders: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Take the event receiver (can only be called once).
    pub fn take_event_rx(&mut self) -> Option<mpsc::Receiver<HostEvent>> {
        self.event_rx.take()
    }

    /// Allocate a new command port client ID (`0xC000_xxxx` namespace).
    pub fn next_cmd_client_id(&self) -> u32 {
        self.next_cmd_client_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Allocate a new data port client ID (`0xA000_xxxx` namespace).
    pub fn next_data_client_id(&self) -> u32 {
        self.next_data_client_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Get the configured command port.
    pub fn command_port(&self) -> u16 {
        self.command_port
    }

    /// Get the configured data port.
    pub fn data_port(&self) -> u16 {
        self.data_port
    }

    /// Get a clone of the response senders map for sending responses to
    /// connected clients.
    pub fn response_senders(&self) -> Arc<Mutex<HashMap<u32, mpsc::Sender<ArdopResponse>>>> {
        self.response_senders.clone()
    }

    /// Get a clone of the data senders map for sending data to connected
    /// data-port clients. Payloads are raw bytes; the writer adds framing.
    pub fn data_senders(&self) -> Arc<Mutex<HashMap<u32, mpsc::Sender<Vec<u8>>>>> {
        self.data_senders.clone()
    }

    /// Bind both ports and run. Callers that need OS-assigned ports bind
    /// first and use [`ArdopServer::run_with_listeners`].
    pub async fn run(&self) -> Result<(), crate::HostError> {
        let cmd_addr = format!("{}:{}", self.bind_addr, self.command_port);
        let data_addr = format!("{}:{}", self.bind_addr, self.data_port);

        let cmd_listener = TcpListener::bind(&cmd_addr).await?;
        let data_listener = TcpListener::bind(&data_addr).await?;

        self.run_with_listeners(cmd_listener, data_listener).await
    }

    /// Run the server's accept loops on listeners the caller already bound
    /// (bind-before-spawn, COP-32: a taken port is the caller's startup
    /// error). The configured ports are ignored on this path.
    pub async fn run_with_listeners(
        &self,
        cmd_listener: TcpListener,
        data_listener: TcpListener,
    ) -> Result<(), crate::HostError> {
        let event_tx = self.event_tx.clone();
        let response_senders = self.response_senders.clone();

        // Accept loop for command port
        let cmd_event_tx = event_tx.clone();
        let cmd_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
        let cmd_task = async {
            loop {
                match cmd_listener.accept().await {
                    Ok((stream, _addr)) => {
                        // Cap concurrent connections; drop the client if at
                        // capacity.
                        let permit = match cmd_semaphore.clone().try_acquire_owned() {
                            Ok(p) => p,
                            Err(_) => {
                                tracing::warn!(
                                    "ARDOP command port connection limit reached, dropping client"
                                );
                                drop(stream);
                                continue;
                            }
                        };
                        let client_id = self.next_cmd_client_id();
                        let ev_tx = cmd_event_tx.clone();
                        let (resp_tx, resp_rx) = response_channel();
                        let resp_senders = response_senders.clone();
                        resp_senders.lock().await.insert(client_id, resp_tx);
                        let resp_senders_cleanup = resp_senders.clone();
                        let cleanup_id = client_id;
                        tokio::spawn(async move {
                            let _permit = permit;
                            if let Err(e) =
                                super::command::handle_client(stream, client_id, ev_tx, resp_rx)
                                    .await
                            {
                                tracing::warn!(client_id, error = %e, "ARDOP command client error");
                            }
                            resp_senders_cleanup.lock().await.remove(&cleanup_id);
                        });
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "ARDOP command port accept error");
                    }
                }
            }
        };

        // Accept loop for data port
        let data_event_tx = event_tx.clone();
        let data_senders = self.data_senders.clone();
        let data_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
        let data_task = async {
            loop {
                match data_listener.accept().await {
                    Ok((stream, _addr)) => {
                        let permit = match data_semaphore.clone().try_acquire_owned() {
                            Ok(p) => p,
                            Err(_) => {
                                tracing::warn!(
                                    "ARDOP data port connection limit reached, dropping client"
                                );
                                drop(stream);
                                continue;
                            }
                        };
                        let client_id = self.next_data_client_id();
                        let ev_tx = data_event_tx.clone();
                        let (data_tx, data_rx) = data_channel();
                        let ds = data_senders.clone();
                        ds.lock().await.insert(client_id, data_tx);
                        let ds_cleanup = ds.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            if let Err(e) =
                                super::data::handle_client(stream, client_id, ev_tx, data_rx).await
                            {
                                tracing::warn!(client_id, error = %e, "ARDOP data client error");
                            }
                            ds_cleanup.lock().await.remove(&client_id);
                        });
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "ARDOP data port accept error");
                    }
                }
            }
        };

        tokio::select! {
            _ = cmd_task => {}
            _ = data_task => {}
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ids_use_the_two_ardop_namespaces() {
        let server = ArdopServer::new(9515, 9516);
        assert_eq!(server.command_port(), 9515);
        assert_eq!(server.data_port(), 9516);
        // Command IDs are in the 0xC000_xxxx namespace, data IDs in
        // 0xA000_xxxx — both disjoint from VARA command (1+), VARA data
        // (0x8000_0001+) and WebSocket (0x4000_0001+).
        assert_eq!(server.next_cmd_client_id(), 0xC000_0001);
        assert_eq!(server.next_cmd_client_id(), 0xC000_0002);
        assert_eq!(server.next_data_client_id(), 0xA000_0001);
        assert_eq!(server.next_data_client_id(), 0xA000_0002);
    }

    #[test]
    fn take_event_rx_yields_once() {
        let mut server = ArdopServer::new(9515, 9516);
        assert!(server.take_event_rx().is_some());
        assert!(server.take_event_rx().is_none());
    }

    /// The queue depths mirror the VARA command/data port policy.
    #[test]
    fn queue_depths_match_vara_policy() {
        assert_eq!(crate::ardop::command::COMMAND_QUEUE_DEPTH, 1024);
        assert_eq!(crate::ardop::data::DATA_QUEUE_DEPTH, 1024);
        assert_eq!(crate::ardop::command::TELEMETRY_QUEUE_DEPTH, 64);
    }
}
