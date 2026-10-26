//! COP-56: `coppa rx` is the one receiver. It takes a bare FILE path,
//! `--duration` and `--callsign`, and `coppa listen` is a hidden, deprecated
//! alias that prints the same output. These tests run the real binary.
#![cfg(feature = "file-backend")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MESSAGE: &str = "CQ CQ CQ DE VK2ABC K";
const PAYLOAD_HEX: &str = "435120435120435120444520564b32414243204b";
/// `"FIRST FRAME DE VK2ABC"` as lowercase hex.
const FIRST_HEX: &str = "4649525354204652414d4520444520564b32414243";

/// A per-process temp path, removed when dropped.
struct TempWav(PathBuf);

impl TempWav {
    fn new(name: &str) -> Self {
        Self(std::env::temp_dir().join(format!("coppa_cop56_{}_{name}.wav", std::process::id())))
    }
}

impl Drop for TempWav {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

fn coppa(args: &[&str], path: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coppa"));
    for arg in args {
        if *arg == "<path>" {
            command.arg(path.expect("<path> needs a path"));
        } else {
            command.arg(arg);
        }
    }
    command.output().expect("failed to spawn coppa")
}

/// Run `coppa`, require success, and return (stdout, stderr).
fn ok(args: &[&str], path: &Path) -> (String, String) {
    let output = coppa(args, Some(path));
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "coppa {args:?} failed: {stderr}\n{stdout}"
    );
    (stdout, stderr)
}

/// `coppa tx MESSAGE -o <path>`.
fn write_cq_wav(name: &str) -> TempWav {
    let wav = TempWav::new(name);
    ok(&["tx", MESSAGE, "-o", "<path>"], &wav.0);
    wav
}

/// FIRST, 1.0 s of silence, SECOND. Frame 1 ends at 1.365 s and frame 2
/// starts at 2.365 s, so a 2-second cut falls in the gap. 179,040 samples.
fn write_two_frame_wav(name: &str) -> TempWav {
    use coppa_audio::AudioSink;
    let wav = TempWav::new(name);
    let core = coppa_engine::CoppaCore::new();
    let mut samples = core.encode("FIRST FRAME DE VK2ABC").unwrap();
    samples.extend(std::iter::repeat_n(0.0f32, 48_000));
    samples.extend(core.encode("SECOND FRAME DE VK2ABC").unwrap());
    assert_eq!(samples.len(), 179_040);
    let mut sink = coppa_audio::file_backend::WavSink::new(&wav.0, 48_000);
    sink.start().unwrap();
    sink.write(&samples).unwrap();
    sink.stop().unwrap();
    wav
}

#[test]
fn rx_takes_a_bare_path() {
    let cq = write_cq_wav("bare");
    let (bare, _) = ok(&["rx", "<path>"], &cq.0);
    let (flag, _) = ok(&["rx", "-i", "<path>"], &cq.0);
    assert!(bare.contains("Decoded: \"CQ CQ CQ DE VK2ABC K\""), "{bare}");
    assert_eq!(bare, flag);
}

#[test]
fn rx_rejects_a_path_and_input_together() {
    let cq = write_cq_wav("conflict");
    let output = coppa(&["rx", "<path>", "-i", "<path>"], Some(&cq.0));
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

#[test]
fn rx_duration_decodes_only_the_first_seconds_of_a_file() {
    let two = write_two_frame_wav("duration");
    let first = "Decoded: \"FIRST FRAME DE VK2ABC\"";
    let second = "Decoded: \"SECOND FRAME DE VK2ABC\"";

    let (all, _) = ok(&["rx", "<path>"], &two.0);
    assert!(all.contains("Reading 179040 samples from"), "{all}");
    assert!(all.contains(first) && all.contains(second), "{all}");

    let (cut, _) = ok(&["rx", "<path>", "-d", "2"], &two.0);
    assert!(
        cut.contains("Reading the first 2 s (96000 of 179040 samples) from"),
        "{cut}"
    );
    assert!(cut.contains(first), "{cut}");
    assert!(!cut.contains("SECOND FRAME"), "{cut}");

    for flags in [["--duration", "60"], ["-d", "0"]] {
        let (uncapped, _) = ok(&["rx", "<path>", flags[0], flags[1]], &two.0);
        assert_eq!(uncapped, all, "{flags:?}");
    }

    let (raw, _) = ok(&["rx", "<path>", "--raw", "-d", "2"], &two.0);
    assert_eq!(raw.trim(), FIRST_HEX);
}

#[test]
fn rx_callsign_is_echoed_only_under_verbose() {
    let cq = write_cq_wav("callsign");
    let (_, verbose_err) = ok(
        &["--verbose", "rx", "<path>", "--callsign", "VK2ABC"],
        &cq.0,
    );
    assert!(
        verbose_err.contains("[verbose] Callsign: VK2ABC"),
        "{verbose_err}"
    );

    let (plain, _) = ok(&["rx", "<path>"], &cq.0);
    let (with_callsign, err) = ok(&["rx", "<path>", "--callsign", "VK2ABC"], &cq.0);
    assert_eq!(with_callsign, plain);
    assert!(!err.contains("Callsign"), "{err}");
}

#[test]
fn listen_is_a_deprecated_alias_for_rx() {
    let cq = write_cq_wav("listen");
    let two = write_two_frame_wav("listen_two");

    let (rx_out, rx_err) = ok(&["rx", "<path>"], &cq.0);
    let (listen_out, listen_err) = ok(&["listen", "<path>"], &cq.0);
    assert_eq!(listen_out, rx_out);
    assert!(
        listen_err.contains("'coppa listen' is deprecated"),
        "{listen_err}"
    );
    assert!(listen_err.contains("'coppa rx'"), "{listen_err}");
    assert!(!rx_err.contains("deprecated"), "{rx_err}");

    let (rx_raw, _) = ok(&["rx", "--raw", "<path>"], &cq.0);
    let (listen_raw, _) = ok(&["listen", "--raw", "<path>"], &cq.0);
    assert_eq!(listen_raw, rx_raw);
    assert_eq!(listen_raw.trim(), PAYLOAD_HEX);

    let (rx_cut, _) = ok(&["rx", "-d", "2", "<path>"], &two.0);
    let (listen_cut, _) = ok(&["listen", "-d", "2", "<path>"], &two.0);
    assert_eq!(listen_cut, rx_cut);

    let (quiet_out, quiet_err) = ok(&["--quiet", "listen", "<path>"], &cq.0);
    assert!(
        quiet_err.contains("'coppa listen' is deprecated"),
        "{quiet_err}"
    );
    assert!(!quiet_out.contains("deprecated"), "{quiet_out}");
}

#[test]
fn listen_is_hidden_from_help_but_still_answers_it() {
    let help = coppa(&["--help"], None);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(
        !help
            .lines()
            .any(|l| l.split_whitespace().next() == Some("listen")),
        "{help}"
    );
    assert!(
        help.lines()
            .any(|l| l.split_whitespace().next() == Some("rx")),
        "{help}"
    );

    let listen_help = coppa(&["listen", "--help"], None);
    assert!(listen_help.status.success());
    let listen_help = String::from_utf8(listen_help.stdout).unwrap();
    assert!(listen_help.contains("Deprecated"), "{listen_help}");
    assert!(listen_help.contains("--duration"), "{listen_help}");
}
