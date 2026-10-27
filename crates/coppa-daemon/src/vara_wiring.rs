//! The VARA-style TCP server's wiring into the daemon event loop, shared by
//! `coppad`'s `main` and the integration tests so both run the same code
//! (COP-46: a test of a re-typed copy of this wiring can drift from
//! production without failing).

use crate::config::HostConfig;
use crate::event_loop::{DaemonEvent, EventLoop};

/// Start the VARA server on already-bound listeners and wire it to `event_loop`:
/// host events in, command-port telemetry out, `DataOut` to the data port.
///
/// `response_rx` is the receiving end of the channel handed to
/// [`EventLoop::set_response_tx`]. The listeners are bound by the caller so a
/// port that is already taken is a startup error (COP-32), not a log line from
/// a spawned task. Must be called inside a Tokio runtime; the server and both
/// bridges run as spawned tasks.
pub fn serve_vara(
    event_loop: &mut EventLoop,
    mut response_rx: tokio::sync::mpsc::Receiver<coppa_host::HostResponse>,
    cmd_listener: tokio::net::TcpListener,
    data_listener: tokio::net::TcpListener,
    host: &HostConfig,
) {
    let mut vara_server = coppa_host::vara::VaraServer::with_bind_addr(
        host.vara_command_port,
        host.vara_data_port,
        host.bind_address.clone(),
    );
    tracing::info!(
        command_addr = %listen_addr(&cmd_listener),
        data_addr = %listen_addr(&data_listener),
        "VARA TCP server listening"
    );

    // Pipe VARA events into the daemon event loop
    if let Some(mut vara_rx) = vara_server.take_event_rx() {
        let vara_event_tx = event_loop.event_sender();
        tokio::spawn(async move {
            while let Some(host_event) = vara_rx.recv().await {
                if vara_event_tx
                    .send(DaemonEvent::Host(host_event))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    // Wire VARA command-port response senders so the event loop can broadcast
    // SNR/PTT/BUFFER/BUSY telemetry (decision 8, Phase 3 Task 7).
    event_loop.set_vara_responses(vara_server.response_senders());

    // Bridge response_rx to VARA data_senders (decoded data goes via the data port)
    let vara_data_senders = vara_server.data_senders();
    tokio::spawn(async move {
        while let Some(response) = response_rx.recv().await {
            // StatusUpdate is deliberately not forwarded: EventLoop::notify_session
            // pushes session state directly to the command port and WebSocket.
            if let coppa_host::HostResponse::DataOut { client_id, data } = &response {
                let senders = vara_data_senders.lock().await;
                if *client_id == 0 {
                    // Broadcast to all data-port clients
                    for tx in senders.values() {
                        let _ = tx.try_send(data.clone());
                    }
                } else if let Some(tx) = senders.get(client_id) {
                    let _ = tx.try_send(data.clone());
                }
            }
        }
    });

    // Serve the listeners bound above
    tokio::spawn(async move {
        if let Err(e) = vara_server
            .run_with_listeners(cmd_listener, data_listener)
            .await
        {
            tracing::error!("VARA server stopped: {e:#}");
        }
    });
}

/// A listener's bound address for logs, e.g. `127.0.0.1:8300`.
pub fn listen_addr(listener: &tokio::net::TcpListener) -> String {
    listener
        .local_addr()
        .map_or_else(|e| format!("unknown ({e})"), |addr| addr.to_string())
}
