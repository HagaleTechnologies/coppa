//! Keep the shipped service files (`contrib/coppad.service`, the launchd
//! plist) and the guides that install them in step with the real `coppad`.
//!
//! Each service file's own command line is run through `coppad --check`, so a
//! CLI change that would stop the unit or the agent from starting fails here.
//! The directive checks each close a specific failure found while writing the
//! unit; every assertion message says which one.
//!
//! Repo files are read at runtime (never `include_str!`), so the crate stays
//! packageable without `contrib/` or `docs/`.

use std::collections::BTreeSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// How long `coppad --check` may take before the test fails.
const DEADLINE: Duration = Duration::from_secs(5);

const CHECK_OK: &str =
    "Configuration syntax and field types OK.\nHardware and runtime settings were not checked.\n";

const UNIT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../contrib/coppad.service");
const PLIST: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../contrib/io.github.hagaletechnologies.coppad.plist"
);
const EXAMPLE_CONFIG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../coppad.toml.example");
const PI_GUIDE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/tutorials/raspberry-pi-install.md"
);
const CONTRIB_README: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../contrib/README.md");

const LAUNCHD_LABEL: &str = "io.github.hagaletechnologies.coppad";

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// A uniquely named directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "coppad-service-{tag}-{}-{}",
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
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Every `key=value` line of a systemd unit as `(section, key, value)`, in
/// order. Blank lines and `#`/`;` comments are skipped; repeated keys are kept.
fn unit_entries(text: &str) -> Vec<(String, String, String)> {
    let mut section = String::new();
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].to_string();
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            entries.push((
                section.clone(),
                key.trim().to_string(),
                value.trim().to_string(),
            ));
        }
    }
    entries
}

/// Every value of `key` in `section`, in order.
fn unit_values(entries: &[(String, String, String)], section: &str, key: &str) -> Vec<String> {
    entries
        .iter()
        .filter(|(s, k, _)| s == section && k == key)
        .map(|(_, _, v)| v.clone())
        .collect()
}

/// The single value of `key` in `section`.
fn unit_value(entries: &[(String, String, String)], section: &str, key: &str) -> String {
    let values = unit_values(entries, section, key);
    assert_eq!(
        values.len(),
        1,
        "contrib/coppad.service must set [{section}] {key} exactly once, found {values:?}"
    );
    values[0].clone()
}

/// The unit's `ExecStart`, split on whitespace.
fn unit_exec_start(entries: &[(String, String, String)]) -> Vec<String> {
    unit_value(entries, "Service", "ExecStart")
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// The unit's `SupplementaryGroups`, whitespace-split across every line, in order.
fn unit_groups(entries: &[(String, String, String)]) -> Vec<String> {
    unit_values(entries, "Service", "SupplementaryGroups")
        .iter()
        .flat_map(|v| v.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .collect()
}

/// The value after `--config` in a coppad command line.
fn config_path(args: &[String]) -> String {
    let at = args
        .iter()
        .position(|a| a == "--config")
        .unwrap_or_else(|| panic!("command line {args:?} has no --config"));
    args.get(at + 1)
        .unwrap_or_else(|| panic!("command line {args:?} has no value after --config"))
        .clone()
}

/// Remove whitespace between XML tags, so element sequences can be matched
/// regardless of indentation.
fn strip_tag_whitespace(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        out.push_str(line.trim());
    }
    out
}

/// Every `<string>` in the `<array>` that follows `<key>{key}</key>`.
fn plist_strings_after(text: &str, key: &str) -> Vec<String> {
    let flat = strip_tag_whitespace(text);
    let open = format!("<key>{key}</key><array>");
    let start = flat
        .find(&open)
        .unwrap_or_else(|| panic!("plist has no {open}"))
        + open.len();
    let end = start
        + flat[start..]
            .find("</array>")
            .unwrap_or_else(|| panic!("plist's {key} array is not closed"));
    let mut rest = &flat[start..end];
    let mut values = Vec::new();
    while let Some(after_open) = rest.strip_prefix("<string>") {
        let close = after_open
            .find("</string>")
            .expect("plist <string> is not closed");
        let value = &after_open[..close];
        assert!(
            !value.contains('&'),
            "plist value {value:?} uses an XML entity; keep the shipped values entity-free"
        );
        values.push(value.to_string());
        rest = &after_open[close + "</string>".len()..];
    }
    assert!(rest.is_empty(), "unexpected content in {key}: {rest:?}");
    values
}

/// The `<string>` that follows `<key>{key}</key>`.
fn plist_string(text: &str, key: &str) -> String {
    let flat = strip_tag_whitespace(text);
    let open = format!("<key>{key}</key><string>");
    let start = flat
        .find(&open)
        .unwrap_or_else(|| panic!("plist has no {open}"))
        + open.len();
    let end = start
        + flat[start..]
            .find("</string>")
            .unwrap_or_else(|| panic!("plist's {key} string is not closed"));
    flat[start..end].to_string()
}

/// Run a service file's own coppad command line with `--check`, pointing its
/// `--config` at a copy of `coppad.toml.example`, and require success.
fn run_check(args: &[String]) {
    let binary = Path::new(&args[0]);
    assert!(
        binary.is_absolute(),
        "{binary:?} must be absolute: systemd and launchd do not search PATH the way a shell does"
    );
    assert_eq!(
        binary.file_name().and_then(|n| n.to_str()),
        Some("coppad"),
        "service files must start coppad, not {binary:?}"
    );

    let dir = TempDir::new("check");
    let config = dir.path().join("coppad.toml");
    std::fs::copy(EXAMPLE_CONFIG, &config).expect("copy coppad.toml.example");
    let mut rest: Vec<String> = args[1..].to_vec();
    let at = rest
        .iter()
        .position(|a| a == "--config")
        .unwrap_or_else(|| panic!("command line {args:?} has no --config"));
    assert!(
        at + 1 < rest.len(),
        "command line {args:?} has no value after --config"
    );
    rest[at + 1] = config.to_str().expect("utf-8 temp path").to_string();
    rest.push("--check".to_string());

    let stdout_path = dir.path().join("stdout");
    let stderr_path = dir.path().join("stderr");
    let mut child = Command::new(env!("CARGO_BIN_EXE_coppad"))
        .args(&rest)
        .current_dir(dir.path())
        .env_remove("RUST_LOG")
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE")
        .stdin(Stdio::null())
        .stdout(File::create(&stdout_path).expect("create stdout capture"))
        .stderr(File::create(&stderr_path).expect("create stderr capture"))
        .spawn()
        .expect("failed to spawn coppad");
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll coppad") {
            break status;
        }
        if started.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            panic!("coppad {rest:?} did not exit within {DEADLINE:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = std::fs::read_to_string(&stdout_path).expect("read stdout capture");
    let stderr = std::fs::read_to_string(&stderr_path).expect("read stderr capture");
    assert!(
        status.success(),
        "coppad {rest:?} exited {status}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, CHECK_OK, "stderr:\n{stderr}");
}

#[test]
fn systemd_unit_execstart_is_accepted_by_coppad() {
    let entries = unit_entries(&read(UNIT));
    run_check(&unit_exec_start(&entries));
}

#[test]
fn systemd_unit_runs_unprivileged_with_device_groups() {
    let entries = unit_entries(&read(UNIT));
    let user = unit_value(&entries, "Service", "User");
    let group = unit_value(&entries, "Service", "Group");
    assert_eq!(user, "coppa", "the install guides create the user coppa");
    assert_eq!(group, "coppa", "the install guides create the group coppa");
    assert_ne!(user, "root", "coppad must not run as root");
    assert_ne!(group, "root", "coppad must not run as group root");
    let groups: BTreeSet<String> = unit_groups(&entries).into_iter().collect();
    let expected: BTreeSet<String> = ["audio", "dialout", "gpio"]
        .iter()
        .map(|g| g.to_string())
        .collect();
    assert_eq!(
        groups, expected,
        "coppad needs audio (sound cards), dialout (serial PTT) and gpio (sysfs GPIO PTT)"
    );
}

#[test]
fn systemd_unit_restarts_on_failure_without_giving_up() {
    let entries = unit_entries(&read(UNIT));
    assert_eq!(
        unit_value(&entries, "Service", "Restart"),
        "on-failure",
        "an unattended gateway must come back after a crash or a failed start"
    );
    assert!(
        !unit_values(&entries, "Service", "RestartSec").is_empty(),
        "RestartSec spaces restarts out instead of systemd's 100 ms default"
    );
    assert_eq!(
        unit_value(&entries, "Unit", "StartLimitIntervalSec"),
        "0",
        "without StartLimitIntervalSec=0 systemd stops restarting after a burst of failures \
         (for example, an unplugged USB sound card) and the gateway stays down"
    );
    for key in ["StartLimitIntervalSec", "StartLimitBurst"] {
        assert!(
            unit_values(&entries, "Service", key).is_empty(),
            "{key} belongs under [Unit]; systemd ignores StartLimitIntervalSec under [Service]"
        );
    }
}

#[test]
fn systemd_unit_leaves_the_ptt_guardian_alone_on_stop() {
    let entries = unit_entries(&read(UNIT));
    assert_eq!(
        unit_value(&entries, "Service", "KillMode"),
        "process",
        "under KillMode=control-group or mixed, systemd SIGKILLs the COP-31 PTT guardian \
         (coppad --ptt-guardian) along with coppad, so a stop that times out can leave PTT keyed"
    );
    assert!(
        !unit_values(&entries, "Service", "TimeoutStopSec").is_empty(),
        "TimeoutStopSec bounds how long a stuck coppad delays a stop or restart"
    );
}

#[test]
fn systemd_unit_omits_directives_that_break_audio_or_ptt() {
    let entries = unit_entries(&read(UNIT));
    let forbidden = [
        (
            "ProtectKernelTunables",
            "it makes /sys read-only, so GPIO PTT writes fail",
        ),
        (
            "PrivateDevices",
            "it hides /dev/snd and /dev/ttyUSB*, so there is no audio or serial PTT",
        ),
        (
            "DynamicUser",
            "there is no fixed user to put in the audio/dialout/gpio groups",
        ),
        (
            "ProtectClock",
            "below systemd 252 it denies every device except the RTC",
        ),
        (
            "DeviceAllow",
            "a device allow-list closes /dev/snd, serial ports or GPIO",
        ),
        (
            "DevicePolicy",
            "a device policy closes /dev/snd, serial ports or GPIO",
        ),
    ];
    for (key, why) in forbidden {
        assert!(
            entries.iter().all(|(_, k, _)| k != key),
            "contrib/coppad.service must not set {key}: {why}"
        );
    }
}

#[test]
fn systemd_unit_logs_plain_text() {
    let entries = unit_entries(&read(UNIT));
    assert!(
        unit_values(&entries, "Service", "Environment")
            .iter()
            .any(|v| v.split_whitespace().any(|kv| kv == "NO_COLOR=1")),
        "coppad writes ANSI colour codes even when stdout is not a terminal; \
         NO_COLOR=1 keeps them out of the journal"
    );
}

#[test]
fn launchd_plist_program_arguments_are_accepted_by_coppad() {
    run_check(&plist_strings_after(&read(PLIST), "ProgramArguments"));
}

#[test]
fn launchd_plist_restarts_only_on_failure() {
    let text = read(PLIST);
    let flat = strip_tag_whitespace(&text);
    assert!(
        flat.contains("<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>"),
        "KeepAlive must be SuccessfulExit=false: restart after a crash, but not after a clean exit"
    );
    assert_eq!(plist_string(&text, "Label"), LAUNCHD_LABEL);
    assert_eq!(
        Path::new(PLIST).file_name().and_then(|n| n.to_str()),
        Some(format!("{LAUNCHD_LABEL}.plist").as_str()),
        "launchd expects the plist file to be named after its Label"
    );
    assert!(
        flat.contains("<key>NO_COLOR</key><string>1</string>"),
        "NO_COLOR=1 keeps ANSI colour codes out of the log file"
    );
}

#[test]
fn pi_guide_creates_what_the_unit_expects() {
    let entries = unit_entries(&read(UNIT));
    let guide = read(PI_GUIDE);
    let user = unit_value(&entries, "Service", "User");
    let exec = unit_exec_start(&entries);
    let binary = &exec[0];
    let config = config_path(&exec);
    let groups = unit_groups(&entries).join(",");

    // Shell commands may continue over several lines with a trailing `\`.
    let joined = guide.replace("\\\n", " ");
    assert!(
        joined
            .lines()
            .any(|l| l.contains("useradd") && l.split_whitespace().last() == Some(user.as_str())),
        "the Pi guide must create the unit's User={user} with useradd"
    );
    assert!(
        guide.contains(&format!("--groups {groups}")),
        "the Pi guide must add the user to the unit's SupplementaryGroups (--groups {groups})"
    );
    assert!(
        guide.contains(&config),
        "the Pi guide must install the config where the unit reads it ({config})"
    );
    assert!(
        guide.contains(binary.as_str()),
        "the Pi guide must install coppad where the unit runs it ({binary})"
    );
    assert!(
        guide.contains("contrib/coppad.service"),
        "the Pi guide must install the shipped unit"
    );
    assert!(
        guide.contains("systemctl enable --now coppad"),
        "the Pi guide must enable and start the unit"
    );
}

#[test]
fn contrib_readme_installs_the_launchd_agent_by_its_label() {
    let readme = read(CONTRIB_README);
    let plist = read(PLIST);
    for needle in [
        LAUNCHD_LABEL,
        "launchctl bootstrap gui/",
        "launchctl bootout gui/",
    ] {
        assert!(
            readme.contains(needle),
            "contrib/README.md must show {needle:?} for the launchd agent"
        );
    }
    let config = config_path(&plist_strings_after(&plist, "ProgramArguments"));
    let log = plist_string(&plist, "StandardOutPath");
    assert!(
        readme.contains(&config),
        "contrib/README.md must install the config where the plist reads it ({config})"
    );
    assert!(
        readme.contains(&log),
        "contrib/README.md must point at the plist's log file ({log})"
    );
}
