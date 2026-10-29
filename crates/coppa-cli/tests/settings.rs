//! End-to-end proof that the CLI reads the same file locations and keys as coppad.
#![cfg(all(unix, feature = "file-backend"))]
mod common;
use common::{coppa_command, Scratch};
use std::path::PathBuf;
use std::process::{Command, Output};

fn station_command(scratch: &Scratch) -> Command {
    let mut command = coppa_command();
    command.env_remove("COPPA_CONFIG");
    command.env_remove("XDG_CONFIG_HOME");
    command.env_remove("APPDATA");
    command.env("HOME", &scratch.0);
    command.current_dir(&scratch.0);
    command
}
fn config_path(scratch: &Scratch) -> PathBuf {
    let path = scratch.0.join(".config/coppa/coppad.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    path
}
fn run(mut command: Command, args: &[&str]) -> Output {
    command.args(args).output().expect("spawn coppa")
}
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}
fn assert_ok(output: &Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        stdout(output),
        stderr(output)
    );
}

#[test]
fn persisted_callsign_and_profile_are_used_for_tx_and_rx() {
    let scratch = Scratch::new();
    let path = config_path(&scratch);
    std::fs::write(
        &path,
        "[engine]\ncallsign = \"W5AU\"\nprofile = \"HF_STANDARD\"\n",
    )
    .unwrap();
    let wav = scratch.0.join("cq.wav");
    let wav_str = wav.to_str().unwrap();
    let tx = run(
        station_command(&scratch),
        &["--verbose", "tx", "hello world hello world", "-o", wav_str],
    );
    assert_ok(&tx);
    let err = stderr(&tx);
    assert!(
        err.contains(&format!("Configuration file: {}", path.display())),
        "{err}"
    );
    assert!(
        err.contains(&format!(
            "Callsign: W5AU ([engine] callsign in {})",
            path.display()
        )),
        "{err}"
    );
    assert!(err.contains("Profile: HF_STANDARD"), "{err}");
    assert!(err.contains("speed_level=2"), "{err}");
    let rx = run(station_command(&scratch), &["rx", wav_str]);
    assert_ok(&rx);
    assert!(
        stdout(&rx).contains("Decoded: \"hello world hello world\""),
        "{}",
        stdout(&rx)
    );
}

#[test]
fn environment_and_flag_beat_file() {
    let scratch = Scratch::new();
    std::fs::write(config_path(&scratch), "[engine]\ncallsign = \"W5AU\"\n").unwrap();
    let wav = scratch.0.join("cq.wav");
    let wav_str = wav.to_str().unwrap();
    let mut command = station_command(&scratch);
    command.env("COPPA_CALLSIGN", "K1ABC");
    let env = run(command, &["--verbose", "tx", "hi", "-o", wav_str]);
    assert_ok(&env);
    assert!(stderr(&env).contains("Callsign: K1ABC (COPPA_CALLSIGN)"));
    let mut command = station_command(&scratch);
    command.env("COPPA_CALLSIGN", "K1ABC");
    let flag = run(
        command,
        &[
            "--verbose",
            "tx",
            "hi",
            "-o",
            wav_str,
            "--callsign",
            "VK2ABC",
        ],
    );
    assert_ok(&flag);
    assert!(stderr(&flag).contains("Callsign: VK2ABC (--callsign)"));
}

#[test]
fn bad_file_and_missing_explicit_path_are_errors() {
    let scratch = Scratch::new();
    let path = config_path(&scratch);
    std::fs::write(&path, "[engine]\ncalsign = \"W5AU\"\n").unwrap();
    let typo = run(station_command(&scratch), &["tx", "hi", "-o", "cq.wav"]);
    assert_eq!(typo.status.code(), Some(1));
    assert!(stderr(&typo).contains(&format!("Failed to parse config {}", path.display())));
    assert!(stderr(&typo).contains("unknown field `calsign`"));
    let config = run(station_command(&scratch), &["config"]);
    assert_ok(&config);
    let help = run(station_command(&scratch), &["tx", "--help"]);
    assert_ok(&help);
    for use_flag in [true, false] {
        let mut command = station_command(&scratch);
        if !use_flag {
            command.env("COPPA_CONFIG", "missing.toml");
        }
        let args: &[&str] = if use_flag {
            &["--config", "missing.toml", "tx", "hi"]
        } else {
            &["tx", "hi"]
        };
        let missing = run(command, args);
        assert_eq!(missing.status.code(), Some(1));
        assert!(
            stderr(&missing).contains("Failed to load config missing.toml"),
            "{}",
            stderr(&missing)
        );
    }
}

#[test]
fn coppad_only_ptt_is_rejected_on_live_path_only() {
    let scratch = Scratch::new();
    let path = config_path(&scratch);
    std::fs::write(&path, "[radio]\nptt_method = \"serial:/dev/ttyUSB0:dtr\"\n").unwrap();
    let live = run(station_command(&scratch), &["tx", "hi"]);
    assert_eq!(live.status.code(), Some(1));
    let tune = run(station_command(&scratch), &["tune", "--seconds", "0.01"]);
    assert_eq!(tune.status.code(), Some(1));
    assert!(stderr(&tune).contains("coppad-only"));
    assert!(stderr(&live).contains("coppad-only"), "{}", stderr(&live));
    assert!(stderr(&live).contains(path.to_str().unwrap()));
    let wav = scratch.0.join("cq.wav");
    let wav_str = wav.to_str().unwrap();
    assert_ok(&run(
        station_command(&scratch),
        &["tx", "hi", "-o", wav_str],
    ));
    assert_ok(&run(station_command(&scratch), &["rx", wav_str]));
    let tone = scratch.0.join("tone.wav");
    assert_ok(&run(
        station_command(&scratch),
        &["tune", "--seconds", "0.01", "-o", tone.to_str().unwrap()],
    ));
}

#[test]
fn selected_config_is_not_merged_with_search_result() {
    let scratch = Scratch::new();
    std::fs::write(config_path(&scratch), "[engine]\nprofile = \"VHF_FAST\"\n").unwrap();
    let explicit = scratch.0.join("station.toml");
    std::fs::write(&explicit, "[engine]\nprofile = \"HF_STANDARD\"\n").unwrap();
    let mut command = station_command(&scratch);
    command.env("COPPA_CONFIG", "missing.toml");
    let output = run(
        command,
        &[
            "--config",
            explicit.to_str().unwrap(),
            "--verbose",
            "tx",
            "hi",
            "-o",
            scratch.0.join("cq.wav").to_str().unwrap(),
        ],
    );
    assert_ok(&output);
    assert!(stderr(&output).contains("speed_level=2"));
}
