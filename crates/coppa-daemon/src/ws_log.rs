//! Opt-in WebSocket diagnostics, carried only on the disposable telemetry path.

use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::broadcast;
use tracing::{field::Visit, Event, Level, Subscriber};
use tracing_subscriber::{layer::Context, Layer};

/// Wall-clock timestamp shared by the daemon's diagnostic events.
pub(crate) fn timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// A tracing layer whose sender can be attached after host ports are bound.
#[derive(Clone, Default)]
pub struct WsLogLayer {
    sender: Arc<Mutex<Option<broadcast::Sender<String>>>>,
}

impl WsLogLayer {
    /// The daemon's logging subscriber and WebSocket startup share this instance.
    pub fn global() -> &'static Self {
        static LAYER: OnceLock<WsLogLayer> = OnceLock::new();
        LAYER.get_or_init(Self::default)
    }

    /// Attach the server's telemetry sender without waiting for any client.
    pub fn set_sender(&self, sender: broadcast::Sender<String>) {
        *self.sender.lock().expect("WebSocket log sender lock") = Some(sender);
    }
}

#[derive(Default)]
struct MessageVisitor(String);
impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl<S: Subscriber> Layer<S> for WsLogLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        if *event.metadata().level() > Level::INFO {
            return;
        }
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let mut message = format!("{}: {}", event.metadata().target(), visitor.0);
        let mut limit = message.len().min(2048);
        while !message.is_char_boundary(limit) {
            limit -= 1;
        }
        message.truncate(limit);
        let msg = coppa_host::websocket::WsServerMessage::Log {
            level: event.metadata().level().as_str().to_ascii_lowercase(),
            message,
            timestamp_ms: timestamp_ms(),
        };
        if let Ok(json) = serde_json::to_string(&msg) {
            if let Some(sender) = self
                .sender
                .lock()
                .expect("WebSocket log sender lock")
                .as_ref()
            {
                let _ = sender.send(json);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::prelude::*;

    #[test]
    fn log_opt_in_receives_info_lines() {
        let layer = WsLogLayer::default();
        let (sender, mut receiver) = broadcast::channel(16);
        layer.set_sender(sender);
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("frame decoded");
            tracing::debug!("excluded");
        });
        let value: serde_json::Value = serde_json::from_str(&receiver.try_recv().unwrap()).unwrap();
        println!("{value}");
        assert_eq!(value["type"], "log");
        assert_eq!(value["level"], "info");
        assert!(value["message"].as_str().unwrap().contains("frame decoded"));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn log_lines_truncate_at_2kib() {
        let layer = WsLogLayer::default();
        let (sender, mut receiver) = broadcast::channel(16);
        layer.set_sender(sender);
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
            tracing::warn!("{}", "é".repeat(3000));
        });
        let value: serde_json::Value = serde_json::from_str(&receiver.try_recv().unwrap()).unwrap();
        assert!(value["message"].as_str().unwrap().len() <= 2048);
    }
}
