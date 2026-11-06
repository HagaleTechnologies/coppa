//! Offline COP-116 acceptance scenario. No Direwolf, radio or network is needed.
use coppa_codec::g3ruh;
use std::path::PathBuf;
use std::process::Command;

const FIXTURE_SHA256: &str = "11487f60768c86e6eb841fab197016232f85f0447565806eab2fcff28b7d80e5";
/// `direwolf -c direwolf.conf -t 0`, channel-0 raw PCM on stdin, MODEM 9600.
/// Verbatim output and config: docs/testing/tnc-direwolf-g3ruh-9600.md.
const DIREWOLF_FRAMES: usize = 100;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/g3ruh/g3ruh_9600_100frames_10db.wav")
}

#[test]
fn fixture_sha256_is_pinned() {
    // Use the host hash utility, keeping the workspace free of a hash dependency.
    let mut command = if cfg!(target_os = "macos") {
        let mut cmd = Command::new("shasum");
        cmd.args(["-a", "256"]);
        cmd
    } else if cfg!(windows) {
        let mut cmd = Command::new("certutil");
        cmd.arg("-hashfile");
        cmd
    } else {
        Command::new("sha256sum")
    };
    command.arg(fixture());
    if cfg!(windows) {
        command.arg("SHA256");
    }
    let output = command
        .output()
        .expect("host SHA-256 utility must be installed");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.split_whitespace().any(|s| s.eq_ignore_ascii_case(FIXTURE_SHA256)),
        "fixture changed; regenerate deliberately via tools/tnc-bench/gen_g3ruh_9600.sh, never hand-edit: {text}");
}

#[test]
fn delivers_at_least_95_percent_of_direwolf() {
    let mut reader = hound::WavReader::open(fixture()).unwrap();
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect();
    let mut demod = g3ruh::Demodulator::new();
    let mut count = 0;
    for chunk in samples.chunks(1024) {
        demod.process(chunk);
        count += demod.take_frames().len();
    }
    assert!(
        count * 100 >= 95 * DIREWOLF_FRAMES,
        "coppa={count}, Direwolf={DIREWOLF_FRAMES}"
    );
    assert!(count <= 200, "duplicate delivery: {count}");
}

#[test]
fn fixture_is_mono_16bit_48k() {
    let reader = hound::WavReader::open(fixture()).unwrap();
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 48000);
    assert_eq!(spec.channels, 1);
    assert_eq!(spec.bits_per_sample, 16);
    assert_eq!(spec.sample_format, hound::SampleFormat::Int);
    assert!((5 * 48000..=15 * 48000).contains(&reader.duration()));
}
