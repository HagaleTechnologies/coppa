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
}
