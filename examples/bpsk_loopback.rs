//! Minimal end-to-end loopback example.
//!
//! Encodes a text message into audio samples with [`CoppaCore`], then decodes
//! those same samples back to text and prints the result. This is the simplest
//! possible demonstration of the Coppa encode/decode pipeline -- no channel
//! impairments, no audio hardware, just the pure software path.
//!
//! Run with:
//!     cargo run --example bpsk_loopback

use coppa_engine::CoppaCore;

fn main() {
    // `CoppaCore::new()` uses the default configuration (speed level 1:
    // BPSK with a strong 1/4-rate LDPC code), which is the most robust mode.
    let core = CoppaCore::new();

    let message = "CQ CQ CQ de N0CALL";
    println!("Original message:  {message:?}");

    // Encode the message into a buffer of 48 kHz f32 audio samples.
    // In a real system these samples would be played out to a sound card
    // and transmitted over the air.
    let samples = core.encode(message).expect("encoding failed");
    println!("Encoded into {} audio samples", samples.len());

    // Decode the samples straight back to text (a clean "loopback": the
    // samples never leave memory, so there is no channel noise).
    let decoded = core.decode(&samples).expect("decoding failed");
    println!("Decoded message:   {decoded:?}");

    assert_eq!(message, decoded, "round-trip must preserve the message");
    println!("Round-trip succeeded.");
}
