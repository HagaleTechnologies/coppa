#![no_main]
use libfuzzer_sys::fuzz_target;
use coppa_protocol::outer_code::{OuterPacket, OuterBlockRx};
fuzz_target!(|data: &[u8]| {
    let mut rx=OuterBlockRx::default();
    // Exercise sequences of independently parsed packets as well as whole input.
    if let Ok(packet)=OuterPacket::from_bytes(data) {let _=rx.insert(&packet);}
    for bytes in data.chunks(103) {if let Ok(packet)=OuterPacket::from_bytes(bytes) {let _=rx.insert(&packet);}}
});
