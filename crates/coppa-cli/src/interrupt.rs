//! Ctrl-C and SIGTERM for `coppa rx` (COP-86, H-076): the first signal sets a
//! flag the decode loop polls, so the trailing flush, `source.stop()` and the
//! summary still run; a second one exits at once with status 130, so an
//! operator is never stuck behind a stalled device or stdin producer.
//!
//! Installed only by the commands that poll it, so every other subcommand
//! keeps the default Ctrl-C behaviour.

// Without an audio backend `coppa rx` decodes nothing and never polls it.
#![cfg_attr(
    not(any(feature = "file-backend", feature = "cpal-backend")),
    allow(dead_code)
)]

use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Printed when a second signal arrives before the first was handled.
const INTERRUPTED_AGAIN: &str = "Interrupted again; exiting without cleanup.";

/// A process-wide "stop now" flag set by SIGINT or SIGTERM (Ctrl-C on Windows).
#[derive(Debug)]
pub(crate) struct Interrupt {
    flag: Arc<AtomicBool>,
}

impl Interrupt {
    /// Start listening for signals. Returns once the handlers are
    /// registered, so there is no window where Ctrl-C still kills the process
    /// without cleanup.
    pub(crate) fn install() -> Result<Self> {
        let flag = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<std::io::Result<()>>();
        let thread_flag = Arc::clone(&flag);
        std::thread::Builder::new()
            .name("coppa-signals".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                runtime.block_on(listen(thread_flag, ready_tx));
            })
            .context("starting the signal thread")?;
        ready_rx
            .recv()
            .context("the signal thread stopped before it was ready")?
            .context("registering the Ctrl-C handler")?;
        Ok(Self { flag })
    }

    /// A signal has arrived.
    pub(crate) fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// An `Interrupt` no signal sets, and the flag a test sets instead.
    #[cfg(test)]
    pub(crate) fn manual() -> (Self, Arc<AtomicBool>) {
        let flag = Arc::new(AtomicBool::new(false));
        (
            Self {
                flag: Arc::clone(&flag),
            },
            flag,
        )
    }
}

/// A live transmission stopped by Ctrl-C after PTT was released and the
/// audio stopped; `main` reports it and exits 130. When cleanup fails, the
/// composed `interrupted; cleanup: ...` message is returned instead.
#[derive(Debug)]
pub(crate) struct Interrupted;

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("interrupted")
    }
}

impl std::error::Error for Interrupted {}

/// Record the first signal in `flag`; exit 130 on the next one.
fn on_signal(flag: &AtomicBool) {
    if flag.swap(true, Ordering::SeqCst) {
        eprintln!("{INTERRUPTED_AGAIN}");
        std::process::exit(130);
    }
}

#[cfg(unix)]
async fn listen(flag: Arc<AtomicBool>, ready: std::sync::mpsc::Sender<std::io::Result<()>>) {
    use tokio::signal::unix::{signal, SignalKind};
    let registered = signal(SignalKind::interrupt())
        .and_then(|interrupt| Ok((interrupt, signal(SignalKind::terminate())?)));
    let (mut interrupt, mut terminate) = match registered {
        Ok(streams) => streams,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    loop {
        tokio::select! {
            _ = interrupt.recv() => on_signal(&flag),
            _ = terminate.recv() => on_signal(&flag),
        }
    }
}

#[cfg(windows)]
async fn listen(flag: Arc<AtomicBool>, ready: std::sync::mpsc::Sender<std::io::Result<()>>) {
    let mut ctrl_c = match tokio::signal::windows::ctrl_c() {
        Ok(stream) => stream,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    while ctrl_c.recv().await.is_some() {
        on_signal(&flag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_interrupt_reports_its_flag() {
        let (interrupt, flag) = Interrupt::manual();
        assert!(!interrupt.is_set());
        flag.store(true, Ordering::SeqCst);
        assert!(interrupt.is_set());
    }

    #[test]
    fn first_signal_only_sets_the_flag() {
        let flag = AtomicBool::new(false);
        on_signal(&flag);
        assert!(flag.load(Ordering::SeqCst));
    }
}
