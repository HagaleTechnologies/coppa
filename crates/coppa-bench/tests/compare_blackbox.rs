//! Live black-box tests. Run with a release coppad and `--ignored --test-threads 1`.
#![cfg(all(feature = "cpal-backend", target_os = "linux"))]

use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

fn profile(name: &str) -> PathBuf {
    let root = coppa_bench::compare::repo_root();
    let binary = std::env::var_os("COPPAD")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target/release/coppad"));
    assert!(binary.exists(), "build a release coppad first: cargo build --release -p coppa-daemon --bin coppad --features cpal-backend,ardop,kiss-tnc");
    let source = fs::read_to_string(root.join(format!("tools/compare/{name}.toml"))).unwrap();
    let source = source.replace("target/release/coppad", &binary.display().to_string());
    let dir = std::env::temp_dir().join(format!("coppa-live-test-{}-{name}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("profile.toml");
    fs::write(&path, source).unwrap();
    path
}

fn run(name: &str) -> (String, String) {
    let p = profile(name);
    let out_dir = p.parent().unwrap().join("results");
    let output = Command::new(env!("CARGO_BIN_EXE_coppa-bench"))
        .args(["compare", "blackbox", "--modem"])
        .arg(&p)
        .args([
            "--channel",
            "wgn",
            "--snr",
            "30",
            "--size",
            "2048",
            "--trials",
            "1",
            "--out-dir",
        ])
        .arg(&out_dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "black-box run failed: {stderr}");
    let csv = fs::read_to_string(out_dir.join(format!("blackbox-{name}.csv"))).unwrap();
    (csv, stderr)
}

#[test]
#[ignore = "requires release coppad, ALSA file PCM, and real time"]
fn cable_integrity_probe() {
    let (_, log) = run("coppad");
    assert!(
        log.contains("integrity probe: correlation 1.000000"),
        "{log}"
    );
    assert!(log.contains("pass"), "{log}");
}

/// COP-221 AC1: a profile declaring s16le at 12000 Hz mono gets a cable
/// whose integrity probe, opened in that format, passes the AC10 gate. The
/// modem is `/bin/true`, so the run stops at calibration, after the probe.
/// COP-256 AC4: the reported level also stays within 0.1 dB of unity.
#[test]
#[ignore = "requires ALSA file PCM and real time"]
fn native_s16le_12k_integrity_probe() {
    let root = coppa_bench::compare::repo_root();
    let source = fs::read_to_string(root.join("tools/compare/coppad.toml")).unwrap();
    let source = source
        .replace("name = \"coppad\"", "name = \"native-s16le-12k\"")
        .replace("target/release/coppad", "/bin/true")
        .replace(
            "format = \"f32le\"\nrate = 48000",
            "format = \"s16le\"\nrate = 12000",
        );
    assert!(
        source.contains("rate = 12000"),
        "coppad.toml lost its [audio] table"
    );
    let dir = std::env::temp_dir().join(format!("coppa-live-test-{}-native", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("profile.toml");
    fs::write(&path, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_coppa-bench"))
        .args(["compare", "blackbox", "--modem"])
        .arg(&path)
        .arg("--out-dir")
        .arg(dir.join("results"))
        .output()
        .unwrap();
    let log = String::from_utf8(output.stderr).unwrap();
    let _ = fs::remove_dir_all(&dir);
    // `gated` prints this line only when the AC10 thresholds held.
    assert!(log.contains("integrity probe: correlation"), "{log}");
    assert!(log.contains(": pass"), "{log}");
    // COP-256 AC4: the probe runs at unity gain, so the level it reports is
    // the cable's own, within 0.1 dB of unity.
    let level = log
        .lines()
        .find(|l| l.contains("integrity probe:"))
        .and_then(|l| l.split("level ").nth(1))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.trim_end_matches("dB").parse::<f64>().ok())
        .unwrap_or_else(|| panic!("no reported level in {log}"));
    assert!(level.abs() <= 0.1, "level {level} dB: {log}");
}

/// COP-221 AC2: a profile with no `[audio]` table, or a format the cable
/// cannot carry, stops the harness at start with exit 1, naming the key and
/// the values it accepts.
#[test]
fn undeclared_or_unsupported_audio_exits_1_naming_the_key() {
    let root = coppa_bench::compare::repo_root();
    let source = fs::read_to_string(root.join("tools/compare/coppad.toml")).unwrap();
    let dir = std::env::temp_dir().join(format!("coppa-audio-key-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let start = source.find("\n[audio]\nformat").unwrap();
    let end = start + 1 + source[start + 1..].find("\n\n").unwrap();
    for (case, audio, expected) in [
        (
            "undeclared",
            String::new(),
            "audio.format is undeclared; supported values: f32le, s16le",
        ),
        (
            "unsupported",
            "\n[audio]\nformat = \"s24le\"\nrate = 48000\nchannels = 1".to_string(),
            "audio.format \"s24le\" is unsupported; supported values: f32le, s16le",
        ),
    ] {
        let path = dir.join(format!("{case}.toml"));
        fs::write(
            &path,
            format!("{}{}{}", &source[..start], audio, &source[end..]),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_coppa-bench"))
            .args(["compare", "blackbox", "--modem"])
            .arg(&path)
            .arg("--out-dir")
            .arg(dir.join("results"))
            .output()
            .unwrap();
        let log = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(1), "{case}: {log}");
        assert!(log.contains(expected), "{case}: {log}");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "requires release coppad, ALSA file PCM, and real time"]
fn coppad_vara_transfer_through_cable() {
    let (csv, _) = run("coppad");
    assert!(csv.contains(",complete,"), "{csv}");
    assert!(csv.contains(",2048,true,"), "{csv}");
}

#[test]
#[ignore = "requires release coppad with ardop, ALSA file PCM, and real time"]
fn coppad_ardop_transfer_through_cable() {
    let (csv, _) = run("coppad-ardop");
    assert!(csv.contains(",complete,"), "{csv}");
    assert!(csv.contains(",2048,true,"), "{csv}");
}

#[test]
#[ignore = "requires release coppad with kiss-tnc, ALSA file PCM, and real time"]
fn coppad_kiss_datagrams_through_cable() {
    let (csv, log) = run("coppad-kiss");
    // COP-214 Phase 3: the profile's datagram fits every KISS interface
    // coppad reports. This runs before the CSV asserts, because an oversized
    // profile still records outcome `datagram`, with exact=false.
    let kiss = coppa_bench::compare::blackbox::profile::Profile::load(
        &coppa_bench::compare::repo_root().join("tools/compare/coppad-kiss.toml"),
    )
    .unwrap();
    let datagram_bytes = kiss.datagram_bytes.unwrap();
    let max: Vec<usize> = log
        .split("max_datagram_bytes=")
        .skip(1)
        .map(|rest| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits
                .parse()
                .unwrap_or_else(|e| panic!("max_datagram_bytes={rest:.20}: {e}"))
        })
        .collect();
    assert!(
        !max.is_empty(),
        "coppad logged no max_datagram_bytes: {log}"
    );
    assert!(
        max.iter().all(|&m| datagram_bytes <= m),
        "datagram_bytes {datagram_bytes} exceeds a logged max_datagram_bytes {max:?}"
    );
    assert!(csv.contains(",datagram,"), "{csv}");
    assert!(
        csv.contains(",2048,true,"),
        "KISS trial delivered less than the payload. A KISS datagram is never \
         resent; one measured cause is coppad's [audio] buffer_size overflowing \
         during a decode on a CPU-contended host (COP-244, docs/LIMITATIONS.md): {csv}"
    );
}

/// COP-227 (AC4): a resume into a CSV from before the `payload_crc32` column
/// is refused before the probe or any modem starts, and the file is left as
/// it was. Needs no coppad: the profile names this test's own binary, which
/// hashes but is never launched.
#[test]
fn resume_into_a_pre_payload_csv_is_refused_before_any_modem_starts() {
    let root = coppa_bench::compare::repo_root();
    let dir = std::env::temp_dir().join(format!("coppa-cop227-legacy-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let out_dir = dir.join("out");
    fs::create_dir_all(&out_dir).unwrap();
    let source = fs::read_to_string(root.join("tools/compare/coppad.toml")).unwrap();
    let source = source.replace("target/release/coppad", env!("CARGO_BIN_EXE_coppa-bench"));
    let profile = dir.join("profile.toml");
    fs::write(&profile, source).unwrap();
    let legacy_header = coppa_bench::compare::blackbox::CSV_HEADER
        .strip_suffix(",capture_gain_db")
        .and_then(|h| h.strip_suffix(",payload_crc32"))
        .unwrap();
    let legacy = format!(
        "{legacy_header}\ncoppad,vara,VERSION Coppa 0.16.0,9b8b4957d0c18b1df65c23d26a97b2a855101fff94fb6902757ff0c1d6d1327e,HF_STANDARD,wgn,30,2048,0,0x17000000,0.022691,0.020689,600,complete,7.078,2048,true,51.477,2387.090,0,58.555,355,0,0\n"
    );
    let csv = out_dir.join("blackbox-coppad.csv");
    fs::write(&csv, &legacy).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_coppa-bench"))
        .args(["compare", "blackbox", "--modem"])
        .arg(&profile)
        .arg("--out-dir")
        .arg(&out_dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("no payload_crc32 column"), "{stderr}");
    assert!(stderr.contains("use a fresh --out-dir"), "{stderr}");
    assert!(!stderr.contains("integrity probe"), "{stderr}");
    assert_eq!(fs::read_to_string(&csv).unwrap(), legacy);
    fs::remove_dir_all(&dir).unwrap();
}

/// COP-256 (AC7): a resume into a CSV from before the `capture_gain_db`
/// column is refused before the probe or any modem starts, and the file is
/// left as it was. Needs no coppad: the profile names this test's own
/// binary, which hashes but is never launched.
#[test]
fn resume_into_a_pre_gain_csv_is_refused_before_any_modem_starts() {
    let root = coppa_bench::compare::repo_root();
    let dir = std::env::temp_dir().join(format!("coppa-cop256-legacy-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let out_dir = dir.join("out");
    fs::create_dir_all(&out_dir).unwrap();
    let source = fs::read_to_string(root.join("tools/compare/coppad.toml")).unwrap();
    let source = source.replace("target/release/coppad", env!("CARGO_BIN_EXE_coppa-bench"));
    let profile = dir.join("profile.toml");
    fs::write(&profile, source).unwrap();
    let legacy_header = coppa_bench::compare::blackbox::CSV_HEADER
        .strip_suffix(",capture_gain_db")
        .unwrap();
    assert_eq!(legacy_header.split(',').count(), 25);
    assert!(legacy_header.ends_with(",payload_crc32"));
    let legacy = format!(
        "{legacy_header}\ncoppad,vara,VERSION Coppa 0.16.0,9b8b4957d0c18b1df65c23d26a97b2a855101fff94fb6902757ff0c1d6d1327e,HF_STANDARD,wgn,30,2048,0,0x17000000,0.022691,0.020689,600,complete,7.078,2048,true,51.477,2387.090,0,58.555,355,0,0,0x6887c6a5\n"
    );
    let csv = out_dir.join("blackbox-coppad.csv");
    fs::write(&csv, &legacy).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_coppa-bench"))
        .args(["compare", "blackbox", "--modem"])
        .arg(&profile)
        .arg("--out-dir")
        .arg(&out_dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("no capture_gain_db column"), "{stderr}");
    assert!(stderr.contains("use a fresh --out-dir"), "{stderr}");
    assert!(!stderr.contains("integrity probe"), "{stderr}");
    assert!(!stderr.contains("modem ready"), "{stderr}");
    assert_eq!(fs::read_to_string(&csv).unwrap(), legacy);
    fs::remove_dir_all(&dir).unwrap();
}

/// The COP-256 scripted station: the profile's modem binary is this test
/// binary itself, re-executed with one exact filter, so the harness
/// launches an unmodified process that speaks the VARA command exchanges
/// and plays finite PCM into the production FIFOs. The outer tests set
/// [`FIXTURE_ENV`] for the `coppa-bench` child; the station inherits it, so
/// the fixture body only runs when re-executed, never inside the suite.
const FIXTURE_ENV: &str = "COPPA_S16_FIXTURE";
/// Where station B of a trial writes its capture summary.
const SUMMARY_ENV: &str = "COPPA_S16_FIXTURE_SUMMARY";
/// COPPA_GIT_SHA is irrelevant here; this value only labels the reply.
const FIXTURE_VERSION_REPLY: &str = "VERSION s16-fixture 1";

#[test]
fn s16le_station_fixture() {
    if std::env::var_os(FIXTURE_ENV).is_none() {
        return;
    }
    station_main();
}

/// The station process. HOME is `{cable}/home-A` or `{cable}/home-B` inside
/// the harness's cable directory, so the parent gives the FIFO paths and
/// the directory name gives the phase: `calibration` or `trial-N`.
fn station_main() {
    let home = PathBuf::from(std::env::var_os("HOME").expect("station HOME"));
    let side = if home.file_name().and_then(|n| n.to_str()) == Some("home-A") {
        'A'
    } else {
        'B'
    };
    let cable = home.parent().expect("station cable dir").to_path_buf();
    let trial = cable
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("trial-"));

    let command = TcpListener::bind("127.0.0.1:0").expect("bind command port");
    let data = TcpListener::bind("127.0.0.1:0").expect("bind data port");
    {
        let mut out = std::io::stdout().lock();
        writeln!(
            out,
            "test station ready command_addr={} data_addr={}",
            command.local_addr().unwrap(),
            data.local_addr().unwrap()
        )
        .expect("ready line");
        out.flush().expect("flush ready line");
    }

    // A trial station drains its capture FIFO the whole time it runs, so
    // the relay's writes never fill the pipe; B also measures what it
    // drained for the outer test.
    if trial {
        let capture_path = if side == 'A' {
            cable.join("ba_out")
        } else {
            cable.join("ab_out")
        };
        let summary = (side == 'B')
            .then(|| std::env::var_os(SUMMARY_ENV).map(PathBuf::from))
            .flatten();
        std::thread::spawn(move || drain_capture(&capture_path, summary.as_deref()));
    }

    let deadline = Instant::now() + Duration::from_secs(120);
    let mut cmd = accept_bounded(&command, deadline);
    let mut data_stream = accept_bounded(&data, deadline);
    let _ = &mut data_stream;
    cmd.set_read_timeout(Some(Duration::from_millis(200)))
        .expect("command read timeout");
    let mut pending = Vec::new();
    while Instant::now() < deadline {
        let mut bytes = [0u8; 256];
        match cmd.read(&mut bytes) {
            Ok(0) => break,
            Ok(n) => pending.extend_from_slice(&bytes[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
        while let Some(pos) = pending.iter().position(|&b| b == b'\r' || b == b'\n') {
            let line: String = String::from_utf8_lossy(&pending[..pos]).trim().to_string();
            pending.drain(..=pos);
            match line.as_str() {
                "VERSION" => reply(&mut cmd, FIXTURE_VERSION_REPLY),
                l if l.starts_with("MYCALL") => reply(&mut cmd, "OK"),
                "LISTEN ON" => reply(&mut cmd, "OK"),
                l if l.starts_with("CONNECT") => {
                    if trial {
                        // AC1/AC3: station A plays the fixture waveform on
                        // its CONNECT request, then the dial ends in WRONG,
                        // which the adapter reads as a dial failure.
                        if side == 'A' {
                            play_waveform(&cable);
                        }
                        reply(&mut cmd, "WRONG");
                    } else {
                        // Calibration: the dial succeeds, so the run with a
                        // pinned --ref-power proceeds to the trial.
                        reply(&mut cmd, "OK");
                        reply(&mut cmd, "CONNECTED N0AAA N0BBB 2300");
                    }
                }
                _ => {}
            }
        }
    }
}

fn accept_bounded(listener: &TcpListener, deadline: Instant) -> std::net::TcpStream {
    listener.set_nonblocking(true).expect("listener mode");
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).ok();
                return stream;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "no station connection arrived");
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("station accept: {e}"),
        }
    }
}

fn reply(stream: &mut std::net::TcpStream, line: &str) {
    stream
        .write_all(format!("{line}\r").as_bytes())
        .expect("station reply");
}

/// Station A's playback, straight into the FIFO the ALSA file PCM would
/// write: 350 native 20 ms ticks of the planning five-tone waveform, or a
/// near-full-scale 1 kHz cosine for the residual-clipping case.
fn play_waveform(cable: &Path) {
    use coppa_bench::compare::blackbox::native::{to_i16, NativeFormat};
    use std::os::unix::fs::OpenOptionsExt;
    const O_NONBLOCK: i32 = 0o4000;

    let native = NativeFormat::declared(Some("s16le"), Some(12_000), Some(1)).unwrap();
    let loud = std::env::var(FIXTURE_ENV).as_deref() == Ok("loud");
    let (samples, deadline) = if loud {
        // Five seconds of near-full-scale playback, bounded by a write
        // window shorter than that: the relay either refuses a block
        // within the first loud ticks or the test fails on its exit code.
        let samples: Vec<f32> = (0..5 * native.rate)
            .map(|n| 0.99 * (std::f32::consts::TAU * 1000.0 * n as f32 / 12_000.0).cos())
            .collect();
        (samples, Duration::from_millis(2500))
    } else {
        let samples: Vec<f32> = (0..350 * native.block())
            .map(|n| {
                [350.0, 700.0, 1050.0, 1400.0, 1750.0]
                    .iter()
                    .map(|&f| 0.1 * (std::f32::consts::TAU * f * n as f32 / 12_000.0).cos())
                    .sum()
            })
            .collect();
        (samples, Duration::from_secs(30))
    };
    let path = cable.join("ab_in");
    let Ok(mut file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(O_NONBLOCK)
        .open(&path)
    else {
        return;
    };
    let bytes: Vec<u8> = samples
        .iter()
        .flat_map(|&s| to_i16(s).to_le_bytes())
        .collect();
    let end = Instant::now() + deadline;
    let mut written = 0;
    while written < bytes.len() && Instant::now() < end {
        match file.write(&bytes[written..]) {
            Ok(n) => written += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return,
        }
    }
}

/// Station B drains and measures its capture FIFO, rewriting a summary the
/// outer test can read whenever the station is stopped.
fn drain_capture(path: &Path, summary_dir: Option<&Path>) {
    use std::os::unix::fs::OpenOptionsExt;
    const O_NONBLOCK: i32 = 0o4000;

    let Ok(mut file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(O_NONBLOCK)
        .open(path)
    else {
        return;
    };
    let end = Instant::now() + Duration::from_secs(20);
    let (mut count, mut peak) = (0usize, 0i32);
    let mut last_write = Instant::now() - Duration::from_secs(1);
    let mut bytes = [0u8; 8192];
    while Instant::now() < end {
        match file.read(&mut bytes) {
            Ok(0) => break,
            Ok(n) => {
                count += n / 2;
                for pair in bytes[..n - n % 2].chunks_exact(2) {
                    let code = i16::from_le_bytes([pair[0], pair[1]]) as i32;
                    peak = peak.max(code.abs());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => break,
        }
        if let Some(dir) = summary_dir {
            if last_write.elapsed() >= Duration::from_millis(250) {
                last_write = Instant::now();
                let text = format!("count={count}\npeak={peak}\n");
                let _ = fs::write(dir.join("b-capture.tmp"), &text);
                let _ = fs::rename(dir.join("b-capture.tmp"), dir.join("b-capture"));
            }
        }
    }
    if let Some(dir) = summary_dir {
        let text = format!("count={count}\npeak={peak}\n");
        let _ = fs::write(dir.join("b-capture.tmp"), &text);
        let _ = fs::rename(dir.join("b-capture.tmp"), dir.join("b-capture"));
    }
}

/// The scratch profile whose modem is the scripted station.
fn s16le_fixture_profile(dir: &Path) -> PathBuf {
    let binary = std::env::current_exe().expect("test binary path");
    let source = format!(
        "# COP-256 scripted s16le station: this test binary, re-executed.\n\
         name = \"s16-fixture\"\n\
         adapter = \"vara\"\n\
         binary = \"{}\"\n\
         sha256 = \"\"\n\
         mode = \"test\"\n\
         args = [\"--nocapture\", \"--exact\", \"s16le_station_fixture\"]\n\
         ready_timeout_s = 30\n\
         files = []\n\n\
         [audio]\n\
         format = \"s16le\"\n\
         rate = 12000\n\
         channels = 1\n\n\
         [ports]\n\
         log_line = \"test station ready\"\n\
         command_key = \"command_addr=\"\n\
         data_key = \"data_addr=\"\n",
        binary.display()
    );
    let path = dir.join("profile.toml");
    fs::write(&path, source).unwrap();
    path
}

fn summary_field(dir: &Path, key: &str) -> String {
    let text = fs::read_to_string(dir.join("b-capture")).unwrap_or_default();
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .unwrap_or_default()
        .to_string()
}

/// COP-256 AC1: an s16le 12000 Hz profile running a WGN 0 dB trial with the
/// published powers records a `no_connect` row with `capture_gain_db=-7`,
/// and station B's capture is nonempty and inside the s16 code rails. The
/// strict production encoder accepted every block the relay wrote.
#[test]
#[ignore = "requires ALSA file PCM and real time"]
fn native_s16le_12k_wgn_zero_db_records_a_trial() {
    use coppa_bench::compare::blackbox::score;

    let dir = std::env::temp_dir().join(format!("coppa-cop256-ac1-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let out_dir = dir.join("out");
    let summary = dir.join("summary");
    fs::create_dir_all(&summary).unwrap();
    let profile = s16le_fixture_profile(&dir);
    let output = Command::new(env!("CARGO_BIN_EXE_coppa-bench"))
        .args(["compare", "blackbox", "--modem"])
        .arg(&profile)
        .args([
            "--channel",
            "wgn",
            "--snr",
            "0",
            "--size",
            "2048",
            "--trials",
            "1",
            "--ref-power",
            "0.023498,0.020689",
        ])
        .arg("--out-dir")
        .arg(&out_dir)
        .env(FIXTURE_ENV, "tones")
        .env(SUMMARY_ENV, &summary)
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "black-box run failed: {stderr}");
    assert!(stderr.contains("integrity probe: correlation"), "{stderr}");
    assert!(stderr.contains(": pass"), "{stderr}");

    let csv = fs::read_to_string(out_dir.join("blackbox-s16-fixture.csv")).unwrap();
    let mut lines = csv.lines();
    assert_eq!(
        lines.next(),
        Some(coppa_bench::compare::blackbox::CSV_HEADER)
    );
    let rows: Vec<&str> = lines.collect();
    assert_eq!(rows.len(), 1, "{csv}");
    let fields: Vec<&str> = rows[0].split(',').collect();
    assert_eq!(fields.len(), coppa_bench::compare::blackbox::CSV_COLUMNS);
    assert_eq!(fields[0], "s16-fixture");
    assert_eq!(fields[2], FIXTURE_VERSION_REPLY);
    assert_eq!(fields[5], "wgn");
    assert_eq!(fields[6], "0");
    assert_eq!(fields[7], "2048");
    assert_eq!(fields[8], "0");
    assert_eq!(fields[9], "0x170c0000");
    assert_eq!(fields[10], "0.023498");
    assert_eq!(fields[11], "0.020689");
    assert_eq!(fields[13], "no_connect");
    assert_eq!(
        fields[24],
        score::payload_crc32(&score::trial_payload(0x170c_0000, 2048))
    );
    assert_eq!(fields[25], "-7");

    // No captured sample clipped: the relay joined cleanly and station B
    // drained codes inside the rails.
    let count = summary_field(&summary, "count");
    let peak = summary_field(&summary, "peak");
    assert!(
        count.parse::<usize>().unwrap_or(0) > 0,
        "station B captured nothing: {count:?}"
    );
    let peak = peak.parse::<i32>().unwrap_or(i32::MAX);
    assert!(
        peak <= 32767,
        "capture peak {peak} is outside the s16 rails"
    );
    fs::remove_dir_all(&dir).unwrap();
}

/// COP-256 AC3: a capture that still exceeds full scale after the derived
/// gain is refused exactly as before. A 0.006 TX power at WGN 0 dB derives
/// only -1 dB, and a near-full-scale 1 kHz playback exceeds the rails after
/// it, so the CLI exits 1 and no row is recorded.
#[test]
#[ignore = "requires ALSA file PCM and real time"]
fn native_s16le_residual_clipping_refuses_the_trial() {
    use coppa_bench::compare::blackbox::native::{capture_gain_db, NativeFormat};

    let native = NativeFormat::declared(Some("s16le"), Some(12_000), Some(1)).unwrap();
    assert_eq!(capture_gain_db(native, false, 0, [0.006, 0.006]), -1);

    let dir = std::env::temp_dir().join(format!("coppa-cop256-ac3-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let out_dir = dir.join("out");
    let summary = dir.join("summary");
    fs::create_dir_all(&summary).unwrap();
    let profile = s16le_fixture_profile(&dir);
    let output = Command::new(env!("CARGO_BIN_EXE_coppa-bench"))
        .args(["compare", "blackbox", "--modem"])
        .arg(&profile)
        .args([
            "--channel",
            "wgn",
            "--snr",
            "0",
            "--size",
            "2048",
            "--trials",
            "1",
            "--ref-power",
            "0.006,0.006",
        ])
        .arg("--out-dir")
        .arg(&out_dir)
        .env(FIXTURE_ENV, "loud")
        .env(SUMMARY_ENV, &summary)
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("s16le capture sample")
            && stderr.contains("outside the representable range"),
        "{stderr}"
    );
    assert!(stderr.contains("integrity probe: correlation"), "{stderr}");
    let csv = fs::read_to_string(out_dir.join("blackbox-s16-fixture.csv")).unwrap();
    assert!(
        csv.lines().count() == 1,
        "a refused trial recorded a row: {csv}"
    );
    assert_eq!(
        csv.lines().next(),
        Some(coppa_bench::compare::blackbox::CSV_HEADER)
    );
    fs::remove_dir_all(&dir).unwrap();
}

/// COP-256 AC5: a release coppad repeats the two published f32le cells,
/// every row at 0 dB capture gain, and each cell's fresh three-trial mean
/// lands inside its published interval. Capped, dropped and no-connect rows
/// count, as the renderer counts them. One retry in a fresh out-dir is
/// allowed only for an integrity-probe failure before any trial ran.
#[test]
#[ignore = "requires a release coppad, ALSA file PCM, and about 35 minutes of real time"]
fn coppad_f32le_selftest_reproduces_published_intervals() {
    let cells = [("wgn", 30, 0.0, 4899.0), ("mpg", 20, 0.0, 246.0)];
    let handles: Vec<_> = cells
        .iter()
        .map(|&(channel, snr, lo, hi)| {
            std::thread::spawn(move || replay_published_cell(channel, snr, lo, hi))
        })
        .collect();
    for handle in handles {
        handle.join().expect("cell replay panicked");
    }
}

fn replay_published_cell(channel: &str, snr: i32, lo: f64, hi: f64) {
    use coppa_bench::compare::blackbox::score;
    use coppa_bench::compare::grid::{Cell, Channel, SEED_BASE};

    let root = coppa_bench::compare::repo_root();
    let binary = root.join("target/release/coppad");
    assert!(
        binary.exists(),
        "build a release coppad first: cargo build --release -p coppa-daemon --bin coppad --features cpal-backend,ardop,kiss-tnc"
    );
    let source = fs::read_to_string(root.join("tools/compare/coppad.toml"))
        .unwrap()
        .replace("target/release/coppad", &binary.display().to_string());
    let dir = std::env::temp_dir().join(format!(
        "coppa-cop256-replay-{channel}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let profile = dir.join("profile.toml");
    fs::write(&profile, source).unwrap();

    for attempt in 0..2 {
        let out_dir = dir.join(if attempt == 0 { "out" } else { "out-retry" });
        let output = Command::new(env!("CARGO_BIN_EXE_coppa-bench"))
            .args(["compare", "blackbox", "--modem"])
            .arg(&profile)
            .args([
                "--channel",
                channel,
                "--snr",
                &snr.to_string(),
                "--size",
                "2048",
                "--trials",
                "3",
                "--jobs",
                "1",
            ])
            .arg("--out-dir")
            .arg(&out_dir)
            .output()
            .unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        if !output.status.success() {
            let ran_trials = stderr.contains("compare blackbox: Wgn")
                || stderr.contains("compare blackbox: Mpg");
            assert!(
                attempt == 0 && !ran_trials && stderr.contains("integrity probe"),
                "black-box replay failed: {stderr}"
            );
            // Integrity-probe flake before any trial: retry once in a new
            // output directory, never on throughput.
            continue;
        }
        let csv = fs::read_to_string(out_dir.join("blackbox-coppad.csv")).unwrap();
        let mut lines = csv.lines();
        assert_eq!(
            lines.next(),
            Some(coppa_bench::compare::blackbox::CSV_HEADER)
        );
        let rows: Vec<Vec<&str>> = lines.map(|l| l.split(',').collect()).collect();
        assert_eq!(rows.len(), 3, "{csv}");
        let grid_channel = Channel::parse(channel).unwrap();
        let cell = Cell {
            channel: grid_channel,
            snr_db: snr,
            size_bytes: 2048,
        };
        let mut trials: Vec<usize> = rows.iter().map(|r| r[8].parse().unwrap()).collect();
        trials.sort_unstable();
        assert_eq!(trials, vec![0, 1, 2], "{csv}");
        let mut rates = Vec::new();
        for row in &rows {
            assert_eq!(row.len(), 26, "{csv}");
            assert_eq!(row[25], "0", "an f32le row recorded a gain: {row:?}");
            let trial: usize = row[8].parse().unwrap();
            assert_eq!(
                row[9],
                format!("0x{:x}", cell.seed(SEED_BASE, trial).unwrap()),
                "{csv}"
            );
            assert_eq!(
                row[24],
                score::payload_crc32(&score::trial_payload(
                    cell.seed(SEED_BASE, trial).unwrap(),
                    2048
                )),
                "{csv}"
            );
            let exact: bool = row[16].parse().unwrap();
            assert_eq!(
                row[13] == "complete",
                exact,
                "complete must mean exact: {row:?}"
            );
            rates.push(row[18].parse::<f64>().unwrap_or(0.0));
        }
        let mean: f64 = rates.iter().sum::<f64>() / rates.len() as f64;
        assert!(
            (lo..=hi).contains(&mean),
            "{channel} {snr} dB mean {mean} net bytes/min is outside the published interval {lo}-{hi}"
        );
        let completed = rows.iter().filter(|r| r[13] == "complete").count();
        println!("{channel} {snr} dB: {completed}/3 completed, mean {mean:.1} net B/min");
        let _ = fs::remove_dir_all(&dir);
        return;
    }
    panic!("both {channel} {snr} dB replay attempts failed the integrity probe");
}
