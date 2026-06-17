#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Fuzz Frame::from_payload_bits with arbitrary bit data
    let bits: Vec<u8> = data.iter().flat_map(|byte| {
        (0..8).rev().map(move |i| (byte >> i) & 1)
    }).collect();

    let _ = coppa_protocol::Frame::from_payload_bits(&bits);

    // Also try find_sync_with_polarity
    let _ = coppa_protocol::Frame::find_sync_with_polarity(&bits);
});
