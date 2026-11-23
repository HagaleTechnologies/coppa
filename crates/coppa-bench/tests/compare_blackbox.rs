//! Live black-box tests. Run with a release coppad and `--ignored --test-threads 1`.
#![cfg(all(feature = "cpal-backend", target_os = "linux"))]

use std::{fs, path::PathBuf, process::Command};

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
    assert!(csv.contains(",2048,true,"), "{csv}");
}
