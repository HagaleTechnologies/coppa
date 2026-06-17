#![no_main]
use libfuzzer_sys::fuzz_target;
use coppa_codec::traits::FecCodec;

fuzz_target!(|data: &[u8]| {
    // Create soft symbols from the raw bytes (-1.0 to 1.0 range)
    let soft_symbols: Vec<f32> = data.iter()
        .map(|&b| (b as f32 / 128.0) - 1.0)
        .collect();

    // Fuzz the Viterbi decoder with arbitrary soft symbols
    let decoder = coppa_protocol::fec::convolutional::ViterbiDecoder::new();
    let _ = decoder.decode(&soft_symbols);
});
