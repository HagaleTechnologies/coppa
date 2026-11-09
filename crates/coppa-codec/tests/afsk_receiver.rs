//! Receiver robustness tests for the AFSK 1200 demodulator (COP-113).
//!
//! Every test here uses synthetic audio, so CI needs no recorded track. Each
//! one stays under about 60 s of audio, because a debug build demodulates at
//! only a few times real time.
use coppa_codec::afsk::{append_fcs, modulate, Demodulator};

const RATE: usize = 48_000;

/// Seeded xorshift64 generator, so every stream is reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// Uniform integer in `lo..=hi`.
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + (self.next() % (hi - lo + 1) as u64) as usize
    }
    fn unit(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    /// Standard normal sample (Box-Muller).
    fn gauss(&mut self) -> f64 {
        let (u, v) = (self.unit(), self.unit());
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
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

fn random_frame(rng: &mut Rng, total_len: usize) -> Vec<u8> {
    let body: Vec<u8> = (0..total_len - 2).map(|_| rng.next() as u8).collect();
    append_fcs(&body)
}

/// `count` random frames of `min..=max` bytes (FCS included), each after a
/// seeded gap of 50 to 250 ms.
fn random_stream(seed: u64, count: usize, min: usize, max: usize) -> (Vec<f32>, Vec<Vec<u8>>) {
    let mut rng = Rng(seed);
    let mut audio = Vec::new();
    let mut frames = Vec::new();
    for _ in 0..count {
        let gap = rng.range(RATE / 20, RATE / 4);
        audio.extend(std::iter::repeat_n(0.0, gap));
        let len = rng.range(min, max);
        let frame = random_frame(&mut rng, len);
        audio.extend(modulate(&frame));
        frames.push(frame);
    }
    audio.extend(std::iter::repeat_n(0.0, RATE / 10));
    (audio, frames)
}

/// One-pole low-pass, the de-emphasis a radio's audio path applies.
fn de_emphasise(audio: &[f32], corner_hz: f64) -> Vec<f32> {
    let k = (-std::f64::consts::TAU * corner_hz / RATE as f64).exp();
    let mut y = 0.0_f64;
    audio
        .iter()
        .map(|&x| {
            y = k * y + (1.0 - k) * x as f64;
            y as f32
        })
        .collect()
}

/// Linear-interpolation resampler: models a transmitter whose sample clock
/// is `ppm` parts per million away from the receiver's.
fn clock_offset(audio: &[f32], ppm: f64) -> Vec<f32> {
    let ratio = 1.0 + ppm * 1e-6;
    (0..((audio.len() - 1) as f64 / ratio) as usize)
        .map(|i| {
            let position = i as f64 * ratio;
            let index = position as usize;
            let fraction = (position - index as f64) as f32;
            audio[index] * (1.0 - fraction) + audio[index + 1] * fraction
        })
        .collect()
}

/// Counts sent frames that were delivered, and fails on any frame that was
/// not sent or was delivered twice.
fn count_sent(sent: &[Vec<u8>], got: &[Vec<u8>], label: &str) -> usize {
    for (i, frame) in got.iter().enumerate() {
        assert!(
            sent.contains(frame),
            "{label}: delivered a frame that was not sent"
        );
        assert!(
            !got[..i].contains(frame),
            "{label}: delivered the same reception twice"
        );
    }
    got.len()
}

#[test]
fn clean_frame_decodes_at_every_symbol_offset() {
    let mut rng = Rng(0xC0_113);
    let frame = random_frame(&mut rng, 84);
    let tx = modulate(&frame);
    let mut failed = Vec::new();
    for off in 0..40 {
        let mut audio = vec![0.0; 4800 + off];
        audio.extend(&tx);
        audio.extend(vec![0.0; 4800]);
        if decode(&audio, 1024) != vec![frame.clone()] {
            failed.push(off);
        }
    }
    assert!(failed.is_empty(), "failed at offsets {failed:?}");
}

#[test]
fn long_frames_survive_sample_clock_offset() {
    let mut rng = Rng(0x200_0113);
    for ppm in [-200.0, 200.0] {
        let mut audio = Vec::new();
        let mut sent = Vec::new();
        for i in 0..4 {
            audio.extend(vec![0.0; 2400 + 13 * i]);
            let frame = random_frame(&mut rng, 252);
            audio.extend(modulate(&frame));
            sent.push(frame);
        }
        audio.extend(vec![0.0; 4800]);
        let got = decode(&clock_offset(&audio, ppm), 1024);
        assert_eq!(got, sent, "{ppm} ppm");
    }
}

fn de_emphasised_count(corner_hz: f64) -> usize {
    let (audio, sent) = random_stream(60, 60, 30, 90);
    let audio = coppa_channel::awgn_seeded(&de_emphasise(&audio, corner_hz), 0.0, 113);
    let got = decode(&audio, 1024);
    count_sent(&sent, &got, &format!("{corner_hz} Hz de-emphasis"))
}

#[test]
fn de_emphasised_stream_decodes_at_0_db() {
    let decoded = de_emphasised_count(300.0);
    assert!(decoded >= 57, "decoded {decoded} of 60");
}

#[test]
fn steep_de_emphasis_needs_more_than_one_slicer() {
    let decoded = de_emphasised_count(150.0);
    assert!(decoded >= 59, "decoded {decoded} of 60");
}

fn noisy_stream() -> (Vec<f32>, Vec<Vec<u8>>) {
    let (audio, sent) = random_stream(20, 20, 30, 90);
    (coppa_channel::awgn_seeded(&audio, 6.0, 1130), sent)
}

#[test]
fn decode_is_independent_of_block_size() {
    let (audio, sent) = noisy_stream();
    let reference = decode(&audio, 1024);
    assert_eq!(count_sent(&sent, &reference, "1024-sample blocks"), 20);
    for chunk in [1, 17, 4096] {
        assert_eq!(decode(&audio, chunk), reference, "chunk {chunk}");
    }
}

#[test]
fn decode_is_independent_of_input_level() {
    let (audio, sent) = noisy_stream();
    let reference = decode(&audio, 1024);
    assert_eq!(count_sent(&sent, &reference, "level 1.0"), 20);
    for level in [0.01_f32, 3.0] {
        let scaled: Vec<f32> = audio.iter().map(|&x| x * level).collect();
        assert_eq!(decode(&scaled, 1024), reference, "level {level}");
    }
}

#[test]
fn identical_frames_back_to_back_deliver_twice() {
    let frame = append_fcs(b"N0CALL>APRS:repeat");
    let tx = modulate(&frame);
    for gap in [0, 480, 4800] {
        let mut audio = vec![0.0; 2000];
        audio.extend(&tx);
        audio.extend(vec![0.0; gap]);
        audio.extend(&tx);
        audio.extend(vec![0.0; 4800]);
        assert_eq!(
            decode(&audio, 1024),
            vec![frame.clone(), frame.clone()],
            "gap {gap}"
        );
    }
}

#[test]
fn noise_only_input_delivers_nothing() {
    let mut rng = Rng(0x0113_0113);
    let mut audio: Vec<f32> = (0..30 * RATE).map(|_| (0.3 * rng.gauss()) as f32).collect();
    let mut y = 0.0_f64;
    for _ in 0..30 * RATE {
        y = 0.9 * y + 0.1 * 0.3 * rng.gauss();
        audio.push((4.0 * y) as f32);
    }
    assert!(decode(&audio, 1024).is_empty());
}
