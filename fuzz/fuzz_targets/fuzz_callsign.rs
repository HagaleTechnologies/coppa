#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Fuzz callsign parsing with arbitrary strings
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = coppa_protocol::mac::Callsign::new(s);
    }

    // Also fuzz callsign decoding from bytes
    if data.len() >= 6 {
        let bytes: &[u8; 6] = data[..6].try_into().unwrap();
        let _ = coppa_protocol::mac::Callsign::decode(bytes);
    }
});
