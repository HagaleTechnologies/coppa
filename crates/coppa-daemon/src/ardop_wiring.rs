//! The ARDOP TNC host server's wiring into the daemon event loop (COP-97),
//! the ARDOP twin of `vara_wiring::serve_vara` — shared by `coppad`'s
//! `main` and the tests so both run the same code.

use crate::config::HostConfig;
use crate::event_loop::{DaemonEvent, EventLoop, VaraDataSenders};

use coppa_host::ardop::ArdopServer;

/// Start the ARDOP server on already-bound listeners and wire it to
/// `event_loop`: host events in, command-port replies out. Returns the
/// server's data senders for the shared `DataOut` fan-out
/// (`event_loop::forward_data_out`), which the caller spawns once for every
/// enabled data-carrying surface.
///
/// The listeners are bound by the caller so a port that is already taken is
/// a startup error (COP-32), not a log line from a spawned task. Must be
/// called inside a Tokio runtime; the server and the event bridge run as
/// spawned tasks.
pub fn serve_ardop(
    event_loop: &mut EventLoop,
    cmd_listener: tokio::net::TcpListener,
    data_listener: tokio::net::TcpListener,
    host: &HostConfig,
) -> VaraDataSenders {
    let mut ardop_server = ArdopServer::with_bind_addr(
        host.ardop_command_port,
        host.ardop_data_port,
        host.bind_address.clone(),
    );
    tracing::info!(
        command_addr = %crate::vara_wiring::listen_addr(&cmd_listener),
        data_addr = %crate::vara_wiring::listen_addr(&data_listener),
        "ARDOP TNC host interface listening"
    );

    // Pipe ARDOP events into the daemon event loop
    if let Some(mut ardop_rx) = ardop_server.take_event_rx() {
        let ardop_event_tx = event_loop.event_sender();
        tokio::spawn(async move {
            while let Some(host_event) = ardop_rx.recv().await {
                if ardop_event_tx
                    .send(DaemonEvent::Host(host_event))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    // Wire ARDOP command-port response senders so the event loop can push
    // NEWSTATE/CONNECTED/TARGET/DISCONNECTED/BUSY/PTT/BUFFER lines and
    // answer commands.
    event_loop.set_ardop_responses(ardop_server.response_senders());

    let data_senders = ardop_server.data_senders();

    // Serve the listeners bound above
    tokio::spawn(async move {
        if let Err(e) = ardop_server
            .run_with_listeners(cmd_listener, data_listener)
            .await
        {
            tracing::error!("ARDOP server stopped: {e:#}");
        }
    });

    data_senders
}
