//! OFDM round-trip example with an AWGN channel.
//!
//! Encodes binary data with [`CoppaCore`], pushes the audio samples through a
//! simulated additive-white-Gaussian-noise (AWGN) channel, and decodes them
//! back. This shows how the OFDM modem plus LDPC forward error correction
//! recover the original bytes despite channel noise.
//!
//! It also contrasts two speed levels: a robust low-rate mode and a faster
//! higher-order mode, to illustrate the throughput-vs-robustness trade-off.
//!
//! Run with:
//!     cargo run --example ofdm_roundtrip

use coppa_channel::awgn_seeded;
use coppa_engine::{CoppaCore, EngineConfig};

fn try_decode(speed_level: u8, data: &[u8], snr_db: f32) {
    let core = CoppaCore::with_config(EngineConfig {
        speed_level,
        ..Default::default()
    });

    // Encode the bytes into OFDM audio samples.
    let samples = core.encode_bytes(data).expect("encoding failed");

    // Pass the samples through an AWGN channel at the given SNR. The seed makes
    // the noise reproducible across runs.
    let noisy = awgn_seeded(&samples, snr_db, 42);

    // Attempt to decode. LDPC FEC corrects bit errors introduced by the noise.
    match core.decode_bytes(&noisy) {
        Ok(decoded) if decoded == data => {
            println!(
                "speed level {speed_level} @ {snr_db:.0} dB SNR: \
                 recovered all {} bytes",
                data.len()
            );
        }
        Ok(_) => {
            println!(
                "speed level {speed_level} @ {snr_db:.0} dB SNR: decoded, but payload differs"
            );
        }
        Err(e) => {
            println!("speed level {speed_level} @ {snr_db:.0} dB SNR: decode failed ({e})");
        }
    }
}

fn main() {
    let data = b"The quick brown fox jumps over the lazy dog.";
    println!("Payload: {} bytes\n", data.len());

    // Robust mode (BPSK, strong FEC) should survive a noisy channel.
    try_decode(1, data, 8.0);

    // Faster mode (QPSK, higher rate) needs a cleaner channel to decode.
    try_decode(3, data, 12.0);
}
