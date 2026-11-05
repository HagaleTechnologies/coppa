use coppa_codec::afsk::{append_fcs, modulate, modulate_fx25, Demodulator, Fx25Error};
use std::f64::consts::TAU;

const TAGS: [u64; 11] = [
    0xB74DB7DF8A532F3E,
    0x26FF60A600CC8FDE,
    0xC7DC0508F3D9B09E,
    0x8F056EB4369660EE,
    0x6E260B1AC5835FAE,
    0xFF94DC634F1CFF4E,
    0x1EB7B9CDBC09C00E,
    0xDBF869BD2DBB1776,
    0x3ADB0C13DEAE2836,
    0xAB69DB6A543188D6,
    0x4A4ABEC4A724B796,
];
fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/fx25/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}
fn payload(info: &[u8]) -> Vec<u8> {
    let mut b = vec![
        0x82, 0xa0, 0xa4, 0xa6, 0x40, 0x40, 0xe0, 0x9c, 0x60, 0x86, 0x82, 0x98, 0x98, 0xe1, 3, 0xf0,
    ];
    b.extend(info);
    b
}
// Independent test renderer, never calls coppa's FX.25 encoder.
fn render(bytes: &[u8]) -> Vec<f32> {
    let (mut phase, mut state) = (0.0_f64, true);
    let mut out = Vec::new();
    for &b in bytes {
        for i in 0..8 {
            if b & (1 << i) == 0 {
                state = !state;
            }
            for _ in 0..40 {
                out.push(phase.sin() as f32);
                phase = (phase + TAU * if state { 1200.0 } else { 2200.0 } / 48000.0) % TAU;
            }
        }
    }
    out
}
fn wire(tag: u64, block: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0x7e; 36];
    bytes.extend(tag.to_le_bytes());
    bytes.extend(block);
    bytes.extend([0x7e; 2]);
    bytes
}
fn decode(audio: &[f32], chunk: usize) -> Vec<Vec<u8>> {
    let mut d = Demodulator::new();
    let mut frames = Vec::new();
    for c in audio.chunks(chunk) {
        d.process(c);
        frames.extend(d.take_frames());
    }
    frames
}
#[test]
fn all_independent_profiles_clean_and_damaged_streaming() {
    let expected = append_fcs(&payload(b"????~~~~\n"));
    for (i, &tag) in TAGS.iter().enumerate() {
        let clean = fixture(&format!("tag{:02}.bin", i + 1));
        for chunk in [1, 17, 1024, 997] {
            assert_eq!(
                decode(&render(&wire(tag, &clean)), chunk),
                vec![expected.clone()],
                "tag {} chunk {}",
                i + 1,
                chunk
            );
            let mut damaged = clean.clone();
            for v in &mut damaged[8..16] {
                *v ^= 255;
            }
            assert_eq!(
                decode(&render(&wire(tag, &damaged)), chunk),
                vec![expected.clone()]
            );
        }
    }
}
#[test]
fn encoder_errors_capacity_and_plain_output() {
    assert!(matches!(modulate_fx25(&[0, 0]), Err(Fx25Error::InvalidFcs)));
    let mut invalid = append_fcs(b"bad");
    invalid[0] ^= 1;
    assert!(matches!(
        modulate_fx25(&invalid),
        Err(Fx25Error::InvalidFcs)
    ));
    for (len, bytes) in [(1, 64), (31, 96), (63, 160), (127, 255)] {
        let frame = append_fcs(&vec![0; len]);
        let audio = modulate_fx25(&frame).unwrap();
        assert_eq!(audio.len(), (36 * 8 + 64 + bytes * 8 + 2 * 8) * 40);
        assert_eq!(decode(&audio, 17), vec![frame]);
    }
    // No stuffing for zero payload; account for FCS stuffing at the boundary.
    let fits: Vec<_> = (1..230)
        .filter(|&n| modulate_fx25(&append_fcs(&vec![0; n])).is_ok())
        .collect();
    assert!(fits.last().copied().unwrap() >= 218);
    assert!(matches!(
        modulate_fx25(&append_fcs(&[0xff; 223])),
        Err(Fx25Error::FrameTooLong)
    ));
    let frame = append_fcs(b"plain");
    assert_eq!(decode(&modulate(&frame), 17), vec![frame]);
}
#[test]
fn repeated_mixed_and_reset_recovery() {
    let frame = append_fcs(b"same payload");
    let fx = modulate_fx25(&frame).unwrap();
    let plain = modulate(&frame);
    let mut audio = fx.clone();
    audio.extend(&fx);
    audio.extend(&plain);
    assert_eq!(decode(&audio, 17), vec![frame.clone(); 3]);
    let mut d = Demodulator::new();
    d.process(&fx[..15000]);
    d.reset();
    d.process(&fx);
    assert_eq!(d.take_frames(), vec![frame]);
}
#[test]
fn false_truncated_uncorrectable_and_invalid_fcs_recover() {
    let clean = fixture("tag07.bin");
    let expected = append_fcs(&payload(b"????~~~~\n"));
    let mut bytes = vec![0x7e; 36];
    bytes.extend(TAGS[0].to_le_bytes());
    bytes.extend([0; 20]); // false long block
    bytes.extend(TAGS[6].to_le_bytes());
    bytes.extend(&clean);
    bytes.extend([0x7e; 2]);
    assert_eq!(decode(&render(&bytes), 17), vec![expected.clone()]);
    let mut bad = clean.clone();
    for (i, b) in bad.iter_mut().take(32).enumerate() {
        *b ^= (i + 1) as u8;
    }
    let mut audio = render(&wire(TAGS[6], &bad));
    audio.extend(render(&wire(TAGS[6], &clean)));
    assert_eq!(decode(&audio, 1024), vec![expected]);
}
fn wav(name: &str) -> Vec<f32> {
    // Committed fixtures are mono PCM16 RIFF. Locate data rather than assuming
    // a fixed WAV header size (Direwolf includes its own metadata chunks).
    let b = fixture(name);
    assert_eq!(&b[..4], b"RIFF");
    let mut pos = 12;
    while pos + 8 <= b.len() {
        let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if &b[pos..pos + 4] == b"data" {
            return b[pos + 8..pos + 8 + len]
                .chunks_exact(2)
                .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0)
                .collect();
        }
        pos += 8 + len + (len % 2);
    }
    panic!("missing WAV data");
}
#[test]
fn recorded_direwolf_clean_and_damaged_audio() {
    let expected = append_fcs(&payload(b"coppa COP-114 FX25 fixture\n"));
    for name in ["direwolf-clean.wav", "direwolf-damaged.wav"] {
        let audio = wav(name);
        // Direwolf starts its first tone at sample 1300 (20 modulo 40).
        // The existing fixed-window receiver needs a matching sample origin.
        let offset = if name == "direwolf-clean.wav" { 20 } else { 0 };
        assert_eq!(
            decode(&audio[offset..], 1024),
            vec![expected.clone()],
            "{name}"
        );
    }
    // Remove only the tag, leaving the damaged inner HDLC bytes unchanged.
    // Ordinary HDLC cannot recover the original frame.
    let mut bytes = wire(0, &fixture("audio-damaged.bin"));
    bytes[36..44].fill(0);
    assert!(decode(&render(&bytes), 1024).is_empty());
}

#[test]
fn plain_candidate_survives_failed_rs_and_variable_chunks_with_noise() {
    let mut block = fixture("tag07.bin");
    for v in &mut block[64..] {
        *v ^= 255;
    }
    let expected = append_fcs(&payload(b"????~~~~\n"));
    let mut audio = vec![0.0; 400];
    audio.extend(render(&wire(TAGS[6], &block)));
    audio.extend(vec![0.0; 400]);
    let audio = coppa_channel::awgn_seeded(&audio, 25.0, 114);
    let mut receiver = Demodulator::new();
    let (mut pos, mut index) = (0, 0);
    let mut frames = Vec::new();
    while pos < audio.len() {
        let end = (pos + [1, 17, 1024, 53][index % 4]).min(audio.len());
        receiver.process(&audio[pos..end]);
        frames.extend(receiver.take_frames());
        pos = end;
        index += 1;
    }
    assert_eq!(frames, vec![expected]);
}

#[test]
fn correctable_embedded_tag_preserves_original_reception() {
    let mut block = fixture("tag07.bin");
    // Eight erroneous symbols remain within tag 7's sixteen-byte limit.
    // The false long tag must not cancel the original reception.
    block[8..16].copy_from_slice(&TAGS[0].to_le_bytes());
    assert_eq!(
        decode(&render(&wire(TAGS[6], &block)), 17),
        vec![append_fcs(&payload(b"????~~~~\n"))]
    );
}
