# Embedding coppa in your Rust app

This guide shows how to call the coppa modem from your own Rust program: turn bytes into
48 kHz audio samples, and turn received samples back into bytes. Your program owns the audio
device, the radio and the PTT line. coppa only converts between bytes and samples.

Every Rust block in this guide is compiled and run as a doctest by `cargo test --workspace`,
so the code matches the current API.

## Add the dependency

The coppa crates are not on crates.io. Depend on them from git:

```toml
[dependencies]
coppa-engine = { git = "https://github.com/HagaleTechnologies/coppa" }
# For reproducible builds, pin a commit: add rev = "<commit sha>"
```

`coppa-engine` is the entry point. Add `coppa-protocol`, `coppa-codec` or `coppa-channel`
the same way when a section below uses them. Each crate has a README under `crates/<name>/`
that says what it does and what it depends on.

## Encode and decode a frame

`CoppaCore` is the engine. `encode_bytes` returns one frame of mono `f32` samples at 48 kHz,
and `decode_bytes` reads one frame back. A complete `src/main.rs`:

```rust
use coppa_engine::CoppaCore;

fn main() {
    let core = CoppaCore::new();
    let audio = core.encode_bytes(b"hello from my app").expect("payload fits");
    println!("{} samples at 48 kHz", audio.len());
    let bytes = core.decode_bytes(&audio).expect("clean loopback decodes");
    assert_eq!(bytes, b"hello from my app");
    println!("round trip ok: {}", String::from_utf8_lossy(&bytes));
}
```

`cargo run` prints:

```text
65520 samples at 48 kHz
round trip ok: hello from my app
```

`CoppaCore::new()` uses speed level 1 on the `hf_standard` OFDM profile, with compression and
squelch off. `encode` and `decode` do the same for `&str` and `String`.

## Receive a stream

A live receiver gets audio in small blocks, not one frame at a time. Feed each block to
`push_samples`. It buffers across calls and returns a `StreamFrame` for every frame it finishes.
Each `StreamFrame` carries the payload as `Result<Vec<u8>, EngineError>` plus link metrics.

```rust
use coppa_engine::CoppaCore;

let tx = CoppaCore::new();
let mut audio = vec![0.0f32; 4_800]; // 100 ms of silence before the frame
audio.extend(tx.encode_bytes(b"streamed").expect("payload fits"));
audio.extend(vec![0.0f32; 4_800]);

let mut rx = CoppaCore::new();
let mut received = Vec::new();
for block in audio.chunks(1_024) {
    for frame in rx.push_samples(block) {
        println!("level {} at {:.1} dB SNR", frame.speed_level, frame.snr_db);
        received.push(frame.payload.expect("clean loopback decodes"));
    }
}
assert_eq!(received, vec![b"streamed".to_vec()]);
```

`push_samples` takes `&mut self`, so keep one receiving `CoppaCore` per audio input.

## Choose a speed level or profile

The speed level (1-9) sets the constellation and the LDPC code rate. Higher levels carry more
bytes per frame and need a cleaner channel. HF profiles carry levels 1-8. Set the level in
`EngineConfig`, start from a named profile, or change it on a running engine:

```rust
use coppa_engine::{CoppaCore, EngineConfig, HF_ROBUST};

let level_4 = CoppaCore::with_config(EngineConfig {
    speed_level: 4,
    ..EngineConfig::default()
});
assert_eq!(level_4.speed_level(), 4);

let robust = CoppaCore::from_profile(&HF_ROBUST);
println!("{} carries {:?} bytes per frame", HF_ROBUST.name, HF_ROBUST.max_payload());

let mut core = CoppaCore::new();
core.set_speed_level(3).expect("level 3 exists on HF");
assert_eq!(core.speed_level(), 3);

// The speed level travels in the frame header, so a receiver on the same
// OFDM profile decodes any level.
let audio = level_4.encode_bytes(b"level 4 frame").expect("payload fits");
assert_eq!(robust.decode_bytes(&audio).expect("decodes"), b"level 4 frame");
```

`coppa profiles` on the command line prints every level's modulation, code rate, maximum payload
and air time. The `ladder` module in `coppa-engine` holds the same facts. Both ends of a link
must agree on compression: a named profile sets it, and `EngineConfig::default()` leaves it off.

## Handle errors

Every fallible call returns `EngineError`. The enum is `#[non_exhaustive]`, so match the kinds
you handle and keep a wildcard arm for variants added later:

```rust
use coppa_engine::{CoppaCore, EngineError, HF_STANDARD};
use coppa_protocol::modem::TransmitError;

let mut core = CoppaCore::from_profile(&HF_STANDARD);

match core.set_speed_level(9) {
    Err(EngineError::SpeedLevelUnavailableOnHf { level }) => assert_eq!(level, 9),
    other => panic!("unexpected result: {other:?}"),
}

// HF_STANDARD compresses, so use an uncompressed engine to hit the size limit.
let plain = CoppaCore::new();
match plain.encode_bytes_with_level(&[0xAB; 4096], 1) {
    Err(EngineError::Transmit(TransmitError::PayloadTooLarge { max })) => {
        println!("split the data into chunks of at most {max} bytes");
    }
    Err(other) => panic!("unexpected error: {other}"),
    Ok(_) => panic!("4096 bytes do not fit one level-1 frame"),
}

match plain.decode_bytes(&[]) {
    Err(EngineError::NoSamplesToDecode) => {}
    Err(e) => eprintln!("decode failed: {e}"),
    Ok(bytes) => println!("got {} bytes", bytes.len()),
}
```

## Go lower: the transceiver

`CoppaCore` wraps `CoppaTransceiver` from `coppa-protocol`, adding compression, squelch and
streaming. Use the transceiver directly when you want to build the frame header yourself or read
the receiver's recommended speed level. Add `coppa-protocol` and `coppa-codec` as git
dependencies first.

```rust
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::modem::CoppaTransceiver;

let transceiver = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
let payload = b"CQ CQ DE N0CALL";
let header = CoppaHeader {
    version: 1,
    phy_mode: 0,
    frame_type: CoppaFrameType::Data,
    bandwidth: transceiver.profile().bandwidth_id,
    fec_type: 0,
    speed_level: 1,
    seq_num: 0,
    payload_len: payload.len() as u16,
    codewords: 1,
};
let audio = transceiver.transmit(&header, payload).expect("payload fits level 1");
let (rx_header, rx_payload, recommended_level) =
    transceiver.receive(&audio).expect("clean loopback decodes");
assert_eq!(rx_header.speed_level, 1);
assert_eq!(rx_payload, payload);
println!("receiver recommends level {recommended_level}");
```

## Test against a noisy channel

`coppa-channel` adds noise, HF fading, an SSB filter and frequency or clock offsets to a sample
buffer. Add it as a dev-dependency so your tests can check decoding under noise:

```toml
[dev-dependencies]
coppa-channel = { git = "https://github.com/HagaleTechnologies/coppa" }
```

`awgn_ref_seeded` sets the SNR the way HF modems quote it: signal power over noise in a 3 kHz
bandwidth. Measure the signal power on the clean audio. The seed makes the noise repeatable.

```rust
use coppa_channel::{awgn_ref_seeded, mean_power};
use coppa_engine::CoppaCore;

let core = CoppaCore::new();
let clean = core.encode_bytes(b"through the noise").expect("payload fits");
let noisy = awgn_ref_seeded(&clean, 10.0, mean_power(&clean), 48_000.0, 0x00C0_FFEE);
assert_eq!(core.decode_bytes(&noisy).expect("level 1 decodes at 10 dB"), b"through the noise");
```

`awgn_seeded` is simpler but references the full 48 kHz band, which reads about 9 dB better than
the 3 kHz figure. The SNR band where each level's frame error rate falls to 10% or less, on
the 3 kHz reference with every level pinned to the `hf_standard` profile, is listed in the doc
comment on `bracket` in [crates/coppa-bench/src/runner.rs](../../crates/coppa-bench/src/runner.rs).
The `waterfall_bracket_level_N` tests there check one SNR point below and one above each band.

## What the library does not do

- **No sessions or ARQ.** The library sends and receives single frames. It does not connect to
  a peer, acknowledge frames or retransmit lost ones. That layer runs only inside the `coppad`
  daemon, which host programs drive over the VARA-style TCP command port. See
  [Connected sessions in docs/OPERATING.md](../OPERATING.md#connected-sessions-delivery-and-buffer)
  and [docs/PAT.md](../PAT.md) for Pat/Winlink.
- **No audio or radio I/O.** You capture and play the samples. `coppa-audio` (CPAL, WAV) and
  `coppa-radio` (rigctld CAT, serial and GPIO PTT) are what the CLI and daemon use.
- **No C API here.** C, C++ and other languages use `coppa-ffi`. See
  [Using the C FFI](getting-started.md#using-the-c-ffi) in the getting-started tutorial.

Known gaps in the modem itself are listed in [docs/LIMITATIONS.md](../LIMITATIONS.md).
