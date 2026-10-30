//! Real-binary checks for the CLI's machine-readable output and verbosity flags.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(serde::Deserialize)]
struct DeviceOutputCheck {
    name: String,
    max_sample_rate_hz: u32,
    input_channels: u16,
    output_channels: u16,
}

fn run(args: &[&str], path: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coppa"));
    for arg in args {
        if *arg == "<path>" {
            command.arg(path.expect("missing WAV path"));
        } else {
            command.arg(arg);
        }
    }
    command.output().unwrap()
}

fn json(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn quiet_verbose_conflict_exits_before_output() {
    for args in [
        vec!["-q", "-v", "devices"],
        vec!["--quiet", "--verbose", "devices"],
        vec!["-qvv", "devices"],
    ] {
        let output = run(&args, None);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
    }
}

#[test]
fn devices_json_is_one_array_with_typed_records() {
    let output = run(&["devices", "--json"], None);
    let value = json(&output);
    let typed: Vec<DeviceOutputCheck> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(typed.len(), value.as_array().unwrap().len());
    for (device, value) in typed.iter().zip(value.as_array().unwrap()) {
        assert_eq!(value["name"], device.name);
        assert_eq!(value["max_sample_rate_hz"], device.max_sample_rate_hz);
        assert_eq!(value["input_channels"], device.input_channels);
        assert_eq!(value["output_channels"], device.output_channels);
    }
    for device in value.as_array().unwrap() {
        let object = device.as_object().unwrap();
        assert_eq!(object.len(), 5);
        assert!(object["name"].is_string());
        // COP-64: the exact selector ID, or null when the host gives none.
        assert!(object["id"].is_string() || object["id"].is_null());
        assert!(object["max_sample_rate_hz"].is_u64());
        assert!(object["input_channels"].is_u64());
        assert!(object["output_channels"].is_u64());
    }
    assert_eq!(output.stdout.last(), Some(&b'\n'));
}

#[cfg(feature = "file-backend")]
#[test]
fn rx_json_decodes_a_wav_from_the_tx_command() {
    let wav = TempWav::new("tx_command");
    let tx = run(&["tx", "COP-58 test payload", "-o", "<path>"], Some(&wav.0));
    assert!(
        tx.status.success(),
        "{}",
        String::from_utf8_lossy(&tx.stderr)
    );
    let rx = run(&["rx", "<path>", "--json"], Some(&wav.0));
    let lines = json_lines(&rx);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["payload_text"], "COP-58 test payload");
    assert_eq!(
        lines[0]["payload_hex"],
        "434f502d35382074657374207061796c6f6164"
    );
    assert!(rx.stderr.is_empty());
}

#[test]
fn profiles_help_list_detail_and_error() {
    let help = run(&["--help"], None);
    assert!(String::from_utf8_lossy(&help.stdout).contains("profiles"));

    let text = run(&["profiles"], None);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    let names = ["HF_ROBUST", "HF_STANDARD", "VHF_FAST", "EMERGENCY"];
    let mut offset = 0;
    for name in names {
        let found = text[offset..].find(name).unwrap();
        offset += found + name.len();
    }

    let output = run(&["profiles", "--json"], None);
    let records = json(&output);
    let records = records.as_array().unwrap();
    assert_eq!(records.len(), 4);
    for (record, name) in records.iter().zip(names) {
        assert_eq!(record["name"], name);
        assert_eq!(record.as_object().unwrap().len(), 7);
        assert!(record["description"].is_string());
        assert!(record["speed_level"].is_u64());
        assert!(record["max_payload_bytes"].is_u64());
        assert!(record["compression"].is_boolean());
        assert_eq!(record["sample_rate_hz"], 48_000);
        assert!(record["ofdm_profile"].is_string());
    }
    assert_eq!(records[0]["max_payload_bytes"], 56);
    assert_eq!(records[1]["max_payload_bytes"], 117);
    assert_eq!(records[2]["max_payload_bytes"], 158);

    let detail = run(&["profiles", "-p", "hf_robust", "--json"], None);
    assert_eq!(json(&detail), records[0]);
    let detail_text = run(&["profiles", "-p", "HF_ROBUST"], None);
    assert!(String::from_utf8_lossy(&detail_text.stdout).contains("Max bytes per frame: 56"));

    let unknown = run(&["profiles", "-p", "BOGUS", "--json"], None);
    assert_eq!(unknown.status.code(), Some(1));
    assert!(unknown.stdout.is_empty());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains(
        "Unknown profile: BOGUS. Available: HF_ROBUST, HF_STANDARD, VHF_FAST, EMERGENCY"
    ));

    // `config` is now a deprecated alias for `profiles` (COP-55).
    let config = run(&["config"], None);
    assert!(config.status.success());
    let plain = run(&["profiles"], None);
    assert_eq!(config.stdout, plain.stdout);
}

#[cfg(feature = "file-backend")]
struct TempWav(PathBuf);

#[cfg(feature = "file-backend")]
impl TempWav {
    fn new(name: &str) -> Self {
        Self(std::env::temp_dir().join(format!("coppa_cop58_{}_{name}.wav", std::process::id())))
    }
}

#[cfg(feature = "file-backend")]
impl Drop for TempWav {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

#[cfg(feature = "file-backend")]
fn write_wav(messages: &[&str]) -> TempWav {
    use coppa_audio::AudioSink;
    let wav = TempWav::new("frames");
    let core = coppa_engine::CoppaCore::new();
    let mut samples = Vec::new();
    for message in messages {
        samples.extend(core.encode(message).unwrap());
        samples.extend(std::iter::repeat_n(0.0f32, 48_000));
    }
    let mut sink = coppa_audio::file_backend::WavSink::new(&wav.0, 48_000);
    sink.start().unwrap();
    sink.write(&samples).unwrap();
    sink.stop().unwrap();
    wav
}

#[cfg(feature = "file-backend")]
fn json_lines(output: &Output) -> Vec<serde_json::Value> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[cfg(feature = "file-backend")]
#[test]
fn rx_json_streams_frames_in_order_and_listen_matches() {
    let wav = write_wav(&["FIRST FRAME", "SECOND FRAME"]);
    let rx = run(&["rx", "<path>", "--json"], Some(&wav.0));
    let lines = json_lines(&rx);
    assert_eq!(lines.len(), 2);
    for (line, message) in lines.iter().zip(["FIRST FRAME", "SECOND FRAME"]) {
        assert_eq!(line["payload_text"], message);
        assert_eq!(
            line["payload_hex"],
            message
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        assert!(line["snr_db"].is_number());
        assert!(line["cfo_hz"].is_number());
        assert!(line["frame_start"].is_u64());
        assert_eq!(line["speed_level"], 1);
        assert_eq!(line.as_object().unwrap().len(), 6);
    }
    let listen = run(&["listen", "<path>", "--json"], Some(&wav.0));
    assert_eq!(rx.stdout, listen.stdout);
    assert!(String::from_utf8_lossy(&listen.stderr).contains("deprecated"));
    let quiet_listen = run(&["-q", "listen", "<path>", "--json"], Some(&wav.0));
    assert_eq!(rx.stdout, quiet_listen.stdout);
    assert!(String::from_utf8_lossy(&quiet_listen.stderr).contains("deprecated"));

    let quiet = run(&["-q", "rx", "<path>", "--json"], Some(&wav.0));
    assert_eq!(json_lines(&quiet), lines);
    let verbose = run(&["-vv", "rx", "<path>", "--json"], Some(&wav.0));
    assert_eq!(json_lines(&verbose), lines);
    let diagnostics = String::from_utf8_lossy(&verbose.stderr);
    assert!(diagnostics.contains("recommended_level="));
    assert!(diagnostics.contains("delay_spread_ms="));
    assert!(!String::from_utf8_lossy(&verbose.stdout).contains("[verbose]"));
}

#[cfg(feature = "file-backend")]
#[test]
fn rx_json_handles_binary_and_rejects_other_modes() {
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/golden/L1_clean.wav");
    let output = run(&["rx", "<path>", "--json"], Some(&golden));
    let lines = json_lines(&output);
    assert_eq!(lines.len(), 1);
    assert!(lines[0]["payload_text"].is_null());
    assert_eq!(lines[0]["payload_hex"], "8639c483ddec99e970b1e47ee64369311c85fea33a653d6a95d6d17d91cb927c367ad7bdc343f8b21ee1658fd19f047eb1fe4c7a1c1834b1");

    for mode in ["--raw", "--hex"] {
        let conflict = run(&["rx", "<path>", "--json", mode], Some(&golden));
        assert_eq!(conflict.status.code(), Some(2));
        assert!(conflict.stdout.is_empty());
    }
}

/// A recording with no frame writes no JSON. Since COP-54 it is also a
/// failure (a script must not see success for a file that decoded
/// nothing), reported on stderr only, so stdout stays empty.
#[cfg(feature = "file-backend")]
#[test]
fn empty_recording_has_empty_json_stdout() {
    use coppa_audio::AudioSink;
    let wav = TempWav::new("empty");
    let mut sink = coppa_audio::file_backend::WavSink::new(&wav.0, 48_000);
    sink.start().unwrap();
    sink.write(&vec![0.0f32; 48_000]).unwrap();
    sink.stop().unwrap();
    let output = run(&["rx", "<path>", "--json"], Some(&wav.0));
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no frames decoded"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(feature = "file-backend")]
#[test]
fn missing_wav_has_no_json_stdout() {
    let missing = TempWav::new("missing");
    let output = run(&["rx", "<path>", "--json"], Some(&missing.0));
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Error:"));
}
