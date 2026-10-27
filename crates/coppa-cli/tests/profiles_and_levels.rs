//! `coppa profiles`, its deprecated `coppa config` alias, and `--level` on
//! `tx`, driven through the real binary (COP-55).
#![cfg(feature = "file-backend")]

use std::path::PathBuf;
use std::process::{Command, Output};

/// A per-process temporary WAV path, removed on drop.
struct TempWav(PathBuf);

impl TempWav {
    fn new(tag: &str) -> Self {
        Self(std::env::temp_dir().join(format!(
            "coppa_profiles_and_levels_{tag}_{}.wav",
            std::process::id()
        )))
    }

    fn path(&self) -> &str {
        self.0.to_str().expect("temp path is UTF-8")
    }
}

impl Drop for TempWav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn coppa(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_coppa"))
        .args(args)
        .output()
        .expect("failed to spawn coppa")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8")
}

#[test]
fn profiles_prints_the_ladder_and_exits_0() {
    let output = coppa(&["profiles"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    for level in 1..=9 {
        assert!(
            out.lines()
                .any(|line| line.starts_with(&format!("{level} "))),
            "no row for level {level}:\n{out}"
        );
    }
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
}

#[test]
fn unknown_profile_exits_1_on_stderr() {
    let output = coppa(&["profiles", "-p", "BOGUS"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).is_empty(), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("Unknown profile: BOGUS"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn config_is_a_deprecated_alias_for_profiles() {
    for flags in [&[][..], &["-p", "HF_ROBUST"][..]] {
        let profiles = coppa(&[&["profiles"][..], flags].concat());
        let config = coppa(&[&["config"][..], flags].concat());
        assert!(config.status.success(), "{}", stderr(&config));
        assert_eq!(stdout(&config), stdout(&profiles));
        let warning = stderr(&config);
        assert!(
            warning.contains("'coppa config' is deprecated"),
            "{warning}"
        );
        assert!(warning.contains("'coppa profiles'"), "{warning}");
        assert!(
            !stderr(&profiles).contains("deprecated"),
            "{}",
            stderr(&profiles)
        );
    }
    let quiet = coppa(&["--quiet", "config"]);
    assert!(
        stderr(&quiet).contains("'coppa config' is deprecated"),
        "{}",
        stderr(&quiet)
    );
}

#[test]
fn config_is_hidden_from_help_but_still_answers_it() {
    let help = stdout(&coppa(&["--help"]));
    assert!(help.contains("profiles"), "{help}");
    assert!(!help.contains("config"), "{help}");

    let output = coppa(&["config", "--help"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output)
            .starts_with("Deprecated: use 'coppa profiles'. Removed in the next release"),
        "{}",
        stdout(&output)
    );
}

/// The ticket's bug as a regression test: the max bytes `coppa profiles` shows
/// is the limit `coppa tx` enforces (it used to print 64 for a limit of 56).
/// Only the profiles that do not compress, since compressed size depends on the
/// text.
#[test]
fn shown_max_bytes_is_the_tx_limit() {
    for name in ["HF_ROBUST", "EMERGENCY"] {
        let detail = stdout(&coppa(&["profiles", "-p", name]));
        let max: usize = detail
            .lines()
            .find_map(|line| line.trim().strip_prefix("Max bytes per frame: "))
            .unwrap_or_else(|| panic!("no max bytes line:\n{detail}"))
            .parse()
            .expect("max bytes is a number");

        let wav = TempWav::new(name);
        let fits = coppa(&["tx", &"A".repeat(max), "--profile", name, "-o", wav.path()]);
        assert!(
            fits.status.success(),
            "{name}: {max} bytes refused: {}",
            stderr(&fits)
        );

        let over = coppa(&[
            "tx",
            &"A".repeat(max + 1),
            "--profile",
            name,
            "-o",
            wav.path(),
        ]);
        assert_eq!(over.status.code(), Some(1), "{name}: {} bytes", max + 1);
        assert!(
            stderr(&over).contains(&format!("(max {max} bytes)")),
            "{name}: {}",
            stderr(&over)
        );
    }
}

#[test]
fn tx_at_a_level_decodes_with_plain_rx() {
    let wav = TempWav::new("level4");
    let tx = coppa(&["tx", "CQ CQ DE TEST", "--level", "4", "-o", wav.path()]);
    assert!(tx.status.success(), "{}", stderr(&tx));
    // 37800 samples at 48 kHz is the 788 ms HF row of `coppa profiles`.
    assert!(
        stdout(&tx).contains("Generated 37800 audio samples"),
        "{}",
        stdout(&tx)
    );

    let rx = coppa(&["rx", "-i", wav.path()]);
    assert!(rx.status.success(), "{}", stderr(&rx));
    assert!(
        stdout(&rx).contains("Decoded: \"CQ CQ DE TEST\""),
        "{}",
        stdout(&rx)
    );
}

#[test]
fn tx_level_9_on_hf_is_refused() {
    let wav = TempWav::new("level9_hf");
    let output = coppa(&["tx", "CQ", "--level", "9", "-o", wav.path()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("level 9 is VHF only"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn tx_level_9_on_vhf_fast_decodes() {
    let wav = TempWav::new("level9_vhf");
    let tx = coppa(&[
        "tx",
        "CQ",
        "--profile",
        "VHF_FAST",
        "--level",
        "9",
        "-o",
        wav.path(),
    ]);
    assert!(tx.status.success(), "{}", stderr(&tx));

    let rx = coppa(&["rx", "--profile", "VHF_FAST", "-i", wav.path()]);
    assert!(rx.status.success(), "{}", stderr(&rx));
    assert!(stdout(&rx).contains("Decoded: \"CQ\""), "{}", stdout(&rx));
}

#[test]
fn level_out_of_range_is_a_usage_error() {
    let output = coppa(&["tx", "CQ", "--level", "10"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
}
