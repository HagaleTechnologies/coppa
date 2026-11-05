//! Log output decisions shared by `coppad` and the `coppa` CLI.

use std::ffi::OsStr;

/// Whether log output should carry ANSI colour codes: only when the stream is
/// a terminal and `NO_COLOR` is unset or empty (<https://no-color.org>).
///
/// `tracing-subscriber` colours whenever `NO_COLOR` is unset, terminal or
/// not, and `with_ansi(true)` overrides `NO_COLOR`, so neither check is
/// enough alone.
pub fn ansi_enabled(is_terminal: bool, no_color: Option<&OsStr>) -> bool {
    is_terminal && no_color.is_none_or(OsStr::is_empty)
}

/// COP-85: a log line format that writes `[NAME] ` before each line of the
/// wrapped format `F`, so two `coppad --instance` daemons sharing a terminal
/// or journal can be told apart. The prefix is plain text, never coloured.
pub struct InstanceFormat<F> {
    prefix: String,
    inner: F,
}

impl<F> InstanceFormat<F> {
    /// Prefix every line `inner` formats with `[instance] `.
    pub fn new(instance: &str, inner: F) -> Self {
        Self {
            prefix: format!("[{instance}] "),
            inner,
        }
    }
}

impl<S, N, F> tracing_subscriber::fmt::FormatEvent<S, N> for InstanceFormat<F>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::FormatFields<'a> + 'static,
    F: tracing_subscriber::fmt::FormatEvent<S, N>,
{
    fn format_event(
        &self,
        ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: tracing_subscriber::fmt::format::Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        writer.write_str(&self.prefix)?;
        self.inner.format_event(ctx, writer, event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_only_on_a_terminal_without_no_color() {
        let one = Some(OsStr::new("1"));
        assert!(ansi_enabled(true, None), "terminal, NO_COLOR unset");
        assert!(
            ansi_enabled(true, Some(OsStr::new(""))),
            "terminal, NO_COLOR empty"
        );
        assert!(!ansi_enabled(true, one), "terminal, NO_COLOR=1");
        assert!(!ansi_enabled(false, None), "not a terminal, NO_COLOR unset");
        assert!(!ansi_enabled(false, one), "not a terminal, NO_COLOR=1");
    }

    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Buffer {
        fn lines(&self) -> Vec<String> {
            String::from_utf8(self.0.lock().unwrap().clone())
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect()
        }
    }

    /// Two events through a subscriber built as `init_tracing` builds one.
    fn log_two_lines(instance: Option<&str>, ansi: bool) -> Vec<String> {
        let buffer = Buffer::default();
        let writer = buffer.clone();
        let builder = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(ansi);
        let dispatch = match instance {
            Some(name) => tracing::Dispatch::new(
                builder
                    .event_format(InstanceFormat::new(
                        name,
                        tracing_subscriber::fmt::format().with_ansi(ansi),
                    ))
                    .finish(),
            ),
            None => tracing::Dispatch::new(builder.finish()),
        };
        tracing::dispatcher::with_default(&dispatch, || {
            tracing::info!("first");
            tracing::info!(port = 8300, "second");
        });
        buffer.lines()
    }

    #[test]
    fn an_instance_prefixes_every_line() {
        let lines = log_two_lines(Some("hf"), false);
        assert_eq!(lines.len(), 2, "{lines:?}");
        for line in &lines {
            assert!(line.starts_with("[hf] "), "{line}");
            assert!(line.contains("INFO"), "{line}");
        }
        assert!(lines[1].contains("second port=8300"), "{lines:?}");
    }

    #[test]
    fn no_instance_leaves_the_line_unchanged() {
        let lines = log_two_lines(None, false);
        assert_eq!(lines.len(), 2, "{lines:?}");
        for line in &lines {
            assert!(
                line.starts_with(|c: char| c.is_ascii_digit()),
                "a line starts with its timestamp: {line}"
            );
        }
    }

    #[test]
    fn the_prefix_carries_no_escape_codes_with_ansi_off() {
        for line in log_two_lines(Some("vhf-2"), false) {
            assert!(line.starts_with("[vhf-2] "), "{line}");
            assert!(!line.contains('\u{1b}'), "{line:?}");
        }
    }
}
