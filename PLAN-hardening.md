# Known Gaps and Follow-ups

These are areas where the implementation compiles and passes tests but is either untested at the integration level, incomplete, or in need of refactoring. Tracked here for transparency.

---

## 1. WebSocket Server Integration Test

**Problem:** `WebSocketServer::run()` has zero integration coverage. Only message serialization is tested — no test actually opens a TCP connection, performs a WebSocket handshake, sends a message, and verifies the `HostEvent` comes out the other end.

**Fix:**
1. Add `tokio-tungstenite` as a dev-dependency on `coppa-host`
2. Write an integration test that:
   - Spawns `WebSocketServer::run()` on a random port
   - Connects with `tokio_tungstenite::connect_async`
   - Sends a `{"type":"send","data":"Hello"}` text frame
   - Reads the `HostEvent::DataReceived` from the event receiver
   - Sends a `{"type":"status"}` and verifies the JSON response
   - Sends invalid JSON and verifies an error response
   - Closes the connection and verifies `HostEvent::Disconnected`

**Files:** `crates/coppa-host/src/websocket.rs` (add `#[cfg(test)]` integration tests)

**Est:** ~60 lines

---

## 2. Connect CPAL Audio to Daemon Ring Buffers

**Problem:** `main.rs` creates `AudioRingProducer`/`AudioRingConsumer` pairs but drops the hardware-facing halves (`_audio_out_consumer`, `_audio_in_producer`). No CPAL streams are spawned, so `coppad` is silent — audio never flows.

**Fix:**
1. After creating ring buffers, optionally spawn CPAL input/output streams:
   - `CpalSource::default_input(config.audio.sample_rate)` → read into `_audio_in_producer` in a spawned task
   - `CpalSink::default_output(config.audio.sample_rate)` → drain from `_audio_out_consumer` in a spawned task
2. Gate behind `cpal-backend` feature (already exists on `coppa-audio`)
3. Add `cpal-backend` as optional feature to `coppa-daemon/Cargo.toml`
4. If CPAL init fails, log a warning and continue without audio (daemon can still serve VARA TCP)

**Files:** `crates/coppa-daemon/Cargo.toml`, `crates/coppa-daemon/src/main.rs`

**Est:** ~40 lines

---

## 3. CLI Listen Command: Sliding Buffer Decoder

**Problem:** `cmd_listen()` calls `core.decode(&buf[..n])` on fixed 1-second blocks. Frame boundaries won't align with block boundaries, so most frames will be missed. The decoder needs to see the full frame (preamble + sync + payload + CRC) in a single call.

**Fix:**
1. Accumulate samples in a `Vec<f32>` sliding window
2. After each read, try `core.decode(&window)`:
   - On success: drain the window up to the decoded frame's end, print the message
   - On failure: if window exceeds max frame size (~60k samples), drain the oldest half
3. This matches how the streaming FFI API (`coppa_feed_samples`) already works — replicate that pattern

**Files:** `crates/coppa-cli/src/main.rs` (`cmd_listen`)

**Est:** ~25 lines changed

---

## 4. OFDM Tests at Realistic SNR

**Problem:** The new OFDM sync tests all use clean or simple signals. The plan called for "noisy preamble detection" and "near-threshold SNR" tests, which were skipped because they need deterministic noise (seeded RNG).

**Fix:**
1. In `sync.rs`: add `test_schmidl_cox_noisy_detection` — inject AWGN at 10 dB SNR using `rand::rngs::StdRng::seed_from_u64`, verify detection still works
2. In `equalizer.rs`: add `test_equalize_frequency_selective_with_noise` — apply a known frequency-selective channel + AWGN, verify equalized symbols are within tolerance
3. In `mod.rs`: add `test_ofdm_roundtrip_with_awgn` — modulate, add noise at 15 dB SNR, demodulate, check subcarrier error is bounded
4. Use `coppa-channel::awgn::add_awgn` if available, or inline noise generation with seeded RNG

**Files:** `crates/coppa-codec/src/ofdm/sync.rs`, `equalizer.rs`, `mod.rs`

**Est:** ~80 lines

---

## 5. LDPC Near-Threshold Test

**Problem:** LDPC tests use clean channel or light deterministic noise. No test verifies decoder behavior near the coding threshold where it should barely converge, or verifies that it gives up gracefully when noise is too high.

**Fix:**
1. `test_ldpc_near_threshold` — rate 1/2, add AWGN at ~2 dB SNR (near threshold), verify decode succeeds
2. `test_ldpc_beyond_capacity` — add AWGN at -3 dB SNR, verify decode returns *something* without panicking (may not match input, that's OK)
3. `test_ldpc_max_iterations_reached` — use very noisy soft inputs, verify decoder terminates within max iterations (doesn't hang)
4. Use seeded RNG for determinism

**Files:** `crates/coppa-protocol/src/fec/ldpc/mod.rs`

**Est:** ~50 lines

---

## 6. ML Model Loading Is No-Op

**Problem:** `load_channel_predictor()` scans a `models/` directory that doesn't exist and always returns EWMA. The change from the original was purely cosmetic.

**Fix:** This is actually fine for now — there are no trained ONNX models to load. But document it clearly:
1. Add a `models/.gitkeep` with a README explaining the expected model format
2. Update the function doc comment to explain: "Currently always returns EWMA. To use a trained model, place a `channel_predictor.onnx` file in the `models/` directory and enable the `ml` feature (not yet implemented)."

**Files:** `crates/coppa-ml/src/lib.rs`, `models/README.md`

**Est:** ~15 lines

---

## Implementation Order

| Priority | Task | Risk | Reason |
|----------|------|------|--------|
| 1 | Sliding buffer decoder (3) | Low | Current listen command is broken for real use |
| 2 | CPAL→ring buffer wiring (2) | Medium | Makes daemon actually functional |
| 3 | WebSocket integration test (1) | Low | Proves the server works end-to-end |
| 4 | OFDM noisy tests (4) | Low | Catches real signal processing bugs |
| 5 | LDPC threshold tests (5) | Low | Validates FEC at operating limits |
| 6 | ML model docs (6) | None | Clarification only |

**Total: ~270 lines**

## Verification

After each task:
```bash
cargo test --workspace
cargo clippy --workspace
```
