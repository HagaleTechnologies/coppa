//! Drive the real `coppad` binary through its command-line interface.
//!
//! Every informational command (`--help`, `--version`, `--check`,
//! `--print-default-config`, argument errors) must exit on its own. Each child
//! runs under a hard deadline; a timeout is a test failure, never a stand-in
//! for the expected exit. Output goes to temporary files rather than pipes, so
//! a full pipe cannot hide the process finishing.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use coppa_daemon::config::DaemonConfig;
// COP-49: the discovery tests drive the Unix search order (HOME,
// XDG_CONFIG_HOME, /etc); Windows searches only the cwd and %APPDATA%.
#[cfg(unix)]
use coppa_daemon::config::{CONFIG_FILE_NAME, SYSTEM_CONFIG_PATH};

/// How long an informational command may take before the test fails.
const DEADLINE: Duration = Duration::from_secs(5);

const CHECK_OK: &str =
    "Configuration syntax and field types OK.\nHardware and runtime settings were not checked.\n";

/// Startup log lines that no informational command may print.
const STARTUP_MARKERS: [&str; 3] = [
    "Coppa Daemon starting",
    "Daemon ready",
    "Event loop started",
];

/// A uniquely named directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "coppad-cli-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Write `contents` to `name`, which may contain subdirectories.
    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().expect("a parent directory"))
            .expect("create parent directory");
        std::fs::write(&path, contents).expect("write temporary file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A `coppad` child whose stdout/stderr go to files in its own capture dir.
struct Daemon {
    child: Child,
    capture: TempDir,
    started: Instant,
    reaped: bool,
}

impl Daemon {
    /// Spawn `coppad` in `cwd`. COP-49: the child's `HOME` is its own empty
    /// capture dir and `XDG_CONFIG_HOME`/`APPDATA` are removed, so the config
    /// search never reaches the real user's files; `envs` override this.
    fn spawn(cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> Self {
        let capture = TempDir::new("capture");
        let stdout = File::create(capture.0.join("stdout")).expect("create stdout capture");
        let stderr = File::create(capture.0.join("stderr")).expect("create stderr capture");
        let mut command = Command::new(env!("CARGO_BIN_EXE_coppad"));
        command
            .args(args)
            .current_dir(cwd)
            .env_remove("RUST_LOG")
            // anyhow appends a backtrace to `main`'s error when either is set.
            .env_remove("RUST_BACKTRACE")
            .env_remove("RUST_LIB_BACKTRACE")
            .env("HOME", capture.path())
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("APPDATA")
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr);
        for (key, value) in envs {
            command.env(key, value);
        }
        let child = command.spawn().expect("failed to spawn coppad");
        Self {
            child,
            capture,
            started: Instant::now(),
            reaped: false,
        }
    }

    fn stdout(&self) -> String {
        std::fs::read_to_string(self.capture.0.join("stdout")).expect("read stdout capture")
    }

    fn stderr(&self) -> String {
        std::fs::read_to_string(self.capture.0.join("stderr")).expect("read stderr capture")
    }

    fn try_exit(&mut self) -> Option<ExitStatus> {
        let status = self.child.try_wait().expect("poll coppad");
        if status.is_some() {
            self.reaped = true;
        }
        status
    }

    /// Kill and reap the child, then return what it printed.
    fn kill(&mut self) -> (String, String) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.reaped = true;
        (self.stdout(), self.stderr())
    }

    /// Wait for the child to exit by itself within [`DEADLINE`].
    fn finish(mut self, args: &[&str]) -> Finished {
        loop {
            if let Some(status) = self.try_exit() {
                return Finished {
                    status,
                    stdout: self.stdout(),
                    stderr: self.stderr(),
                };
            }
            if self.started.elapsed() >= DEADLINE {
                let (stdout, stderr) = self.kill();
                panic!(
                    "coppad {args:?} did not exit within {DEADLINE:?}\n\
                     --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

struct Finished {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl Finished {
    fn assert_code(&self, code: i32, args: &[&str]) {
        assert_eq!(
            self.status.code(),
            Some(code),
            "coppad {args:?} exit status\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.stdout,
            self.stderr
        );
    }

    fn assert_no_startup(&self) {
        for marker in STARTUP_MARKERS {
            assert!(
                !self.stdout.contains(marker) && !self.stderr.contains(marker),
                "unexpected startup output {marker:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
                self.stdout,
                self.stderr
            );
        }
    }
}

fn run_with_env(cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> Finished {
    Daemon::spawn(cwd, args, envs).finish(args)
}

fn run(cwd: &Path, args: &[&str]) -> Finished {
    run_with_env(cwd, args, &[])
}

/// Run an informational command and require exit 0, empty stderr and no
/// startup output. Returns stdout.
fn run_ok(cwd: &Path, args: &[&str]) -> String {
    let out = run(cwd, args);
    out.assert_code(0, args);
    assert_eq!(out.stderr, "", "coppad {args:?} stderr");
    out.assert_no_startup();
    out.stdout
}

/// Run a command that must fail with exit 1, empty stdout, and an error on
/// stderr. Returns stderr.
fn run_config_error(cwd: &Path, args: &[&str]) -> String {
    run_config_error_with_env(cwd, args, &[])
}

/// [`run_config_error`] with extra environment variables.
fn run_config_error_with_env(cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> String {
    let out = run_with_env(cwd, args, envs);
    out.assert_code(1, args);
    assert_eq!(out.stdout, "", "coppad {args:?} stdout");
    out.assert_no_startup();
    assert!(out.stderr.starts_with("Error: "), "{}", out.stderr);
    out.stderr
}

/// Run a command that clap must reject as a usage error (exit 2).
fn run_usage_error(cwd: &Path, args: &[&str]) -> String {
    let out = run(cwd, args);
    out.assert_code(2, args);
    assert_eq!(out.stdout, "", "coppad {args:?} stdout");
    out.assert_no_startup();
    assert!(out.stderr.starts_with("error: "), "{}", out.stderr);
    assert!(
        out.stderr
            .ends_with("For more information, try '--help'.\n"),
        "{}",
        out.stderr
    );
    out.stderr
}

/// A working directory whose `coppad.toml` would fail to parse, proving a
/// command never read it.
fn dir_with_malformed_default() -> TempDir {
    let dir = TempDir::new("malformed");
    dir.write("coppad.toml", "[audio\nsample_rate = = 1\n");
    dir
}

fn version_line() -> String {
    format!("coppad {}\n", env!("CARGO_PKG_VERSION"))
}

/// COP-49: `HOME`'s config file, `$HOME/.config/coppa/coppad.toml`.
#[cfg(unix)]
fn home_config_path(home: &Path) -> PathBuf {
    home.join(".config").join("coppa").join(CONFIG_FILE_NAME)
}

/// COP-49: `/etc/coppa/coppad.toml` cannot be redirected, so a test that
/// expects the search to find nothing at all runs only when it is provably
/// absent. Prints why when it is not.
#[cfg(unix)]
fn system_config_absent() -> bool {
    match Path::new(SYSTEM_CONFIG_PATH).symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        other => {
            eprintln!("skipping: {SYSTEM_CONFIG_PATH} is present or unreadable here ({other:?})");
            false
        }
    }
}

// --- help and version -------------------------------------------------------

/// COP-49: the help's lines on the implicit config search, per platform.
#[cfg(not(windows))]
const HELP_SEARCH_ORDER: &str = "\
Without CONFIG or --config, use the first file found of ./coppad.toml,
$XDG_CONFIG_HOME/coppa/coppad.toml (default ~/.config/coppa/coppad.toml) and
/etc/coppa/coppad.toml; otherwise use built-in defaults.
";
#[cfg(windows)]
const HELP_SEARCH_ORDER: &str = "\
Without CONFIG or --config, use the first file found of ./coppad.toml and
%APPDATA%\\coppa\\coppad.toml; otherwise use built-in defaults.
";

fn assert_full_help(help: &str) {
    for expected in [
        "Coppa digital modem daemon",
        "Usage: coppad [OPTIONS] [CONFIG]",
        "[CONFIG]  Configuration file; legacy alternative to --config",
        "-c, --config <PATH>",
        "Load configuration from PATH",
        "--check",
        "Check TOML syntax and field types, then exit",
        "--log-level <LEVEL>",
        "Override RUST_LOG [possible values: off, error, warn, info, debug, trace]",
        "--print-default-config",
        "Print built-in configuration as TOML, then exit",
        "--tnc",
        "Run KISS TNC mode with its own defaults; requires kiss-tnc",
        "-h, --help",
        "Print help",
        "-V, --version",
        "Print version",
        HELP_SEARCH_ORDER,
        "Explicit config paths must exist. --check does not verify hardware or runtime settings.",
    ] {
        assert!(help.contains(expected), "help lacks {expected:?}:\n{help}");
    }
}

#[test]
fn help_prints_usage_and_exits_without_reading_config() {
    let dir = dir_with_malformed_default();
    let help = run_ok(dir.path(), &["--help"]);
    assert_full_help(&help);
}

#[test]
fn short_help_prints_the_same_usage() {
    let dir = dir_with_malformed_default();
    assert_eq!(run_ok(dir.path(), &["-h"]), run_ok(dir.path(), &["--help"]));
}

#[test]
fn tnc_help_prints_the_same_usage() {
    let dir = dir_with_malformed_default();
    assert_eq!(
        run_ok(dir.path(), &["--tnc", "--help"]),
        run_ok(dir.path(), &["--help"])
    );
}

#[test]
fn help_does_not_advertise_the_internal_guardian_mode() {
    let dir = dir_with_malformed_default();
    let help = run_ok(dir.path(), &["--help"]);
    assert!(!help.contains("ptt-guardian"), "{help}");
    assert!(!help.contains("guardian-rigctld-address"), "{help}");
}

#[test]
fn version_prints_package_version_and_exits() {
    let dir = dir_with_malformed_default();
    for args in [&["--version"][..], &["-V"], &["--tnc", "--version"]] {
        assert_eq!(run_ok(dir.path(), args), version_line(), "coppad {args:?}");
    }
}

// --- argument errors --------------------------------------------------------

#[test]
fn unknown_option_is_a_usage_error() {
    let dir = TempDir::new("unknown");
    let stderr = run_usage_error(dir.path(), &["--wat"]);
    // Clap's own diagnostic; between these lines it may add a tip about `--`.
    assert!(
        stderr.starts_with("error: unexpected argument '--wat' found\n"),
        "{stderr}"
    );
    assert!(
        stderr.contains("\nUsage: coppad [OPTIONS] [CONFIG]\n"),
        "{stderr}"
    );
}

#[test]
fn missing_option_value_is_a_usage_error() {
    let dir = TempDir::new("missing-value");
    run_usage_error(dir.path(), &["--config"]);
    run_usage_error(dir.path(), &["--log-level"]);
}

#[test]
fn extra_positional_argument_is_a_usage_error() {
    let dir = TempDir::new("extra");
    dir.write("a.toml", "");
    dir.write("b.toml", "");
    run_usage_error(dir.path(), &["a.toml", "b.toml", "--check"]);
}

#[test]
fn invalid_log_level_is_a_usage_error() {
    let dir = TempDir::new("bad-level");
    for level in ["loud", "INFO2", "coppa_daemon=debug"] {
        let stderr = run_usage_error(dir.path(), &["--check", "--log-level", level]);
        assert!(stderr.contains("invalid value"), "{stderr}");
    }
}

#[test]
fn conflicting_options_are_usage_errors() {
    let dir = TempDir::new("conflicts");
    dir.write("station.toml", "");
    for args in [
        &["station.toml", "--config", "station.toml"][..],
        &["--check", "--print-default-config"],
        &["--print-default-config", "--config", "station.toml"],
        &["--print-default-config", "station.toml"],
        &["--tnc", "--config", "station.toml"],
        &["--tnc", "station.toml"],
        &["--tnc", "--check"],
        &["--tnc", "--print-default-config"],
    ] {
        let stderr = run_usage_error(dir.path(), args);
        assert!(stderr.contains("cannot be used with"), "{args:?}: {stderr}");
    }
}

// --- --check ----------------------------------------------------------------

#[cfg(unix)]
#[test]
fn check_without_a_config_file_checks_the_defaults() {
    if !system_config_absent() {
        return;
    }
    let dir = TempDir::new("check-defaults");
    let home = TempDir::new("check-defaults-home");
    let home_str = home.path().to_str().expect("UTF-8 temp path");
    let args = ["--check"];
    let out = run_with_env(dir.path(), &args, &[("HOME", home_str)]);
    out.assert_code(0, &args);
    assert_eq!(out.stderr, "");
    assert_eq!(
        out.stdout,
        format!(
            "No configuration file found; checking the built-in defaults (searched: \
             coppad.toml, {}, /etc/coppa/coppad.toml).\n{CHECK_OK}",
            home_config_path(home.path()).display()
        )
    );
}

/// What `--check` prints for an implicit source that found `./coppad.toml`.
fn check_ok_using_the_local_file() -> String {
    format!("Using configuration file coppad.toml\n{CHECK_OK}")
}

#[test]
fn check_loads_the_implicit_default_file() {
    let dir = TempDir::new("check-implicit");
    dir.write("coppad.toml", "[audio]\nsample_rate = 48000\n");
    assert_eq!(
        run_ok(dir.path(), &["--check"]),
        check_ok_using_the_local_file()
    );
}

#[cfg(unix)]
#[test]
fn check_reports_which_discovered_file_it_used() {
    // COP-49: the first line names the file the implicit search chose.
    let home = TempDir::new("discover-home");
    let home_file = home.write(".config/coppa/coppad.toml", "");
    let xdg = TempDir::new("discover-xdg");
    let xdg_file = xdg.write("coppa/coppad.toml", "");
    let home_str = home.path().to_str().expect("UTF-8 temp path");
    let xdg_str = xdg.path().to_str().expect("UTF-8 temp path");

    let empty_cwd = TempDir::new("discover-cwd-empty");
    let local_cwd = TempDir::new("discover-cwd-local");
    local_cwd.write("coppad.toml", "");
    // A relative XDG_CONFIG_HOME is ignored even where it would resolve.
    let relative_cwd = TempDir::new("discover-cwd-relative");
    relative_cwd.write("xdg/coppa/coppad.toml", "");

    for (what, cwd, envs, expected) in [
        (
            "only the HOME file",
            &empty_cwd,
            vec![("HOME", home_str)],
            home_file.display().to_string(),
        ),
        (
            "an absolute XDG_CONFIG_HOME and the HOME file",
            &empty_cwd,
            vec![("HOME", home_str), ("XDG_CONFIG_HOME", xdg_str)],
            xdg_file.display().to_string(),
        ),
        (
            "a local file, XDG_CONFIG_HOME and the HOME file",
            &local_cwd,
            vec![("HOME", home_str), ("XDG_CONFIG_HOME", xdg_str)],
            "coppad.toml".to_string(),
        ),
        (
            "a relative XDG_CONFIG_HOME",
            &relative_cwd,
            vec![("HOME", home_str), ("XDG_CONFIG_HOME", "xdg")],
            home_file.display().to_string(),
        ),
    ] {
        let args = ["--check"];
        let out = run_with_env(cwd.path(), &args, &envs);
        out.assert_code(0, &args);
        assert_eq!(out.stderr, "", "{what}");
        assert_eq!(
            out.stdout,
            format!("Using configuration file {expected}\n{CHECK_OK}"),
            "{what}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_broken_discovered_config_stops_check_and_run_naming_it() {
    // COP-49 with COP-36: a discovered file is parsed as strictly as a local
    // one, so a typo in ~/.config/coppa/coppad.toml stops startup naming it.
    let cwd = TempDir::new("discover-broken-cwd");
    let home = TempDir::new("discover-broken-home");
    let broken = home.write(
        ".config/coppa/coppad.toml",
        "[engine]\ncalsign = \"W5AU\"\n",
    );
    let home_str = home.path().to_str().expect("UTF-8 temp path");
    for args in [&["--check"][..], &[]] {
        let stderr = run_config_error_with_env(cwd.path(), args, &[("HOME", home_str)]);
        assert!(
            stderr.starts_with(&format!(
                "Error: Failed to parse config {}",
                broken.display()
            )),
            "coppad {args:?}: {stderr}"
        );
        assert!(
            stderr.contains("unknown field `calsign`"),
            "coppad {args:?}: {stderr}"
        );
    }
}

#[test]
fn check_accepts_an_empty_file() {
    let dir = TempDir::new("check-empty");
    dir.write("station.toml", "");
    assert_eq!(
        run_ok(dir.path(), &["--config", "station.toml", "--check"]),
        CHECK_OK
    );
}

#[test]
fn check_accepts_every_explicit_config_form_and_order() {
    let dir = dir_with_malformed_default();
    dir.write(
        "station.toml",
        "[engine]\nprofile = \"HF_ROBUST\"\ncallsign = \"VK2ABC\"\n",
    );
    for args in [
        &["--config", "station.toml", "--check"][..],
        &["-c", "station.toml", "--check"],
        &["--config=station.toml", "--check"],
        &["station.toml", "--check"],
        &["--check", "station.toml"],
        &[
            "--check",
            "--config",
            "station.toml",
            "--log-level",
            "debug",
        ],
        &["--log-level", "trace", "station.toml", "--check"],
    ] {
        assert_eq!(run_ok(dir.path(), args), CHECK_OK, "coppad {args:?}");
    }
}

#[test]
fn check_with_log_level_off_ignores_rust_log() {
    let dir = TempDir::new("check-quiet");
    dir.write("coppad.toml", "");
    let args = ["--check", "--log-level", "off"];
    let out = run_with_env(dir.path(), &args, &[("RUST_LOG", "trace")]);
    out.assert_code(0, &args);
    assert_eq!(out.stdout, check_ok_using_the_local_file());
    assert_eq!(out.stderr, "");
}

#[test]
fn check_with_rust_log_trace_prints_no_tracing() {
    let dir = TempDir::new("check-trace");
    dir.write("coppad.toml", "");
    let out = run_with_env(dir.path(), &["--check"], &[("RUST_LOG", "trace")]);
    out.assert_code(0, &["--check"]);
    assert_eq!(out.stdout, check_ok_using_the_local_file());
    assert_eq!(out.stderr, "");
}

#[test]
fn check_rejects_malformed_toml_with_the_path() {
    let dir = TempDir::new("check-malformed");
    dir.write("station.toml", "[audio\nsample_rate = 48000\n");
    let stderr = run_config_error(dir.path(), &["--config", "station.toml", "--check"]);
    assert!(
        stderr.starts_with("Error: Failed to load config station.toml"),
        "{stderr}"
    );
    assert!(stderr.contains("Caused by:"), "{stderr}");
}

#[test]
fn check_rejects_a_wrong_field_type_with_the_path() {
    let dir = TempDir::new("check-type");
    dir.write("station.toml", "[audio]\nsample_rate = \"bad\"\n");
    for args in [
        &["--config", "station.toml", "--check"][..],
        &["station.toml", "--check"],
    ] {
        let stderr = run_config_error(dir.path(), args);
        assert!(
            stderr.starts_with("Error: Failed to load config station.toml"),
            "{stderr}"
        );
        assert!(stderr.contains("sample_rate"), "{stderr}");
    }
}

#[test]
fn explicit_missing_config_fails_in_check_and_run_modes() {
    let dir = TempDir::new("missing");
    for args in [
        &["--config", "missing.toml", "--check"][..],
        &["missing.toml", "--check"],
        &["--config", "missing.toml"],
        &["missing.toml"],
        &["-c", "missing.toml", "--log-level", "info"],
    ] {
        let stderr = run_config_error(dir.path(), args);
        assert!(
            stderr.starts_with("Error: Failed to load config missing.toml"),
            "coppad {args:?}: {stderr}"
        );
        assert!(stderr.contains("Caused by:"), "{stderr}");
    }
}

#[test]
fn explicit_coppad_toml_must_exist_too() {
    // Naming the default file explicitly opts into the strict rule.
    let dir = TempDir::new("explicit-default");
    let stderr = run_config_error(dir.path(), &["--config", "coppad.toml", "--check"]);
    assert!(
        stderr.starts_with("Error: Failed to load config coppad.toml"),
        "{stderr}"
    );
}

#[test]
fn invalid_implicit_config_fails_in_check_and_run_modes() {
    let dir = dir_with_malformed_default();
    for args in [&["--check"][..], &[], &["--log-level", "info"]] {
        let stderr = run_config_error(dir.path(), args);
        assert!(stderr.contains("coppad.toml"), "coppad {args:?}: {stderr}");
    }
}

#[test]
fn explicit_config_is_used_instead_of_the_local_default() {
    // A malformed local coppad.toml is ignored when a path is given...
    let dir = dir_with_malformed_default();
    dir.write("station.toml", "");
    for args in [
        &["--config", "station.toml", "--check"][..],
        &["station.toml", "--check"],
    ] {
        assert_eq!(run_ok(dir.path(), args), CHECK_OK, "coppad {args:?}");
    }

    // ...and a valid local coppad.toml does not mask a bad explicit file.
    let dir = TempDir::new("explicit-bad");
    dir.write("coppad.toml", "");
    dir.write("station.toml", "[audio]\nsample_rate = \"bad\"\n");
    for args in [
        &["--config", "station.toml", "--check"][..],
        &["station.toml", "--check"],
    ] {
        let stderr = run_config_error(dir.path(), args);
        assert!(stderr.contains("station.toml"), "coppad {args:?}: {stderr}");
    }
}

#[test]
fn check_only_deserializes_and_never_touches_hardware_or_listeners() {
    // Every value here would fail or block at runtime: an unreachable
    // rigctld, an unknown PTT method, nonexistent audio devices, an unknown
    // profile and enabled listeners. --check reads and deserializes only.
    let dir = TempDir::new("check-runtime");
    dir.write(
        "station.toml",
        r#"
[audio]
input_device = "no such input device"
output_device = "no such output device"
sample_rate = 44100

[radio]
ptt_method = "definitely-not-a-ptt-backend"
rigctld_address = "127.0.0.1:1"

[host]
bind_address = "203.0.113.1"
vara_enabled = true
websocket_enabled = true

[engine]
profile = "NOT_A_PROFILE"
"#,
    );
    assert_eq!(
        run_ok(dir.path(), &["--config", "station.toml", "--check"]),
        CHECK_OK
    );
}

#[test]
fn check_rejects_unknown_keys_naming_them() {
    // COP-36: a misspelled or unknown key or table is a parse error, not a
    // silently ignored line.
    let dir = TempDir::new("check-unknown");
    dir.write(
        "station.toml",
        "top_level_extra = 1\n[engine]\nnot_a_real_key = true\n[mystery]\nvalue = 2\n",
    );
    let stderr = run_config_error(dir.path(), &["station.toml", "--check"]);
    assert!(stderr.contains("station.toml"), "{stderr}");
    assert!(stderr.contains("unknown field `"), "{stderr}");
}

#[test]
fn a_misspelled_key_stops_check_and_run_modes_naming_it() {
    // COP-36's scenario: `calsign` for `callsign` fails to start, naming the
    // bad key, in --check, in run mode, and through the implicit ./coppad.toml.
    let dir = TempDir::new("typo-key");
    dir.write("station.toml", "[engine]\ncalsign = \"W5AU\"\n");
    for args in [
        &["--config", "station.toml", "--check"][..],
        &["station.toml", "--check"],
        &["--config", "station.toml"],
        &["station.toml"],
    ] {
        let stderr = run_config_error(dir.path(), args);
        assert!(
            stderr.starts_with("Error: Failed to load config station.toml"),
            "coppad {args:?}: {stderr}"
        );
        assert!(
            stderr.contains("unknown field `calsign`"),
            "coppad {args:?}: {stderr}"
        );
    }

    let implicit = TempDir::new("typo-key-implicit");
    implicit.write("coppad.toml", "[engine]\ncalsign = \"W5AU\"\n");
    for args in [&["--check"][..], &[]] {
        let stderr = run_config_error(implicit.path(), args);
        assert!(
            stderr.starts_with("Error: Failed to parse config coppad.toml"),
            "coppad {args:?}: {stderr}"
        );
        assert!(
            stderr.contains("unknown field `calsign`"),
            "coppad {args:?}: {stderr}"
        );
    }
}

// --- --print-default-config ---------------------------------------------------

#[test]
fn print_default_config_prints_only_the_default_toml() {
    let dir = dir_with_malformed_default();
    let args = ["--print-default-config"];
    let out = run_with_env(dir.path(), &args, &[("RUST_LOG", "trace")]);
    out.assert_code(0, &args);
    assert_eq!(out.stderr, "");
    out.assert_no_startup();
    let printed = out.stdout;

    assert!(printed.ends_with("\n") && !printed.ends_with("\n\n"));
    for section in ["[audio]", "[radio]", "[host]", "[engine]", "[station_id]"] {
        assert!(printed.contains(section), "missing {section}:\n{printed}");
    }
    for line in [
        "vara_enabled = false",
        "websocket_enabled = false",
        "websocket_allowed_origins = []",
        "rate_loop_probe_interval = 0",
        "cp_gate_enabled = false",
        "cp_negotiation_enabled = false",
        "id_interval_secs = 540",
        "ptt_guardian = true",
    ] {
        assert!(printed.contains(line), "missing {line:?}:\n{printed}");
    }
    assert!(!printed.contains("grid"), "{printed}");
    // COP-53: an unset token is omitted, never printed as a placeholder.
    assert!(!printed.contains("websocket_token"), "{printed}");

    let parsed: DaemonConfig = toml::from_str(&printed).expect("printed TOML parses");
    assert_eq!(
        toml::to_string_pretty(&parsed).unwrap(),
        toml::to_string_pretty(&DaemonConfig::default()).unwrap()
    );
}

#[test]
fn printed_default_config_passes_check() {
    let dir = TempDir::new("print-roundtrip");
    let printed = run_ok(
        dir.path(),
        &["--print-default-config", "--log-level", "debug"],
    );
    dir.write("station.toml", &printed);
    assert_eq!(
        run_ok(dir.path(), &["--config", "station.toml", "--check"]),
        CHECK_OK
    );
}

// --- --tnc --------------------------------------------------------------------

#[cfg(not(feature = "kiss-tnc"))]
#[test]
fn tnc_without_the_feature_fails_without_a_banner() {
    let dir = TempDir::new("tnc");
    for args in [&["--tnc"][..], &["--tnc", "--log-level", "debug"]] {
        let out = run(dir.path(), args);
        out.assert_code(1, args);
        assert_eq!(out.stdout, "");
        assert_eq!(
            out.stderr,
            "Error: TNC mode requires the kiss-tnc feature\n"
        );
    }
}

// --- normal startup ---------------------------------------------------------
//
// These deliberately start the long-running daemon, so they only run in the
// default build, where it opens no audio hardware. The daemon is killed as
// cleanup once it has proven its state.

/// Start the daemon and wait until it logs `Daemon ready`. Returns the child
/// and the time readiness took.
#[cfg(not(feature = "cpal-backend"))]
fn start_until_ready(cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> (Daemon, Duration) {
    start_until(cwd, args, envs, "Daemon ready")
}

/// Start the daemon and wait until its stdout contains `marker`. Returns the
/// child and the time that took.
#[cfg(not(feature = "cpal-backend"))]
fn start_until(
    cwd: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    marker: &str,
) -> (Daemon, Duration) {
    let mut daemon = Daemon::spawn(cwd, args, envs);
    loop {
        if daemon.stdout().contains(marker) {
            let elapsed = daemon.started.elapsed();
            return (daemon, elapsed);
        }
        if let Some(status) = daemon.try_exit() {
            panic!(
                "coppad {args:?} exited with {status} before logging {marker:?}\n\
                 --- stdout ---\n{}\n--- stderr ---\n{}",
                daemon.stdout(),
                daemon.stderr()
            );
        }
        if daemon.started.elapsed() >= DEADLINE {
            let (stdout, stderr) = daemon.kill();
            panic!(
                "coppad {args:?} did not log {marker:?} within {DEADLINE:?}\n\
                 --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(not(feature = "cpal-backend"))]
#[test]
fn explicit_config_starts_the_daemon() {
    let dir = TempDir::new("run-explicit");
    dir.write("station.toml", "[engine]\ncallsign = \"VK2ABC\"\n");
    for args in [&["--config", "station.toml"][..], &["station.toml"]] {
        let (mut daemon, _) = start_until_ready(dir.path(), args, &[]);
        let (stdout, _) = daemon.kill();
        assert!(stdout.contains("Coppa Daemon starting"), "{stdout}");
        // The file was used: its callsign silences the missing-callsign warning.
        assert!(!stdout.contains("No callsign configured"), "{stdout}");
    }
}

#[cfg(all(unix, not(feature = "cpal-backend")))]
#[test]
fn a_config_in_the_user_config_dir_is_used_without_arguments() {
    // COP-49's scenario: ~/.config/coppa/coppad.toml is loaded by a bare
    // `coppad` started from an unrelated working directory.
    let cwd = TempDir::new("run-discover-cwd");
    let home = TempDir::new("run-discover-home");
    let config = home.write(
        ".config/coppa/coppad.toml",
        "[engine]\ncallsign = \"W5AU\"\n",
    );
    let home_str = home.path().to_str().expect("UTF-8 temp path");
    let (mut daemon, _) = start_until_ready(cwd.path(), &[], &[("HOME", home_str)]);
    let (stdout, _) = daemon.kill();
    // The file was used: its callsign silences the missing-callsign warning.
    assert!(!stdout.contains("No callsign configured"), "{stdout}");
    let line = stdout
        .lines()
        .find(|l| l.contains("Using configuration file"))
        .unwrap_or_else(|| panic!("no \"Using configuration file\" line\n{stdout}"));
    assert!(line.contains("INFO"), "{line}");
    assert!(
        line.contains(&config.display().to_string()),
        "{line} does not name {}",
        config.display()
    );
}

#[cfg(all(unix, not(feature = "cpal-backend")))]
#[test]
fn with_no_config_anywhere_the_daemon_warns_listing_the_search() {
    if !system_config_absent() {
        return;
    }
    let cwd = TempDir::new("run-no-config-cwd");
    let home = TempDir::new("run-no-config-home");
    let home_str = home.path().to_str().expect("UTF-8 temp path");
    let (mut daemon, _) = start_until_ready(cwd.path(), &[], &[("HOME", home_str)]);
    let (stdout, _) = daemon.kill();
    let line = stdout
        .lines()
        .find(|l| l.contains("No configuration file found"))
        .unwrap_or_else(|| panic!("no \"No configuration file found\" line\n{stdout}"));
    assert!(line.contains("WARN"), "{line}");
    assert!(
        line.contains(&format!(
            "using built-in defaults (searched: coppad.toml, {}, {SYSTEM_CONFIG_PATH})",
            home_config_path(home.path()).display()
        )),
        "{line}"
    );
    assert!(line.contains("coppad.toml.example"), "{line}");
    assert!(!stdout.contains("Using configuration file"), "{stdout}");
}

#[cfg(not(feature = "cpal-backend"))]
#[test]
fn log_level_overrides_rust_log_on_a_running_daemon() {
    let dir = TempDir::new("run-level");
    // A local config file, so the run does not depend on this machine's
    // /etc/coppa/coppad.toml.
    dir.write("coppad.toml", "");

    // --log-level info wins over RUST_LOG=off: the daemon logs readiness.
    let (mut loud, ready_after) =
        start_until_ready(dir.path(), &["--log-level", "info"], &[("RUST_LOG", "off")]);
    let (stdout, _) = loud.kill();
    assert!(stdout.contains("Coppa Daemon starting"), "{stdout}");

    // --log-level off wins over RUST_LOG=info: the same daemon runs silently.
    // Give it well past the time the loud run needed to become ready, then
    // require it to still be running and to have printed nothing.
    let mut quiet = Daemon::spawn(dir.path(), &["--log-level", "off"], &[("RUST_LOG", "info")]);
    let settle = (ready_after * 3).max(Duration::from_millis(1500));
    while quiet.started.elapsed() < settle {
        if let Some(status) = quiet.try_exit() {
            panic!(
                "silent daemon exited with {status}\n--- stdout ---\n{}\n--- stderr ---\n{}",
                quiet.stdout(),
                quiet.stderr()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let (stdout, stderr) = quiet.kill();
    assert_eq!(stdout, "", "--log-level off must silence tracing");
    assert_eq!(stderr, "");
}

// --- startup failures (COP-32) ----------------------------------------------
//
// A condition that stops the daemon doing its job is an ERROR, never
// `Daemon ready`. Config and bind failures also exit 1 with `Error: ...` on
// stderr. Each fails before any audio device opens, so these run in every
// build that compiles them.

/// Run the daemon on `toml` and require a fatal startup error: exit 1, an
/// ERROR log line, no readiness, and `Error: <stderr_prefix>` on stderr.
/// Returns (stdout, stderr).
fn run_startup_error(toml: &str, stderr_prefix: &str) -> (String, String) {
    let dir = TempDir::new("startup-error");
    dir.write("station.toml", toml);
    let args = ["--config", "station.toml"];
    let out = run(dir.path(), &args);
    out.assert_code(1, &args);
    assert!(out.stdout.contains("ERROR"), "{}", out.stdout);
    assert!(
        !out.stdout.contains("Daemon ready") && !out.stdout.contains("Event loop started"),
        "{}",
        out.stdout
    );
    assert!(
        out.stderr.starts_with(&format!("Error: {stderr_prefix}")),
        "{}",
        out.stderr
    );
    (out.stdout, out.stderr)
}

#[test]
fn an_unknown_ptt_method_logs_an_error_and_exits_1() {
    run_startup_error(
        "[radio]\nptt_method = \"carrier-pigeon\"\n",
        "failed to start daemon event loop: invalid [radio] ptt_method",
    );
}

#[test]
fn a_sample_rate_the_engine_does_not_run_at_is_a_startup_error() {
    let (stdout, _) = run_startup_error(
        "[audio]\nsample_rate = 44100\n",
        "[audio] sample_rate = 44100 is not supported",
    );
    assert!(stdout.contains("set sample_rate = 48000"), "{stdout}");
}

#[test]
fn an_unknown_profile_name_is_a_startup_error() {
    // COP-36: run mode rejects the name instead of building a level-derived
    // engine from it. --check still accepts it (see
    // check_only_deserializes_and_never_touches_hardware_or_listeners).
    let (stdout, _) = run_startup_error(
        "[engine]\nprofile = \"HF_STANDRD\"\n",
        "[engine] profile = \"HF_STANDRD\" is not a known profile",
    );
    assert!(
        stdout.contains("HF_ROBUST, HF_STANDARD, VHF_FAST, EMERGENCY"),
        "{stdout}"
    );
}

#[test]
fn a_zero_buffer_size_is_a_startup_error() {
    let (stdout, _) = run_startup_error(
        "[audio]\nbuffer_size = 0\n",
        "[audio] buffer_size = 0 is not supported",
    );
    assert!(stdout.contains("set buffer_size = 8192"), "{stdout}");
}

#[cfg(not(feature = "cpal-backend"))]
#[test]
fn a_named_audio_device_without_the_cpal_backend_is_a_startup_error() {
    run_startup_error(
        "[audio]\ninput_device = \"USB Audio CODEC\"\n",
        "[audio] input_device = \"USB Audio CODEC\" is set, but this coppad was built \
         without the cpal-backend feature",
    );
}

#[test]
fn a_taken_vara_command_port_is_a_startup_error() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").expect("hold a port");
    let port = taken.local_addr().unwrap().port();
    run_startup_error(
        &format!("[host]\nvara_enabled = true\nvara_command_port = {port}\nvara_data_port = 0\n"),
        &format!("failed to bind the VARA command port to 127.0.0.1:{port}"),
    );
}

#[test]
fn a_taken_vara_data_port_is_a_startup_error() {
    // Port 0 lets the OS pick a free command port, so only the data port can fail.
    let taken = std::net::TcpListener::bind("127.0.0.1:0").expect("hold a port");
    let port = taken.local_addr().unwrap().port();
    run_startup_error(
        &format!("[host]\nvara_enabled = true\nvara_command_port = 0\nvara_data_port = {port}\n"),
        &format!("failed to bind the VARA data port to 127.0.0.1:{port}"),
    );
}

#[cfg(feature = "websocket")]
#[test]
fn a_taken_websocket_port_is_a_startup_error() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").expect("hold a port");
    let port = taken.local_addr().unwrap().port();
    run_startup_error(
        &format!(
            "[host]\nwebsocket_enabled = true\nwebsocket_port = {port}\n\
             websocket_token = \"{WS_TOKEN}\"\n"
        ),
        &format!("failed to bind the WebSocket port to 127.0.0.1:{port}"),
    );
}

// --- WebSocket access (COP-53) ----------------------------------------------

/// A valid `[host] websocket_token` for the tests below.
const WS_TOKEN: &str = "0123456789abcdef0123456789abcdef";

#[test]
fn websocket_without_a_token_is_a_startup_error() {
    // COP-53: refused in every build, before any socket is bound. --check
    // still accepts the same config (see
    // check_only_deserializes_and_never_touches_hardware_or_listeners).
    let (stdout, _) = run_startup_error(
        "[host]\nwebsocket_enabled = true\n",
        "[host] websocket_enabled = true requires [host] websocket_token",
    );
    assert!(!stdout.contains("WebSocket server listening"), "{stdout}");
}

#[test]
fn a_short_websocket_token_is_a_startup_error() {
    run_startup_error(
        "[host]\nwebsocket_enabled = true\nwebsocket_token = \"short\"\n",
        "[host] websocket_token is 5 characters long",
    );
}

#[test]
fn an_invalid_websocket_origin_is_a_startup_error() {
    run_startup_error(
        &format!(
            "[host]\nwebsocket_enabled = true\nwebsocket_token = \"{WS_TOKEN}\"\n\
             websocket_allowed_origins = [\"*\"]\n"
        ),
        "[host] websocket_allowed_origins entry \"*\"",
    );
}

/// Send one WebSocket upgrade request for `target` with the given extra
/// header lines, and return the response's status line.
#[cfg(all(feature = "websocket", not(feature = "cpal-backend")))]
fn upgrade_status(port: u16, target: &str, headers: &[String]) -> String {
    use std::io::{BufRead, BufReader, Write};

    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(DEADLINE))
        .expect("set read timeout");
    let mut request = format!(
        "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: Upgrade\r\n\
         Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"
    );
    for header in headers {
        request.push_str(header);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).expect("send upgrade");
    let mut status = String::new();
    BufReader::new(stream)
        .read_line(&mut status)
        .expect("read status line");
    status
}

/// COP-53, end to end through the real binary: the configured token and
/// origin allowlist reach the WebSocket server. A wiring bug shows up either
/// as the valid cases failing (fail closed) or the evil origin passing (fail
/// open).
///
/// CI's full suite builds with `cpal-backend`, which compiles this test out
/// (like every `start_until` test), so it runs only under
/// `cargo test -p coppa-daemon --features websocket --test cli`.
#[cfg(all(feature = "websocket", not(feature = "cpal-backend")))]
#[test]
fn websocket_handshake_requires_an_allowed_origin_and_the_token() {
    let dir = TempDir::new("ws-auth");
    dir.write(
        "station.toml",
        &format!(
            "[host]\nwebsocket_enabled = true\nwebsocket_port = 0\n\
             websocket_token = \"{WS_TOKEN}\"\n\
             websocket_allowed_origins = [\"http://localhost:3000\"]\n"
        ),
    );
    let args = ["--config", "station.toml"];
    let (mut daemon, _) = start_until(dir.path(), &args, &[], "Daemon ready");
    let stdout = daemon.stdout();
    let line = stdout
        .lines()
        .find(|line| line.contains("WebSocket server listening"))
        .unwrap_or_else(|| panic!("no listening line:\n{stdout}"));
    let after = &line[line.find("127.0.0.1:").expect("listen address") + "127.0.0.1:".len()..];
    let port: u16 = after
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .expect("listen port");

    let evil = "Origin: https://evil.example".to_string();
    let bearer = format!("Authorization: Bearer {WS_TOKEN}");
    let cases: [(String, Vec<String>, &str); 5] = [
        ("/".to_string(), vec![evil.clone()], "HTTP/1.1 403"),
        ("/".to_string(), vec![evil, bearer.clone()], "HTTP/1.1 403"),
        ("/".to_string(), vec![], "HTTP/1.1 401"),
        ("/".to_string(), vec![bearer], "HTTP/1.1 101"),
        (
            format!("/?token={WS_TOKEN}"),
            vec!["Origin: http://localhost:3000".to_string()],
            "HTTP/1.1 101",
        ),
    ];
    for (target, headers, expected) in &cases {
        let status = upgrade_status(port, target, headers);
        assert!(
            status.starts_with(expected),
            "{headers:?}: expected {expected}, got {status:?}"
        );
    }

    // Give the event loop time to log both accepted clients.
    let deadline = Instant::now() + DEADLINE;
    let connected = |stdout: &str| {
        stdout
            .lines()
            .filter(|line| line.contains("event_loop") && line.contains("Client connected"))
            .count()
    };
    while connected(&daemon.stdout()) < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(200));
    let (stdout, stderr) = daemon.kill();
    assert!(
        !stdout.contains(WS_TOKEN),
        "token leaked to stdout:\n{stdout}"
    );
    assert!(
        !stderr.contains(WS_TOKEN),
        "token leaked to stderr:\n{stderr}"
    );
    assert_eq!(
        connected(&stdout),
        2,
        "only the two authorized handshakes may reach the event loop:\n{stdout}"
    );
}

#[cfg(feature = "cpal-backend")]
#[test]
fn a_missing_named_input_device_is_a_startup_error() {
    run_startup_error(
        "[audio]\ninput_device = \"COP-32 no such input device\"\n",
        "audio input device \"COP-32 no such input device\" not found",
    );
}

#[cfg(feature = "cpal-backend")]
#[test]
fn a_missing_named_output_device_is_a_startup_error() {
    // No input name: the output name must be checked before the default
    // input device is opened, or a host without audio would report that
    // failure instead.
    run_startup_error(
        "[audio]\noutput_device = \"COP-32 no such output device\"\n",
        "audio output device \"COP-32 no such output device\" not found",
    );
}

/// An unreachable rigctld leaves the daemon running without PTT: it logs
/// ERROR and reports itself degraded instead of ready, and keeps running.
#[cfg(not(feature = "cpal-backend"))]
#[test]
fn an_unreachable_rigctld_is_reported_degraded_not_ready() {
    let dir = TempDir::new("run-rigctld");
    dir.write(
        "station.toml",
        "[radio]\nptt_method = \"rigctld\"\nrigctld_address = \"127.0.0.1:1\"\n",
    );
    let args = ["--config", "station.toml"];
    let (mut daemon, _) = start_until(dir.path(), &args, &[], "Daemon running degraded");
    assert!(
        daemon.try_exit().is_none(),
        "a degraded daemon keeps running"
    );
    let (stdout, _) = daemon.kill();
    assert!(!stdout.contains("Daemon ready"), "{stdout}");
    for marker in [
        "Failed to connect to rigctld",
        "Daemon running degraded",
        "no PTT: rigctld at 127.0.0.1:1 is unreachable",
    ] {
        let line = stdout
            .lines()
            .find(|l| l.contains(marker))
            .unwrap_or_else(|| panic!("no {marker:?} line\n{stdout}"));
        assert!(line.contains("ERROR"), "{line}");
    }
}
