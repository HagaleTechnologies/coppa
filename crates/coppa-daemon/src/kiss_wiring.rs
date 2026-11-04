//! KISS TCP wiring for broadcast datagrams on the OFDM modem.
use crate::config::HostConfig;
use crate::event_loop::{max_kiss_datagram_len, DaemonEvent, EventLoop};
use coppa_host::kiss::KissServer;

/// Serve a pre-bound listener and connect its datagrams to the modem.
pub fn serve_kiss(
    event_loop: &mut EventLoop,
    listener: tokio::net::TcpListener,
    host: &HostConfig,
) {
    let mut server = KissServer::with_bind_addr(host.kiss_port, host.bind_address.clone());
    let mut rx = server.take_tx_receiver().expect("new server");
    let event_tx = event_loop.event_sender();
    event_loop.set_kiss_rx(server.rx_sender());
    tracing::info!(
        addr = %crate::vara_wiring::listen_addr(&listener),
        level = event_loop.speed_level(),
        max_datagram_bytes = max_kiss_datagram_len(event_loop.speed_level()),
        "KISS datagram interface listening"
    );
    tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            if event_tx
                .send(DaemonEvent::KissDatagram(data))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    tokio::spawn(async move {
        if let Err(error) = server.run_with_listener(listener).await {
            tracing::error!(%error, "KISS datagram server stopped");
        }
    });
}
