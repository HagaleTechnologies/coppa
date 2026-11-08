//! Integration test for Phase 4 Task 4: `coppa rx --input <wav>` (the
//! `StreamingReceiver`-based streaming path, `cmd_rx`/`stream_decode` in
//! `src/main.rs`) against the golden decode-regression vectors
//! (`testdata/golden/manifest.toml`).
//!
//! This spawns the actual compiled `coppa` binary (`CARGO_BIN_EXE_coppa`) and
//! checks its stdout, not just the library call underneath -- proving the
//! brief's literal scenario ("`coppa rx --wav file.wav` decodes the golden
//! vectors and prints their payloads") end-to-end, including the CLI's own
//! argument parsing, streaming/chunking loop, and hex-printing format. The
//! golden vectors' payloads are arbitrary binary data (not valid UTF-8 in
//! general), which is exactly why `cmd_rx` was migrated off the old
//! UTF-8-forcing batch `core.decode()` call onto `CoppaCore::push_samples`
//! (raw bytes) for this task.
//!
//! ## Profile coverage
//!
//! `CoppaCore`'s profile selection (`select_ofdm_profile` in
//! `coppa-engine/src/engine.rs`) depends on the resolved
//! `EngineConfig::ofdm_band` (COP-23): a named profile pins its band
//! (`--profile VHF_FAST` => vhf_wide at every level, any HF profile =>
//! hf_standard at every level), and only the no-flag
//! `EngineConfig::default()` (`OfdmBand::LevelDerived`) falls back to the
//! level-only rule, `speed_level >= 5` (vhf_wide) or not (hf_standard) --
//! unlike the golden-vector generator's own `profile_for` rule (mirrored in
//! `crates/coppa-protocol/tests/golden_vectors.rs`), which additionally
//! forces `hf_standard` for the `poor25`/`ssbcfo` channel conditions
//! regardless of level. `coppa rx` has no flag for "force hf_standard
//! regardless of level" (and adding one is out of this task's scope), so
//! this test instead partitions each vector by whichever of `coppa rx`'s two
//! actually-reachable profile choices matches its real OFDM profile:
//!   - no `--profile` flag => `EngineConfig::default()` => speed_level 1 => hf_standard
//!   - `--profile VHF_FAST` => speed_level 9 => vhf_wide
//!
//! `golden_manifest_covers_the_full_grid` (in the `coppa-protocol` test)
//! already guarantees the manifest's exact shape (every speed level,
//! multi-codeword frames and retransmissions, COP-129), so
//! `test_rx_golden_vectors_cover_every_vector_exactly_once` below is a
//! self-check that this test's partition doesn't silently drop a vector, not
//! a re-statement of that shape guarantee.
//!
//! ## Every vector runs
//!
//! Every manifest vector runs through `coppa rx` since COP-126, which gave the
//! streaming receiver's sync detector its own decimated band-pass
//! (`docs/adr/011-cop126-decimated-sync-prefilter.md`). That includes the
//! COP-129 retransmission vectors, whose WAV holds two frames of one payload:
//! `coppa rx` must combine them and print exactly one line, the payload.

mod common;

use std::path::{Path, PathBuf};

#[derive(serde::Deserialize)]
struct ManifestFile {
    vectors: Vec<VectorEntry>,
}

#[derive(serde::Deserialize)]
struct VectorEntry {
    id: String,
    level: u8,
    channel: String,
    #[allow(dead_code)]
    seed: u64,
    #[allow(dead_code)]
    seed_attempts: u64,
    payload_hex: String,
    wav_file: String,
    #[allow(dead_code)]
    sample_rate: u32,
    expected_decode_ok: bool,
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/golden")
}

fn read_manifest() -> ManifestFile {
    let manifest_path = golden_dir().join("manifest.toml");
    let text = std::fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        panic!(
            "failed to read golden manifest {}: {e} (run `cargo run -p coppa-bench --release \
             --example golden_vectors_gen` from the workspace root to (re)generate it)",
            manifest_path.display()
        )
    });
    toml::from_str(&text).expect("failed to parse golden manifest.toml")
}

/// vhf_wide is only reachable via `coppa rx --profile VHF_FAST` when the
/// vector's own real profile was actually vhf_wide -- i.e. `level >= 5` AND
/// the channel isn't one of the generator's forced-hf_standard special cases.
/// Every other vector (including all `poor25`/`ssbcfo` vectors, regardless of
/// level) was encoded with hf_standard, `coppa rx`'s no-`--profile` default.
fn needs_vhf_profile(level: u8, channel: &str) -> bool {
    level >= 5 && channel != "poor25" && channel != "ssbcfo"
}

/// Run `coppa rx --input <wav> --raw [--profile <name>]` and return its
/// trimmed stdout (expected to be one lowercase-hex line per decoded frame).
fn run_rx(wav_path: &Path, profile: Option<&str>) -> String {
    let mut cmd = common::coppa_command();
    cmd.args(["rx", "--input", wav_path.to_str().unwrap(), "--raw"]);
    if let Some(p) = profile {
        cmd.args(["--profile", p]);
    }
    let output = cmd.output().expect("failed to spawn coppa binary");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn golden_vectors_rx_cli_decodes_and_prints_payloads() {
    let manifest = read_manifest();
    assert!(
        !manifest.vectors.is_empty(),
        "golden manifest has no vectors"
    );

    let dir = golden_dir();
    let mut failures = Vec::new();

    for v in &manifest.vectors {
        if !v.expected_decode_ok {
            // Documented, verified known-limitation failure (see manifest.toml's
            // header comment) -- not this test's concern.
            continue;
        }

        let wav_path = dir.join(&v.wav_file);
        let profile = if needs_vhf_profile(v.level, &v.channel) {
            Some("VHF_FAST")
        } else {
            None
        };

        let stdout = run_rx(&wav_path, profile);
        // Exactly one line, the payload: no missed frame, no duplicate and no
        // trailing bytes.
        if stdout != v.payload_hex {
            failures.push(format!(
                "{}: coppa rx stdout is not exactly the expected payload\n  expected: {}\n  got:      {}",
                v.id, v.payload_hex, stdout
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "golden vector rx CLI failures ({}/{}):\n{}",
        failures.len(),
        manifest.vectors.len(),
        failures.join("\n")
    );
}

/// Self-check: every manifest vector is `expected_decode_ok` (as of
/// 2026-07-26 -- see `golden_vectors_gen.rs`'s module doc: the previous
/// `L8_poor25` (`L9_poor25` before the COP-24 renumber) "structurally
/// undecodable" classification was a stale-IR-HARQ-accumulator bench bug,
/// fixed, and that vector now decodes like every other), and every one is what
/// `golden_vectors_rx_cli_decodes_and_prints_payloads` above exercises, with
/// no exclusion (COP-126 removed the last one) -- catches the test silently
/// dropping a vector. The total is not pinned here:
/// `golden_manifest_covers_the_full_grid` in `coppa-protocol` owns the shape.
#[test]
fn test_rx_golden_vectors_cover_every_vector_exactly_once() {
    let manifest = read_manifest();
    let total = manifest.vectors.len();
    assert!(total > 0, "golden manifest has no vectors");

    let expected_ok: Vec<&VectorEntry> = manifest
        .vectors
        .iter()
        .filter(|v| v.expected_decode_ok)
        .collect();
    assert_eq!(
        expected_ok.len(),
        total,
        "expected every golden vector to be known-good"
    );

    // `golden_vectors_rx_cli_decodes_and_prints_payloads` skips only vectors
    // that are not `expected_decode_ok`, so every one of these is covered.
    let covered = expected_ok.len();
    assert_eq!(
        covered, total,
        "expected all {total} vectors covered by the rx CLI test, with no exclusion"
    );

    for v in &expected_ok {
        // needs_vhf_profile is a total function over (level, channel) -- this
        // just documents/asserts the grouping is deliberate, not accidental.
        let _ = needs_vhf_profile(v.level, &v.channel);
    }
}
