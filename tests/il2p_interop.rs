//! Independent Direwolf 1.7 audio and KISS interoperability checks.
use coppa_audio::{AudioSource, WavSource};
use coppa_codec::il2p;
use std::path::PathBuf;

fn fixture(name: &str, extension: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/il2p")
        .join(format!("{name}.{extension}"))
}

#[test]
fn direwolf_recordings_decode_byte_exactly() {
    for name in [
        "ui-max",
        "ui-auto-inverted",
        "i",
        "s",
        "u",
        "transparent",
        "maximum",
    ] {
        let mut source = WavSource::open(fixture(name, "wav")).unwrap();
        let mut demod = il2p::Demodulator::new();
        let mut chunk = [0.0; 137];
        loop {
            let n = source.read(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            demod.process(&chunk[..n]);
        }
        assert_eq!(
            demod.take_frames(),
            vec![std::fs::read(fixture(name, "ax25")).unwrap()],
            "{name}"
        );
    }
}

#[test]
#[ignore = "requires Direwolf 1.7; set DIREWOLF to its executable"]
fn direwolf_accepts_coppa() {
    il2p_direwolf::verify(|f| il2p::modulate(f).unwrap());
}

#[path = "support/il2p_direwolf.rs"]
mod il2p_direwolf;
