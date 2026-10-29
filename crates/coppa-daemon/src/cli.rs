//! Command-line interface for `coppad`.
//!
//! Parsing, mode selection and log-filter selection are pure so they can be
//! tested without spawning the daemon; `main.rs` performs the dispatch.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use tracing_subscriber::EnvFilter;

// COP-49: the implicit search order, `config::config_candidates`. Clap is
// built without `wrap_help`, so these lines print as written; keep each under
// 90 columns.
#[cfg(not(windows))]
const AFTER_HELP: &str = "\
Without CONFIG or --config, use the first file found of ./coppad.toml,
$XDG_CONFIG_HOME/coppa/coppad.toml (default ~/.config/coppa/coppad.toml) and
/etc/coppa/coppad.toml; otherwise use built-in defaults.
Explicit config paths must exist. --check does not verify hardware or runtime settings.";

#[cfg(windows)]
const AFTER_HELP: &str = "\
Without CONFIG or --config, use the first file found of ./coppad.toml and
%APPDATA%\\coppa\\coppad.toml; otherwise use built-in defaults.
Explicit config paths must exist. --check does not verify hardware or runtime settings.";

// No doc comment here: clap would turn it into the `--help` long description.
#[derive(Debug, Parser)]
#[command(
    name = "coppad",
    version,
    about = "Coppa digital modem daemon",
    long_about = None,
    after_help = AFTER_HELP
)]
pub struct Cli {
    /// Configuration file; legacy alternative to --config
    #[arg(value_name = "CONFIG", conflicts_with = "config")]
    pub config_path: Option<PathBuf>,

    /// Load configuration from PATH
    #[arg(short = 'c', long = "config", value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Check TOML syntax and field types, then exit
    #[arg(long, conflicts_with = "print_default_config")]
    pub check: bool,

    /// Override RUST_LOG
    #[arg(long, value_name = "LEVEL", value_enum)]
    pub log_level: Option<LogLevel>,

    /// Print built-in configuration as TOML, then exit
    #[arg(long, conflicts_with_all = ["config_path", "config"])]
    pub print_default_config: bool,

    /// Run KISS TNC mode with its own defaults; requires kiss-tnc
    #[arg(
        long,
        conflicts_with_all = ["config_path", "config", "check", "print_default_config"]
    )]
    pub tnc: bool,

    // COP-31: internal. The daemon re-spawns itself with this flag to run the
    // PTT guardian (see `ptt_guardian`); hidden from `--help`. Plain comments,
    // not doc comments, so nothing leaks into the help text.
    #[arg(
        long,
        hide = true,
        value_name = "PTT_METHOD",
        conflicts_with_all = ["config_path", "config", "check", "print_default_config", "tnc"]
    )]
    pub ptt_guardian: Option<String>,

    // COP-31: internal. The rigctld address the guardian releases PTT through.
    #[arg(long, hide = true, value_name = "ADDR", requires = "ptt_guardian")]
    pub guardian_rigctld_address: Option<String>,
}

// Variants carry no doc comments so `--help` keeps the possible values inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    fn directive(self) -> &'static str {
        match self {
            LogLevel::Off => "off",
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

/// Where the daemon configuration comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    /// A path named on the command line; it must exist.
    Explicit(PathBuf),
    /// Nothing named: the first file found of `./coppad.toml`, the user
    /// config dir and `/etc/coppa/coppad.toml` (COP-49, see
    /// `config::config_candidates`), otherwise built-in defaults.
    Implicit,
}

/// What this invocation does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Print the built-in configuration as TOML and exit.
    PrintDefaultConfig,
    /// Read and deserialize the configuration, report the result and exit.
    Check(ConfigSource),
    /// Run the KISS TNC with its own defaults.
    Tnc,
    /// Run the daemon.
    Run(ConfigSource),
    /// Internal (COP-31): watch a running daemon's PTT reports on stdin and
    /// release PTT if it exits keyed.
    PttGuardian {
        ptt_method: String,
        rigctld_address: String,
    },
}

impl Cli {
    /// The configuration this invocation names, if any.
    pub fn config_source(&self) -> ConfigSource {
        match self.config.as_ref().or(self.config_path.as_ref()) {
            Some(path) => ConfigSource::Explicit(path.clone()),
            None => ConfigSource::Implicit,
        }
    }

    /// Select the mode. Clap has already rejected conflicting combinations.
    pub fn mode(&self) -> Mode {
        if let Some(ptt_method) = &self.ptt_guardian {
            Mode::PttGuardian {
                ptt_method: ptt_method.clone(),
                rigctld_address: self
                    .guardian_rigctld_address
                    .clone()
                    .unwrap_or_else(|| crate::config::RadioConfig::default().rigctld_address),
            }
        } else if self.print_default_config {
            Mode::PrintDefaultConfig
        } else if self.check {
            Mode::Check(self.config_source())
        } else if self.tnc {
            Mode::Tnc
        } else {
            Mode::Run(self.config_source())
        }
    }
}

/// Choose the tracing filter: `--log-level` wins; otherwise `rust_log` (the
/// `RUST_LOG` value) if it parses, falling back to `info`.
pub fn log_filter(level: Option<LogLevel>, rust_log: Option<&str>) -> EnvFilter {
    if let Some(level) = level {
        return EnvFilter::new(level.directive());
    }
    rust_log
        .and_then(|value| EnvFilter::try_new(value).ok())
        .unwrap_or_else(|| EnvFilter::new("info"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;
    use tracing_subscriber::filter::LevelFilter;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("coppad").chain(args.iter().copied()))
    }

    fn mode(args: &[&str]) -> Mode {
        parse(args).expect("valid arguments").mode()
    }

    fn explicit(path: &str) -> ConfigSource {
        ConfigSource::Explicit(PathBuf::from(path))
    }

    fn error_kind(args: &[&str]) -> ErrorKind {
        parse(args).expect_err("arguments must be rejected").kind()
    }

    #[test]
    fn parser_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn no_arguments_runs_with_the_implicit_config() {
        assert_eq!(mode(&[]), Mode::Run(ConfigSource::Implicit));
    }

    #[test]
    fn both_explicit_forms_select_the_named_file() {
        assert_eq!(mode(&["station.toml"]), Mode::Run(explicit("station.toml")));
        assert_eq!(
            mode(&["--config", "station.toml"]),
            Mode::Run(explicit("station.toml"))
        );
        assert_eq!(
            mode(&["-c", "station.toml"]),
            Mode::Run(explicit("station.toml"))
        );
        assert_eq!(
            mode(&["--config=coppad.toml"]),
            Mode::Run(explicit("coppad.toml"))
        );
    }

    #[test]
    fn check_selects_the_same_config_as_a_run() {
        assert_eq!(mode(&["--check"]), Mode::Check(ConfigSource::Implicit));
        assert_eq!(
            mode(&["station.toml", "--check"]),
            Mode::Check(explicit("station.toml"))
        );
        assert_eq!(
            mode(&[
                "--check",
                "--config",
                "station.toml",
                "--log-level",
                "debug"
            ]),
            Mode::Check(explicit("station.toml"))
        );
    }

    #[test]
    fn print_default_config_and_tnc_select_their_modes() {
        assert_eq!(mode(&["--print-default-config"]), Mode::PrintDefaultConfig);
        assert_eq!(
            mode(&["--print-default-config", "--log-level", "trace"]),
            Mode::PrintDefaultConfig
        );
        assert_eq!(mode(&["--tnc"]), Mode::Tnc);
        assert_eq!(mode(&["--tnc", "--log-level", "warn"]), Mode::Tnc);
    }

    #[test]
    fn ptt_guardian_mode_parses() {
        assert_eq!(
            mode(&[
                "--ptt-guardian",
                "rigctld",
                "--guardian-rigctld-address",
                "127.0.0.1:1"
            ]),
            Mode::PttGuardian {
                ptt_method: "rigctld".to_string(),
                rigctld_address: "127.0.0.1:1".to_string(),
            }
        );
        assert_eq!(
            mode(&["--ptt-guardian", "gpio:17", "--log-level", "debug"]),
            Mode::PttGuardian {
                ptt_method: "gpio:17".to_string(),
                rigctld_address: crate::config::RadioConfig::default().rigctld_address,
            },
            "the address falls back to the [radio] default"
        );
    }

    #[test]
    fn ptt_guardian_conflicts_with_run_modes() {
        for args in [
            &["--ptt-guardian", "rigctld", "--config", "station.toml"][..],
            &["--ptt-guardian", "rigctld", "station.toml"],
            &["--ptt-guardian", "rigctld", "--check"],
            &["--ptt-guardian", "rigctld", "--tnc"],
            &["--ptt-guardian", "rigctld", "--print-default-config"],
        ] {
            assert_eq!(error_kind(args), ErrorKind::ArgumentConflict, "{args:?}");
        }
    }

    #[test]
    fn guardian_address_requires_guardian_mode() {
        assert_eq!(
            error_kind(&["--guardian-rigctld-address", "127.0.0.1:1"]),
            ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn log_level_accepts_exactly_the_six_levels() {
        for (value, level) in [
            ("off", LogLevel::Off),
            ("error", LogLevel::Error),
            ("warn", LogLevel::Warn),
            ("info", LogLevel::Info),
            ("debug", LogLevel::Debug),
            ("trace", LogLevel::Trace),
        ] {
            let cli = parse(&["--log-level", value]).expect(value);
            assert_eq!(cli.log_level, Some(level));
            assert_eq!(level.directive(), value);
        }
        for value in ["loud", "INFO2", "coppa_daemon=debug", ""] {
            assert_eq!(
                error_kind(&["--log-level", value]),
                ErrorKind::InvalidValue,
                "{value:?}"
            );
        }
    }

    #[test]
    fn conflicting_combinations_are_rejected() {
        for args in [
            &["station.toml", "--config", "other.toml"][..],
            &["--check", "--print-default-config"],
            &["--print-default-config", "--config", "station.toml"],
            &["--print-default-config", "station.toml"],
            &["--tnc", "--config", "station.toml"],
            &["--tnc", "station.toml"],
            &["--tnc", "--check"],
            &["--tnc", "--print-default-config"],
        ] {
            assert_eq!(error_kind(args), ErrorKind::ArgumentConflict, "{args:?}");
        }
    }

    #[test]
    fn usage_errors_are_rejected() {
        assert_eq!(error_kind(&["--wat"]), ErrorKind::UnknownArgument);
        assert_eq!(
            error_kind(&["a.toml", "b.toml"]),
            ErrorKind::UnknownArgument
        );
        assert_eq!(
            error_kind(&["--config"]),
            ErrorKind::InvalidValue,
            "missing --config value"
        );
        assert_eq!(
            error_kind(&["--log-level"]),
            ErrorKind::InvalidValue,
            "missing --log-level value"
        );
    }

    #[test]
    fn help_and_version_win_over_other_arguments() {
        assert_eq!(error_kind(&["--help"]), ErrorKind::DisplayHelp);
        assert_eq!(error_kind(&["-h"]), ErrorKind::DisplayHelp);
        assert_eq!(error_kind(&["--tnc", "--help"]), ErrorKind::DisplayHelp);
        assert_eq!(error_kind(&["--version"]), ErrorKind::DisplayVersion);
        assert_eq!(error_kind(&["-V"]), ErrorKind::DisplayVersion);
        assert_eq!(
            error_kind(&["--tnc", "--version"]),
            ErrorKind::DisplayVersion
        );
    }

    #[test]
    fn version_comes_from_the_package() {
        use clap::CommandFactory;
        assert_eq!(
            Cli::command().render_version(),
            format!("coppad {}\n", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn help_lists_every_option() {
        use clap::CommandFactory;
        let help = Cli::command().render_help().to_string();
        for flag in [
            "--config <PATH>",
            "--check",
            "--log-level <LEVEL>",
            "--print-default-config",
            "--tnc",
            "--version",
            "--help",
            "[CONFIG]",
        ] {
            assert!(help.contains(flag), "help lacks {flag}:\n{help}");
        }
    }

    #[test]
    fn cli_log_level_overrides_rust_log() {
        for (level, expected) in [
            (LogLevel::Off, LevelFilter::OFF),
            (LogLevel::Error, LevelFilter::ERROR),
            (LogLevel::Warn, LevelFilter::WARN),
            (LogLevel::Info, LevelFilter::INFO),
            (LogLevel::Debug, LevelFilter::DEBUG),
            (LogLevel::Trace, LevelFilter::TRACE),
        ] {
            for rust_log in [None, Some("trace"), Some("off"), Some("not=valid=")] {
                assert_eq!(
                    log_filter(Some(level), rust_log).max_level_hint(),
                    Some(expected),
                    "{level:?} with RUST_LOG={rust_log:?}"
                );
            }
        }
    }

    #[test]
    fn valid_rust_log_is_kept_without_a_cli_level() {
        assert_eq!(
            log_filter(None, Some("warn")).max_level_hint(),
            Some(LevelFilter::WARN)
        );
        let filter = log_filter(None, Some("coppa_daemon=debug,warn"));
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::DEBUG));
        assert_eq!(filter.to_string(), "coppa_daemon=debug,warn");
    }

    #[test]
    fn absent_or_invalid_rust_log_falls_back_to_info() {
        for rust_log in [None, Some("not=valid="), Some("coppa=loud")] {
            assert_eq!(
                log_filter(None, rust_log).max_level_hint(),
                Some(LevelFilter::INFO),
                "RUST_LOG={rust_log:?}"
            );
        }
    }
}
