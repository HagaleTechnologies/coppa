//! Process-level receive status and stdout/stderr contracts.
#![cfg(feature = "file-backend")]

use coppa_audio::AudioSink;
use coppa_engine::{config::EngineConfig, profiles::get_profile, CoppaCore};
use std::path::PathBuf;
use std::process::{Command, Output};

struct TempWav(PathBuf);

impl TempWav {
    fn new(name: &str, samples: &[f32], rate: u32) -> Self {
        let path =
            std::env::temp_dir().join(format!("coppa_cop54_{}_{}.wav", std::process::id(), name));
        let mut sink = coppa_audio::file_backend::WavSink::new(&path, rate);
        sink.start().unwrap();
        sink.write(samples).unwrap();
        sink.stop().unwrap();
        Self(path)
    }
}

impl Drop for TempWav {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

fn rx(wav: &TempWav, before: &[&str], after: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_coppa"))
        .args(before)
        .arg("rx")
        .arg(&wav.0)
        .args(after)
        .output()
        .unwrap()
}

fn failed(output: Output, expected: &str) {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(expected), "{stderr}");
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("Decoded"),
        "{output:?}"
    );
}

#[test]
fn wrong_rate_header_fails_before_reading() {
    let wav = TempWav::new("wrong_rate", &vec![0.0; 44_100], 44_100);
    let output = rx(&wav, &[], &[]);
    assert!(output.stdout.is_empty());
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("44100 Hz") && stderr.contains("48000 Hz") && stderr.contains("resample"),
        "{stderr}"
    );
}

#[test]
fn silence_noise_and_incomplete_frames_report_zero_frames() {
    let silence = TempWav::new("silence", &vec![0.0; 48_000], 48_000);
    for (before, command, after) in [
        (&[][..], "rx", &[][..]),
        (&["--quiet"][..], "rx", &[][..]),
        (&[][..], "rx", &["--raw"][..]),
        (&[][..], "listen", &[][..]),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_coppa"))
            .args(before)
            .arg(command)
            .arg(&silence.0)
            .args(after)
            .output()
            .unwrap();
        failed(output, "no frames decoded");
    }
    let noise: Vec<f32> = (0..48_000)
        .map(|n| ((n * 17 % 101) as f32 - 50.0) / 10_000.0)
        .collect();
    failed(
        rx(&TempWav::new("noise", &noise, 48_000), &[], &[]),
        "no frames decoded",
    );
    let samples = CoppaCore::new().encode("CQ CQ DE W5AU").unwrap();
    failed(
        rx(
            &TempWav::new("partial", &samples[..10_000], 48_000),
            &[],
            &[],
        ),
        "no frames decoded",
    );
}

#[test]
fn compressed_profile_mismatch_withholds_payload() {
    let profile = get_profile("HF_STANDARD").unwrap();
    let tx = CoppaCore::with_config(EngineConfig::from_profile(profile));
    let wav = TempWav::new("compressed", &tx.encode("CQ CQ DE W5AU").unwrap(), 48_000);
    let bad = rx(&wav, &[], &[]);
    assert_eq!(bad.status.code(), Some(1), "{bad:?}");
    assert!(
        !String::from_utf8_lossy(&bad.stdout).contains("fe0a"),
        "{bad:?}"
    );
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("appears compressed"),
        "{bad:?}"
    );
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("profile default"),
        "{bad:?}"
    );

    let bad_raw = rx(&wav, &["--quiet"], &["--raw"]);
    assert_eq!(bad_raw.status.code(), Some(1), "{bad_raw:?}");
    assert!(bad_raw.stdout.is_empty(), "{bad_raw:?}");
    assert!(
        String::from_utf8_lossy(&bad_raw.stderr).contains("appears compressed"),
        "{bad_raw:?}"
    );

    let good = rx(&wav, &[], &["--profile", "HF_STANDARD"]);
    assert!(good.status.success(), "{good:?}");
    assert!(
        String::from_utf8_lossy(&good.stdout).contains("CQ CQ DE W5AU"),
        "{good:?}"
    );

    let vhf = rx(&wav, &[], &["--profile", "VHF_FAST"]);
    failed(vhf, "profile VHF_FAST");
}

#[test]
fn raw_marker_payload_is_valid_unless_decompression_is_enabled() {
    let bytes = [0xfe, 1, 2, 3, 4, 5];
    let samples = CoppaCore::new().encode_bytes(&bytes).unwrap();
    let wav = TempWav::new("raw_marker", &samples, 48_000);
    let good = rx(&wav, &[], &["--raw"]);
    assert!(good.status.success(), "{good:?}");
    assert_eq!(String::from_utf8_lossy(&good.stdout).trim(), "fe0102030405");
    let bad = rx(&wav, &["--quiet"], &["--profile", "HF_STANDARD"]);
    assert_eq!(bad.status.code(), Some(1), "{bad:?}");
    assert!(bad.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("payload decode failed"),
        "{bad:?}"
    );
}

#[test]
fn good_frame_followed_by_bad_payload_reports_failure_and_keeps_good_output() {
    let good = CoppaCore::new().encode("GOOD FRAME").unwrap();
    let profile = get_profile("HF_STANDARD").unwrap();
    let bad = CoppaCore::with_config(EngineConfig::from_profile(profile))
        .encode("BAD FRAME")
        .unwrap();
    let mut samples = good;
    samples.extend(std::iter::repeat_n(0.0, 48_000));
    samples.extend(bad);
    let wav = TempWav::new("mixed", &samples, 48_000);
    let output = rx(&wav, &[], &[]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("GOOD FRAME") && !stdout.contains("BAD FRAME") && !stdout.contains("fe0"),
        "{output:?}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("appears compressed"),
        "{output:?}"
    );
}
