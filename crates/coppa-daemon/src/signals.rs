//! COP-85: the signals that stop or reload `coppad`.
//!
//! Every handler is registered by [`Signals::register`] before the daemon
//! logs `Daemon ready`, so a signal sent right after that line never meets a
//! default action. The first stop request (SIGINT or SIGTERM; on Windows also
//! Ctrl-Break, closing the console window and Windows shutting down) starts
//! the orderly shutdown. Any later one forces the exit: the daemon releases
//! PTT itself and exits [`FORCED_EXIT_STATUS`], and if that cannot finish
//! within [`FORCED_EXIT_GRACE`] the process exits anyway. On Unix, SIGHUP
//! asks for a configuration reload instead of killing the process.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::{mpsc, Notify};

use crate::event_loop::DaemonEvent;

/// Exit status of a forced exit: 128 + SIGINT, the same status as
/// `coppa tx`'s second Ctrl-C (COP-86).
pub const FORCED_EXIT_STATUS: i32 = 130;

/// How long a forced exit may take before the process exits regardless.
pub const FORCED_EXIT_GRACE: Duration = Duration::from_secs(2);

/// What a stop request asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopAction {
    /// The first request: the orderly shutdown.
    Graceful,
    /// Any later request: release PTT and exit now.
    Force,
}

/// Counts stop requests: the first is graceful, every later one forces.
#[derive(Debug, Default)]
pub struct StopRequests {
    count: u32,
}

impl StopRequests {
    /// Record one stop request and say what it asks for.
    pub fn record(&mut self) -> StopAction {
        self.count = self.count.saturating_add(1);
        if self.count == 1 {
            StopAction::Graceful
        } else {
            StopAction::Force
        }
    }

    /// Whether a stop request has been received.
    pub fn stopping(&self) -> bool {
        self.count > 0
    }
}

/// The signal or console event behind a stop request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// The console events are constructed on Windows only.
#[cfg_attr(not(windows), allow(dead_code))]
pub enum StopSignal {
    /// SIGINT, or Ctrl-C in a Windows console.
    Interrupt,
    /// SIGTERM (Unix).
    Terminate,
    /// Ctrl-Break (Windows).
    CtrlBreak,
    /// The console window is closing (Windows).
    ConsoleClose,
    /// Windows is shutting down.
    SystemShutdown,
}

impl StopSignal {
    /// Short name used in the forced-exit line.
    pub fn name(self) -> &'static str {
        match self {
            Self::Interrupt => "SIGINT",
            Self::Terminate => "SIGTERM",
            Self::CtrlBreak => "Ctrl-Break",
            Self::ConsoleClose => "a console close",
            Self::SystemShutdown => "a Windows shutdown",
        }
    }

    /// The line logged when this signal starts the orderly shutdown.
    pub fn stop_message(self) -> String {
        match self {
            Self::ConsoleClose => "Console window closing, shutting down".to_string(),
            Self::SystemShutdown => "Windows is shutting down; shutting down".to_string(),
            _ => format!("Received {}, shutting down", self.name()),
        }
    }
}

/// One received signal.
enum Received {
    Stop(StopSignal),
    Reload,
}

/// The daemon's registered signal handlers.
pub struct Signals {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    hangup: tokio::signal::unix::Signal,
    #[cfg(windows)]
    ctrl_c: tokio::signal::windows::CtrlC,
    #[cfg(windows)]
    ctrl_break: tokio::signal::windows::CtrlBreak,
    #[cfg(windows)]
    ctrl_close: tokio::signal::windows::CtrlClose,
    #[cfg(windows)]
    ctrl_shutdown: tokio::signal::windows::CtrlShutdown,
}

impl Signals {
    /// Install every stop and reload handler. From this call on, these
    /// signals no longer have their default action. Must run inside the
    /// Tokio runtime.
    pub fn register() -> Result<Self> {
        #[cfg(unix)]
        {
            use anyhow::Context;
            use tokio::signal::unix::{signal, SignalKind};
            Ok(Self {
                interrupt: signal(SignalKind::interrupt())
                    .context("failed to register the SIGINT handler")?,
                terminate: signal(SignalKind::terminate())
                    .context("failed to register the SIGTERM handler")?,
                hangup: signal(SignalKind::hangup())
                    .context("failed to register the SIGHUP handler")?,
            })
        }
        #[cfg(windows)]
        {
            use anyhow::Context;
            use tokio::signal::windows;
            Ok(Self {
                ctrl_c: windows::ctrl_c().context("failed to register the Ctrl-C handler")?,
                ctrl_break: windows::ctrl_break()
                    .context("failed to register the Ctrl-Break handler")?,
                ctrl_close: windows::ctrl_close()
                    .context("failed to register the console close handler")?,
                ctrl_shutdown: windows::ctrl_shutdown()
                    .context("failed to register the Windows shutdown handler")?,
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            Ok(Self {})
        }
    }

    /// The next signal, or `None` once no handler can deliver one again.
    async fn next(&mut self) -> Option<Received> {
        #[cfg(unix)]
        {
            tokio::select! {
                r = self.interrupt.recv() => r.map(|()| Received::Stop(StopSignal::Interrupt)),
                r = self.terminate.recv() => r.map(|()| Received::Stop(StopSignal::Terminate)),
                r = self.hangup.recv() => r.map(|()| Received::Reload),
            }
        }
        #[cfg(windows)]
        {
            tokio::select! {
                r = self.ctrl_c.recv() => r.map(|()| Received::Stop(StopSignal::Interrupt)),
                r = self.ctrl_break.recv() => r.map(|()| Received::Stop(StopSignal::CtrlBreak)),
                r = self.ctrl_close.recv() => r.map(|()| Received::Stop(StopSignal::ConsoleClose)),
                r = self.ctrl_shutdown.recv() => {
                    r.map(|()| Received::Stop(StopSignal::SystemShutdown))
                }
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            std::future::pending::<Option<Received>>().await
        }
    }

    /// Handle signals until the process exits. The first stop request sends
    /// [`DaemonEvent::Shutdown`]; a later one wakes `force` (the caller
    /// releases PTT and exits) and, if the process is still alive after
    /// [`FORCED_EXIT_GRACE`], exits [`FORCED_EXIT_STATUS`] from here. A
    /// SIGHUP calls `reload`, unless a stop request came first.
    pub async fn run<R>(
        mut self,
        event_tx: mpsc::Sender<DaemonEvent>,
        force: Arc<Notify>,
        mut reload: R,
    ) where
        R: FnMut() + Send,
    {
        let mut stops = StopRequests::default();
        while let Some(received) = self.next().await {
            match received {
                Received::Reload if stops.stopping() => {
                    tracing::info!("Received SIGHUP during shutdown; not reloading");
                }
                Received::Reload => {
                    tracing::info!("Received SIGHUP, reloading configuration");
                    reload();
                }
                Received::Stop(signal) => match stops.record() {
                    StopAction::Graceful => {
                        tracing::info!("{}", signal.stop_message());
                        // Sent from its own task, so a full event channel
                        // never keeps this loop from seeing a second signal.
                        let tx = event_tx.clone();
                        tokio::spawn(async move {
                            let _ = tx.send(DaemonEvent::Shutdown).await;
                        });
                    }
                    StopAction::Force => {
                        tracing::warn!("Received {} again; forcing exit", signal.name());
                        force.notify_one();
                        tokio::time::sleep(FORCED_EXIT_GRACE).await;
                        tracing::error!(
                            "Shutdown did not finish within 2 s of the second stop signal; \
                             exiting now. The PTT guardian, if running, releases rigctld or \
                             GPIO PTT. Serial PTT depends on the operating system closing \
                             the port."
                        );
                        std::process::exit(FORCED_EXIT_STATUS);
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_stop_request_is_graceful_and_every_later_one_forces() {
        let mut stops = StopRequests::default();
        assert!(!stops.stopping());
        assert_eq!(stops.record(), StopAction::Graceful);
        assert!(stops.stopping());
        assert_eq!(stops.record(), StopAction::Force);
        assert_eq!(stops.record(), StopAction::Force);
    }

    #[test]
    fn stop_messages_keep_the_existing_unix_lines() {
        assert_eq!(
            StopSignal::Interrupt.stop_message(),
            "Received SIGINT, shutting down"
        );
        assert_eq!(
            StopSignal::Terminate.stop_message(),
            "Received SIGTERM, shutting down"
        );
        assert_eq!(
            StopSignal::CtrlBreak.stop_message(),
            "Received Ctrl-Break, shutting down"
        );
        assert_eq!(
            StopSignal::ConsoleClose.stop_message(),
            "Console window closing, shutting down"
        );
        assert_eq!(
            StopSignal::SystemShutdown.stop_message(),
            "Windows is shutting down; shutting down"
        );
    }

    #[test]
    fn forced_exit_status_matches_the_cli_second_interrupt() {
        assert_eq!(FORCED_EXIT_STATUS, 130);
        assert_eq!(FORCED_EXIT_GRACE, Duration::from_secs(2));
    }
}
