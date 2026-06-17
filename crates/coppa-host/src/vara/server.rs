//! VARA TCP server orchestrating command and data ports.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex, Semaphore};

use super::command::VaraCommandPort;
use super::data::VaraDataPort;
use super::protocol::VaraResponse;
use crate::HostEvent;

/// Maximum number of concurrent client connections accepted per port.
const MAX_CONCURRENT_CONNECTIONS: usize = 16;

/// VARA-style TCP control server (not RF/waveform-compatible with VARA) with
/// command (8300) and data (8301) ports.
pub struct VaraServer {
    command_port: u16,
    data_port: u16,
    bind_addr: String,
    event_tx: mpsc::Sender<HostEvent>,
    event_rx: Option<mpsc::Receiver<HostEvent>>,
    /// E2: Separate counter for command port client IDs (0x0000_xxxx namespace).
    next_cmd_client_id: AtomicU32,
    /// E2: Separate counter for data port client IDs (0x8000_xxxx namespace).
    next_data_client_id: AtomicU32,
    response_senders: Arc<Mutex<HashMap<u32, mpsc::Sender<VaraResponse>>>>,
    data_senders: Arc<Mutex<HashMap<u32, mpsc::Sender<Vec<u8>>>>>,
}

impl VaraServer {
    /// Create a new VARA server on the specified ports.
    pub fn new(command_port: u16, data_port: u16) -> Self {
        Self::with_bind_addr(command_port, data_port, "127.0.0.1".to_string())
    }

    /// Create a new VARA server with a custom bind address.
    pub fn with_bind_addr(command_port: u16, data_port: u16, bind_addr: String) -> Self {
        let (event_tx, event_rx) = mpsc::channel(256);
        Self {
            command_port,
            data_port,
            bind_addr,
            event_tx,
            event_rx: Some(event_rx),
            // E2: Command IDs start at 1, data IDs start at 0x8000_0001
            next_cmd_client_id: AtomicU32::new(1),
            next_data_client_id: AtomicU32::new(0x8000_0001),
            response_senders: Arc::new(Mutex::new(HashMap::new())),
            data_senders: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create with default VARA ports (8300/8301).
    pub fn default_ports() -> Self {
        Self::new(8300, 8301)
    }

    /// Take the event receiver (can only be called once).
    pub fn take_event_rx(&mut self) -> Option<mpsc::Receiver<HostEvent>> {
        self.event_rx.take()
    }

    /// Get a clone of the event sender for external use.
    pub fn event_sender(&self) -> mpsc::Sender<HostEvent> {
        self.event_tx.clone()
    }

    /// Allocate a new command port client ID (0x0000_xxxx namespace).
    pub fn next_cmd_client_id(&self) -> u32 {
        self.next_cmd_client_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Allocate a new data port client ID (0x8000_xxxx namespace).
    pub fn next_data_client_id(&self) -> u32 {
        self.next_data_client_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Allocate a new client ID (deprecated — use port-specific methods).
    pub fn next_client_id(&self) -> u32 {
        self.next_cmd_client_id()
    }

    /// Get the configured command port.
    pub fn command_port(&self) -> u16 {
        self.command_port
    }

    /// Get the configured data port.
    pub fn data_port(&self) -> u16 {
        self.data_port
    }

    /// Get a clone of the response senders map for sending responses to connected clients.
    pub fn response_senders(&self) -> Arc<Mutex<HashMap<u32, mpsc::Sender<VaraResponse>>>> {
        self.response_senders.clone()
    }

    /// Get a clone of the data senders map for sending data to connected data-port clients.
    pub fn data_senders(&self) -> Arc<Mutex<HashMap<u32, mpsc::Sender<Vec<u8>>>>> {
        self.data_senders.clone()
    }

    /// Start the VARA TCP server, binding to both command and data ports.
    ///
    /// This spawns accept loops on both ports that dispatch incoming
    /// connections to the appropriate handler. Runs until the returned
    /// shutdown sender is dropped or triggered.
    pub async fn run(&self) -> anyhow::Result<()> {
        let cmd_addr = format!("{}:{}", self.bind_addr, self.command_port);
        let data_addr = format!("{}:{}", self.bind_addr, self.data_port);

        let cmd_listener = TcpListener::bind(&cmd_addr).await?;
        let data_listener = TcpListener::bind(&data_addr).await?;

        let event_tx = self.event_tx.clone();
        let next_cmd_id = &self.next_cmd_client_id;
        let next_data_id = &self.next_data_client_id;
        let response_senders = self.response_senders.clone();

        // Accept loop for command port
        let cmd_event_tx = event_tx.clone();
        let cmd_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
        let cmd_task = async {
            loop {
                match cmd_listener.accept().await {
                    Ok((stream, _addr)) => {
                        // Cap concurrent connections; drop the client if at capacity.
                        let permit = match cmd_semaphore.clone().try_acquire_owned() {
                            Ok(p) => p,
                            Err(_) => {
                                eprintln!("Command port connection limit reached, dropping client");
                                drop(stream);
                                continue;
                            }
                        };
                        // E2: Use command-port namespace
                        let client_id = next_cmd_id.fetch_add(1, Ordering::Relaxed);
                        let ev_tx = cmd_event_tx.clone();
                        // Create a response channel for this client
                        let (resp_tx, resp_rx) = mpsc::channel::<VaraResponse>(64);
                        let resp_senders = response_senders.clone();
                        resp_senders.lock().await.insert(client_id, resp_tx);
                        let resp_senders_cleanup = resp_senders.clone();
                        let cleanup_id = client_id;
                        tokio::spawn(async move {
                            let _permit = permit;
                            if let Err(e) =
                                VaraCommandPort::handle_client(stream, client_id, ev_tx, resp_rx)
                                    .await
                            {
                                eprintln!("Command client {} error: {}", client_id, e);
                            }
                            resp_senders_cleanup.lock().await.remove(&cleanup_id);
                        });
                    }
                    Err(e) => {
                        eprintln!("Command port accept error: {}", e);
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
                        // Cap concurrent connections; drop the client if at capacity.
                        let permit = match data_semaphore.clone().try_acquire_owned() {
                            Ok(p) => p,
                            Err(_) => {
                                eprintln!("Data port connection limit reached, dropping client");
                                drop(stream);
                                continue;
                            }
                        };
                        // E2: Use data-port namespace
                        let client_id = next_data_id.fetch_add(1, Ordering::Relaxed);
                        let ev_tx = data_event_tx.clone();
                        let (data_tx, data_rx) = mpsc::channel::<Vec<u8>>(64);
                        let ds = data_senders.clone();
                        ds.lock().await.insert(client_id, data_tx);
                        let ds_cleanup = ds.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            if let Err(e) =
                                VaraDataPort::handle_client(stream, client_id, ev_tx, data_rx).await
                            {
                                eprintln!("Data client {} error: {}", client_id, e);
                            }
                            ds_cleanup.lock().await.remove(&client_id);
                        });
                    }
                    Err(e) => {
                        eprintln!("Data port accept error: {}", e);
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
    fn test_vara_server_default() {
        let server = VaraServer::default_ports();
        assert_eq!(server.command_port(), 8300);
        assert_eq!(server.data_port(), 8301);
    }

    #[test]
    fn test_vara_server_custom_ports() {
        let server = VaraServer::new(9300, 9301);
        assert_eq!(server.command_port(), 9300);
        assert_eq!(server.data_port(), 9301);
    }

    #[test]
    fn test_vara_server_client_ids() {
        let server = VaraServer::default_ports();
        // E2: Command IDs are in the 0x0000_xxxx namespace
        assert_eq!(server.next_cmd_client_id(), 1);
        assert_eq!(server.next_cmd_client_id(), 2);
        assert_eq!(server.next_cmd_client_id(), 3);
        // E2: Data IDs are in the 0x8000_xxxx namespace
        assert_eq!(server.next_data_client_id(), 0x8000_0001);
        assert_eq!(server.next_data_client_id(), 0x8000_0002);
        // Legacy method still works (uses cmd namespace)
        assert_eq!(server.next_client_id(), 4);
    }

    #[test]
    fn test_vara_server_event_rx() {
        let mut server = VaraServer::default_ports();
        assert!(server.take_event_rx().is_some());
        assert!(server.take_event_rx().is_none());
    }

    #[tokio::test]
    async fn test_vara_server_binds_and_accepts() {
        use tokio::net::TcpStream;

        // Use high ports that are likely free
        let server = VaraServer::new(18300, 18301);
        let _event_tx = server.event_sender();

        let server_task = tokio::spawn(async move {
            let _ = server.run().await;
        });

        // Give server time to bind
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Connect to command port
        let result = TcpStream::connect("127.0.0.1:18300").await;
        assert!(result.is_ok(), "Should connect to command port");

        // Connect to data port
        let result = TcpStream::connect("127.0.0.1:18301").await;
        assert!(result.is_ok(), "Should connect to data port");

        server_task.abort();
    }
}
