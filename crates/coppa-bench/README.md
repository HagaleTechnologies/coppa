# coppa-bench

Measurement harness, not a product crate: BER/FER/goodput-vs-SNR scenarios, benchmark examples, and the lib tests that guard each speed level's AWGN waterfall on every push.

Part of [coppa](https://github.com/HagaleTechnologies/coppa), an OFDM HF modem
reference implementation. Depends on coppa-channel, coppa-codec, coppa-engine, coppa-ml and coppa-protocol; coppa-protocol and the root
`coppa` crate use it only as a dev-dependency, for tests.

## Run

Not on crates.io and not a product crate. From a clone of the repository:

- `cargo run -p coppa-bench --release -- --trials 50` sweeps every speed level over AWGN.
- `cargo run -p coppa-bench --release --example <name>` runs one scenario from
  `crates/coppa-bench/diagnostics/`.
- `cargo test -p coppa-bench --lib waterfall_bracket` runs the per-level FER guards.
- `cargo run -p coppa-bench --release -- compare` (COP-170) runs two library
  `Link`s through the WGN, MPG and MPP channels at the 2020 Winlink
  comparison's SNR grid (30 to 0 dB) for 2 KB and 16 KB transfers, 8 trials
  per cell, and renders [docs/benchmarks/comparison.md](../../docs/benchmarks/comparison.md):
  one Markdown table per channel and size with mean net bytes/min, its 95%
  Student t interval and completed/n, each cell linked to a replay row with
  its seed range and command. `compare --check` exits 1 when the page is
  stale. Every trial is a row
  in `results/compare/<row>.csv`; its seed is
  `0x17000000 + cell * 0x10000 + trial`, so a re-run reproduces it and an
  interrupted run resumes with only the missing trials. `--channel`, `--snr`,
  `--size`, `--trials`, `--rows` (`link-l2`, `link-l4`, `link-default`),
  `--jobs` and `--out-dir` narrow or move the run (an `--out-dir` also moves
  the page there, so a replay never overwrites the published one); `--page`
  names another page; `--no-render` measures only. The flat flags above do not
  mix with `compare`.
- `cargo run --release -p coppa-bench -- compare channel-check` measures the
  Watterson presets through the 48 kHz audio channel and writes
  `results/compare/channel-check.csv`. Run bare `compare` to render the page
  from that CSV, then `compare --check` to verify it.
- `cargo run --release -p coppa-bench --features cpal-backend -- compare blackbox --modem tools/compare/coppad.toml`
  runs the Linux ALSA cable experiment. Build a release `coppad` with
  `--features cpal-backend,ardop,kiss-tnc` first. The cable accepts f32le,
  48 kHz, mono playback. Live tests also need that release binary. The
  integrity probe must pass before a trial is recorded. Each row records the
  CRC-32 of the generated trial payload as `payload_crc32`, even if dialing
  fails before sending. Use a fresh `--out-dir` if the CSV lacks that column
  or a row's checksum differs from the expected value for its seed and size.

Results are logged in [BENCHMARKS.md](https://github.com/HagaleTechnologies/coppa/blob/main/BENCHMARKS.md).
Read its [Conventions](../../BENCHMARKS.md) section and the
[conventions-correction log](../../docs/benchmarks/2026-07-measurement-conventions-rebaseline.md) first: entries older than that section
use an SNR convention that reads about 9 dB optimistic.

## Limits

Known gaps are listed in
[docs/LIMITATIONS.md](https://github.com/HagaleTechnologies/coppa/blob/main/docs/LIMITATIONS.md).

## License

MIT OR Apache-2.0.
