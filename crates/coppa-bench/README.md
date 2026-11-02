# coppa-bench

Measurement harness, not a product crate: BER/FER/goodput-vs-SNR scenarios, benchmark examples, and the lib tests that guard each speed level's AWGN waterfall on every push.

Part of [coppa](https://github.com/HagaleTechnologies/coppa), an OFDM HF modem
reference implementation. Depends on coppa-channel, coppa-codec, coppa-ml and coppa-protocol; no other coppa crate depends on it.

## Run

Not on crates.io and not a product crate. From a clone of the repository:

- `cargo run -p coppa-bench --release -- --trials 50` sweeps every speed level over AWGN.
- `cargo run -p coppa-bench --release --example <name>` runs one scenario from
  `crates/coppa-bench/examples/`.
- `cargo test -p coppa-bench --lib waterfall_bracket` runs the per-level FER guards.

Results are logged in [BENCHMARKS.md](https://github.com/HagaleTechnologies/coppa/blob/main/BENCHMARKS.md).
Read its "Measurement conventions corrected" section first: entries older than that section
use an SNR convention that reads about 9 dB optimistic.

## Limits

Known gaps are listed in
[docs/LIMITATIONS.md](https://github.com/HagaleTechnologies/coppa/blob/main/docs/LIMITATIONS.md).

## License

MIT OR Apache-2.0.
