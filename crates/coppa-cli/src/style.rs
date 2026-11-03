//! Terminal colour for the CLI's own output (COP-86, H-082): `PASS`/`FAIL`/
//! `SKIP` in `doctor`, and the `WARNING:` and `Error:` prefixes. Colour is on
//! only when the stream is a terminal and `NO_COLOR` is unset or empty, the
//! same rule the log lines follow (`coppa_daemon::logging::ansi_enabled`).

use std::fmt::Display;
use std::io::IsTerminal;

/// A colour for [`paint`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Green,
    Red,
    Yellow,
    BoldRed,
}

impl Tone {
    fn sgr(self) -> &'static str {
        match self {
            Self::Green => "32",
            Self::Red => "31",
            Self::Yellow => "33",
            Self::BoldRed => "1;31",
        }
    }
}

/// `text` wrapped in the SGR codes for `tone`, or unchanged when disabled.
pub(crate) fn paint(text: &str, tone: Tone, enabled: bool) -> String {
    if enabled {
        format!("\u{1b}[{}m{text}\u{1b}[0m", tone.sgr())
    } else {
        text.to_owned()
    }
}

fn color_for(is_terminal: bool) -> bool {
    coppa_daemon::logging::ansi_enabled(is_terminal, std::env::var_os("NO_COLOR").as_deref())
}

/// Whether stdout may carry colour.
pub(crate) fn stdout_color() -> bool {
    color_for(std::io::stdout().is_terminal())
}

/// Whether stderr may carry colour.
pub(crate) fn stderr_color() -> bool {
    color_for(std::io::stderr().is_terminal())
}

/// Print `WARNING: {message}` on stderr, the prefix yellow on a terminal.
pub(crate) fn warn(message: impl Display) {
    eprintln!(
        "{} {message}",
        paint("WARNING:", Tone::Yellow, stderr_color())
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_is_plain_when_disabled() {
        for tone in [Tone::Green, Tone::Red, Tone::Yellow, Tone::BoldRed] {
            assert_eq!(paint("PASS", tone, false), "PASS");
        }
    }

    #[test]
    fn paint_wraps_with_sgr_when_enabled() {
        assert_eq!(paint("PASS", Tone::Green, true), "\u{1b}[32mPASS\u{1b}[0m");
        assert_eq!(paint("FAIL", Tone::Red, true), "\u{1b}[31mFAIL\u{1b}[0m");
        assert_eq!(paint("SKIP", Tone::Yellow, true), "\u{1b}[33mSKIP\u{1b}[0m");
        assert_eq!(
            paint("Error", Tone::BoldRed, true),
            "\u{1b}[1;31mError\u{1b}[0m"
        );
    }
}
