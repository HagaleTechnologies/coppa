//! Direwolf 1.7 IL2P framing over straight-NRZ Bell 202 audio.
mod rs;
use std::f32::consts::TAU;

/// A frame cannot be represented by the selected IL2P envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Il2pError {
    /// The bytes are not a structurally complete AX.25 frame without FCS.
    #[error("invalid AX.25 frame")]
    InvalidFrame,
    /// The selected header requires more than 1023 payload bytes.
    #[error("IL2P payload exceeds 1023 bytes")]
    FrameTooLong,
}

// Invert the receive recurrence directly, rather than implementing a delayed
// transmit pipeline. Each block starts with the protocol's receive seed.
fn transform(data: &[u8], scramble: bool) -> Vec<u8> {
    let mut state = 0x1f0u16;
    data.iter()
        .map(|&byte| {
            let mut out = 0;
            for bit in (0..8).rev() {
                let input = (byte >> bit) & 1;
                let output = input ^ (state as u8 & 1);
                let wire = if scramble { output } else { input } as u16;
                state = (state >> 1) ^ (wire << 8) ^ (wire << 3);
                out = (out << 1) | output;
            }
            out
        })
        .collect()
}
fn field(h: &[u8], bit: u8, end: usize, width: usize) -> usize {
    (end + 1 - width..=end).fold(0, |v, i| (v << 1) | ((h[i] >> bit) & 1) as usize)
}
fn put(h: &mut [u8], bit: u8, end: usize, width: usize, value: usize) {
    for i in 0..width {
        h[end - i] |= (((value >> i) & 1) as u8) << bit;
    }
}
const PIDS: [u8; 16] = [
    0xf0, 0xf0, 0x20, 1, 6, 7, 8, 0xf0, 0xf0, 0xf0, 0xf0, 0xcc, 0xcd, 0xce, 0xcf, 0xf0,
];
const U_CONTROL: [u8; 8] = [0x2f, 0x43, 0x0f, 0x63, 0x87, 3, 0xaf, 0xe3];

// Locate the control octet without imposing the UI convenience API's 256 limit.
fn validate(frame: &[u8]) -> Result<usize, Il2pError> {
    let mut count = 0;
    for address in frame.chunks_exact(7).take(10) {
        count += 1;
        if address[..6]
            .iter()
            .any(|&b| b & 1 != 0 || !(0x20..=0x7e).contains(&(b >> 1)))
        {
            return Err(Il2pError::InvalidFrame);
        }
        if address[6] & 0x60 != 0x60 {
            return Err(Il2pError::InvalidFrame);
        }
        if address[6] & 1 != 0 {
            let control = count * 7;
            if count < 2 || frame.len() <= control {
                return Err(Il2pError::InvalidFrame);
            }
            let c = frame[control];
            if (c & 1 == 0 || c & 0xef == 3) && frame.len() <= control + 1 {
                return Err(Il2pError::InvalidFrame);
            }
            return Ok(control);
        }
    }
    Err(Il2pError::InvalidFrame)
}
fn header(frame: &[u8], max: bool) -> Result<([u8; 13], &[u8]), Il2pError> {
    let control = validate(frame)?;
    let mut h = [0; 13];
    let mut payload = frame;
    // Type 1 only when reconstructing it preserves every original AX.25 byte.
    let pid = if control == 14 && frame.len() >= 16 {
        PIDS.iter()
            .enumerate()
            .filter(|(i, _)| matches!(i, 2..=6 | 11..=15))
            .find(|&(_, &p)| p == frame[15])
            .map(|(i, _)| i)
    } else {
        None
    };
    let ui = control == 14
        && frame[14] & 0xef == 3
        && pid.is_some()
        && (frame[6] ^ frame[13]) & 0x80 != 0
        && frame[..6]
            .iter()
            .chain(&frame[7..13])
            .all(|&c| (c >> 1) <= b'_');
    if ui {
        for i in 0..6 {
            h[i] = (frame[i] >> 1) - b' ';
            h[i + 6] = (frame[i + 7] >> 1) - b' ';
        }
        h[12] = ((frame[6] >> 1) & 15) << 4 | ((frame[13] >> 1) & 15);
        put(&mut h, 6, 0, 1, 1);
        put(&mut h, 6, 4, 4, pid.unwrap());
        put(
            &mut h,
            6,
            11,
            7,
            ((frame[14] & 0x10) as usize) << 2 | 5 << 3 | ((frame[6] >> 7) as usize) << 2,
        );
        put(&mut h, 7, 1, 1, 1);
        payload = &frame[16..];
    }
    if payload.len() > 1023 {
        return Err(Il2pError::FrameTooLong);
    }
    put(&mut h, 7, 0, 1, max as usize);
    put(&mut h, 7, 11, 10, payload.len());
    Ok((h, payload))
}
fn blocks(n: usize, max: bool) -> Vec<(usize, usize)> {
    if n == 0 {
        return Vec::new();
    }
    let count = n.div_ceil(if max { 239 } else { 247 });
    let small = n / count;
    let large_count = n % count;
    let parity = if max {
        16
    } else if small <= 61 {
        2
    } else if small <= 123 {
        4
    } else if small <= 185 {
        6
    } else {
        8
    };
    (0..count)
        .map(|i| (small + usize::from(i < large_count), parity))
        .collect()
}
fn encode(frame: &[u8], max: bool) -> Result<Vec<u8>, Il2pError> {
    let (h, payload) = header(frame, max)?;
    let mut out = vec![0x55, 0xf1, 0x5e, 0x48];
    out.extend(rs::encode(&transform(&h, true), 2));
    let mut offset = 0;
    for (n, p) in blocks(payload.len(), max) {
        out.extend(rs::encode(
            &transform(&payload[offset..offset + n], true),
            p,
        ));
        offset += n;
    }
    Ok(out)
}
fn read_header(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut h = bytes.get(..15)?.to_vec();
    rs::correct(&mut h, 2)?;
    Some(transform(&h[..13], false))
}
fn restore(h: &[u8], payload: &[u8]) -> Option<Vec<u8>> {
    if field(h, 7, 1, 1) == 0 {
        validate(payload).ok()?;
        return Some(payload.to_vec());
    }
    let pid = field(h, 6, 4, 4);
    let c = field(h, 6, 11, 7) as u8;
    let ui = field(h, 6, 0, 1) != 0;
    if matches!(pid, 7..=10) || (ui && pid < 2) {
        return None;
    }
    let command = if pid >= 2 && !ui { true } else { c & 4 != 0 };
    let mut f = Vec::new();
    for a in 0..2 {
        f.extend(h[a * 6..a * 6 + 6].iter().map(|v| ((v & 63) + b' ') << 1));
        let ssid = if a == 0 { h[12] >> 4 } else { h[12] & 15 };
        f.push(0x60 | ssid << 1 | u8::from(a == 1) | if command == (a == 0) { 0x80 } else { 0 });
    }
    let pf = (c >> 2) & 0x10;
    let control = if pid == 0 {
        ((c >> 3) & 7) << 5 | pf | (c & 3) << 2 | 1
    } else if pid == 1 || ui {
        U_CONTROL[((c >> 3) & 7) as usize] | pf
    } else {
        ((c >> 3) & 7) << 5 | pf | (c & 7) << 1
    };
    if ui && control & 0xef != 3 {
        return None;
    }
    f.push(control);
    if pid >= 2 {
        f.push(PIDS[pid]);
    }
    f.extend_from_slice(payload);
    validate(&f).ok()?;
    Some(f)
}
fn decode(bytes: &[u8]) -> Option<Vec<u8>> {
    let h = read_header(bytes)?;
    let n = field(&h, 7, 11, 10);
    let mut payload = Vec::with_capacity(n);
    let mut offset = 15;
    for (size, parity) in blocks(n, field(&h, 7, 0, 1) != 0) {
        let mut code = bytes.get(offset..offset + size + parity)?.to_vec();
        rs::correct(&mut code, parity)?;
        payload.extend(transform(&code[..size], false));
        offset += size + parity;
    }
    restore(&h, &payload)
}

/// Encode FCS-free AX.25 as maximum-FEC IL2P, returning mono 48 kHz audio.
///
/// # Errors
/// Returns [`Il2pError::InvalidFrame`] for malformed AX.25 or
/// [`Il2pError::FrameTooLong`] when its IL2P payload exceeds 1023 bytes.
pub fn modulate(frame_bytes: &[u8]) -> Result<Vec<f32>, Il2pError> {
    Ok(audio(&encode(frame_bytes, true)?, false))
}
/// Audible 0x55 lead-in before the packet's own preamble byte: 29 bytes is
/// about 193 ms at 1200 baud, so VOX keys and the receiver settles before the
/// sync word (the AX.25 path sends a comparable flag preamble).
const LEAD_IN_BYTES: usize = 29;

fn audio(packet: &[u8], inverted: bool) -> Vec<f32> {
    let mut out = Vec::with_capacity((LEAD_IN_BYTES + packet.len()) * 320 + 4800);
    let mut phase: f32 = 0.0;
    for &byte in std::iter::repeat_n(&0x55, LEAD_IN_BYTES).chain(packet) {
        for bit in (0..8).rev() {
            let mark = ((byte >> bit) & 1 != 0) ^ inverted;
            let step = TAU * if mark { 1200.0 } else { 2200.0 } / 48000.0;
            for _ in 0..40 {
                out.push(phase.sin() * 0.8);
                phase = (phase + step) % TAU;
            }
        }
    }
    out.resize(out.len() + 4800, 0.0);
    out
}

#[derive(Debug, Default)]
struct Candidate {
    bytes: Vec<u8>,
    byte: u8,
    bits: u8,
    inverted: bool,
    expected: Option<usize>,
}
#[derive(Debug, Default)]
struct Slicer {
    sync: u32,
    candidates: Vec<Candidate>,
}
impl Slicer {
    fn bit(&mut self, bit: u8) -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        self.candidates.retain_mut(|c| {
            c.byte = (c.byte << 1) | (bit ^ u8::from(c.inverted));
            c.bits += 1;
            if c.bits < 8 {
                return true;
            }
            c.bytes.push(c.byte);
            c.bits = 0;
            c.byte = 0;
            if c.bytes.len() == 15 {
                let Some(h) = read_header(&c.bytes) else {
                    return false;
                };
                c.expected = Some(
                    15 + blocks(field(&h, 7, 11, 10), field(&h, 7, 0, 1) != 0)
                        .iter()
                        .map(|(n, p)| n + p)
                        .sum::<usize>(),
                );
            }
            if c.expected == Some(c.bytes.len()) {
                if let Some(f) = decode(&c.bytes) {
                    frames.push(f);
                }
                return false;
            }
            c.bytes.len() < 1118
        });
        self.sync = ((self.sync << 1) | bit as u32) & 0xffffff;
        let distance = (self.sync ^ 0xf15e48).count_ones();
        if distance <= 1 || distance >= 23 {
            if self.candidates.len() == 4 {
                // A sync-like sequence is legal inside scrambled payload.
                // Preserve the oldest validated frame while retaining probes
                // that let a truncated frame recover at the next real sync.
                let discard = self
                    .candidates
                    .iter()
                    .position(|c| c.expected.is_none())
                    .unwrap_or(1);
                self.candidates.remove(discard);
            }
            self.candidates.push(Candidate {
                inverted: distance >= 23,
                ..Default::default()
            });
        }
        frames
    }
}

/// Streaming IL2P receiver. Input is mono 48 kHz audio; output excludes FCS.
/// Four staggered timing loops follow tone transitions throughout each packet.
#[derive(Debug)]
pub struct Demodulator {
    history: [[f32; 4]; 40],
    window: [f32; 4],
    timing: [f32; 4],
    previous_bit: u8,
    phase: [f32; 2],
    samples: usize,
    slicers: [Slicer; 4],
    frames: Vec<Vec<u8>>,
    last: Option<(usize, Vec<u8>)>,
}
impl Default for Demodulator {
    fn default() -> Self {
        Self::new()
    }
}
impl Demodulator {
    /// Start with no partial or queued frames.
    pub fn new() -> Self {
        Self {
            history: [[0.0; 4]; 40],
            window: [0.0; 4],
            timing: [0.0, 10.0, 20.0, 30.0],
            previous_bit: 0,
            phase: [0.0; 2],
            samples: 0,
            slicers: std::array::from_fn(|_| Slicer::default()),
            frames: Vec::new(),
            last: None,
        }
    }
    /// Consume an arbitrary chunk, retaining bounded partial-frame state.
    pub fn process(&mut self, samples: &[f32]) {
        for &s in samples {
            let products = [
                s * self.phase[0].sin(),
                s * self.phase[0].cos(),
                s * self.phase[1].sin(),
                s * self.phase[1].cos(),
            ];
            self.phase[0] = (self.phase[0] + TAU * 1200.0 / 48000.0) % TAU;
            self.phase[1] = (self.phase[1] + TAU * 2200.0 / 48000.0) % TAU;
            // A sliding one-symbol correlator supplies a tone decision every
            // sample. Its delay is half a symbol, shared by both the transition
            // detector and the bit sampler below.
            let slot = self.samples % 40;
            for (i, product) in products.iter().enumerate() {
                self.window[i] += product - self.history[slot][i];
            }
            self.history[slot] = products;
            self.samples += 1;
            let w = self.window;
            let bit = u8::from(w[0] * w[0] + w[1] * w[1] >= w[2] * w[2] + w[3] * w[3]);
            let transition = bit != self.previous_bit;
            self.previous_bit = bit;
            for lane in 0..4 {
                // Pull each clock toward a symbol boundary on transitions.
                // Sampling halfway between boundaries centers the correlator
                // on the received symbol. Continuing these corrections prevents
                // independent audio clocks from drifting across long packets.
                let clock = &mut self.timing[lane];
                if transition {
                    let error = if *clock < 20.0 {
                        -*clock
                    } else {
                        40.0 - *clock
                    };
                    *clock += 0.1 * error;
                }
                let previous = *clock;
                *clock += 1.0;
                if *clock >= 40.0 {
                    *clock -= 40.0;
                }
                if previous >= 20.0 || *clock < 20.0 || self.samples < 40 {
                    continue;
                }
                for f in self.slicers[lane].bit(bit) {
                    if !self
                        .last
                        .as_ref()
                        .is_some_and(|(t, last)| self.samples - t <= 40 && *last == f)
                    {
                        self.frames.push(f.clone());
                        self.last = Some((self.samples, f));
                    }
                }
            }
        }
    }
    /// Drain complete FCS-free frames.
    pub fn take_frames(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.frames)
    }
    /// Discard partial and queued frames and reset symbol alignment.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(n: usize) -> Vec<u8> {
        let mut f: Vec<u8> = b"APRS  ".iter().map(|c| c << 1).collect();
        f.push(0xe0);
        f.extend(b"N0CALL".iter().map(|c| c << 1));
        f.extend([0x61, 3, 0xf0]);
        f.extend((0..n).map(|i| i as u8));
        f
    }
    #[test]
    fn boundary_matrix_both_fec_modes() {
        for n in [
            0, 1, 61, 62, 123, 124, 185, 186, 239, 240, 247, 248, 477, 478, 1023,
        ] {
            for max in [false, true] {
                let f = frame(n);
                let packet = encode(&f, max).unwrap();
                assert_eq!(
                    transform(&transform(&header(&f, max).unwrap().0, true), false),
                    header(&f, max).unwrap().0
                );
                assert_eq!(decode(&packet[4..]), Some(f), "length {n}, max {max}");
            }
        }
    }
    #[test]
    fn independent_direwolf_vectors_and_correction() {
        for line in include_str!("../../../testdata/il2p/vectors.txt").lines() {
            let (name, hex) = line.split_once(' ').unwrap();
            let parts: Vec<_> = name.split('-').collect();
            let n: usize = parts[1].parse().unwrap();
            let max = parts[2] == "1";
            let wire: Vec<u8> = hex
                .as_bytes()
                .chunks_exact(2)
                .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
                .collect();
            let f = frame(n);
            assert_eq!(encode(&f, max).unwrap()[4..], wire, "{name}");
            assert_eq!(decode(&wire), Some(f.clone()));
            let mut damaged = wire;
            damaged[4] ^= 0x65; // header's single-symbol correction bound
            let mut offset = 15;
            for (size, parity) in blocks(n, max) {
                for i in 0..parity / 2 {
                    damaged[offset + i] ^= (i + 1) as u8;
                }
                offset += size + parity;
            }
            assert_eq!(decode(&damaged), Some(f), "correction {name}");
        }
    }
    #[test]
    fn transparent_rejection_recovery_and_polarity() {
        let mut transparent = frame(20);
        transparent[6] &= 0x7f; // equal C bits require lossless type 0
        let packet = encode(&transparent, true).unwrap();
        assert_eq!(field(&read_header(&packet[4..]).unwrap(), 7, 1, 1), 0);
        assert_eq!(decode(&packet[4..]), Some(transparent));
        assert_eq!(modulate(&[0; 15]), Err(Il2pError::InvalidFrame));
        assert_eq!(modulate(&frame(1024)), Err(Il2pError::FrameTooLong));
        let good = frame(25);
        let mut packet = encode(&frame(1023), true).unwrap();
        packet.truncate(60);
        packet.extend(encode(&good, true).unwrap());
        packet.extend(encode(&good, true).unwrap());
        for inverted in [false, true] {
            for offset in [0, 7, 19, 35] {
                let mut samples = vec![0.0; offset];
                samples.extend(audio(&packet, inverted));
                let mut d = Demodulator::new();
                for chunk in samples.chunks(71) {
                    d.process(chunk);
                }
                assert_eq!(d.take_frames(), vec![good.clone(), good.clone()]);
            }
        }
        let mut slicer = Slicer::default();
        for i in 0..100_000 {
            slicer.bit((i % 2) as u8);
        }
        assert!(slicer.candidates.iter().all(|c| c.bytes.len() <= 1118));
    }
    #[test]
    fn transmission_starts_with_audible_lead_in() {
        // VOX needs audio, not silence, before the sync word arrives.
        let audio = modulate(&frame(20)).unwrap();
        let sync_at = (LEAD_IN_BYTES + 1) * 8 * 40;
        assert!(sync_at >= 9_000, "lead-in shorter than ~190 ms");
        for window in audio[..sync_at].chunks(480) {
            let peak = window.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak > 0.5, "silent 10 ms window before sync");
        }
    }
    #[test]
    fn payload_sync_patterns_do_not_evict_validated_frame() {
        let payload = transform(&[0xf1, 0x5e, 0x48].repeat(8), false);
        let mut f = frame(0);
        f.extend(payload);
        let mut rx = Demodulator::new();
        rx.process(&modulate(&f).unwrap());
        assert_eq!(rx.take_frames(), vec![f]);
    }
    #[test]
    fn malformed_headers_are_bounded_and_recover() {
        let mut slicer = Slicer::default();
        // Valid RS envelopes can advertise every possible length. Even when
        // their contents are unsupported, partial state must retire at its bound.
        for n in 0..=1023 {
            let mut h = [0; 13];
            put(&mut h, 7, 1, 1, 1);
            put(&mut h, 7, 0, 1, 1);
            put(&mut h, 7, 11, 10, n);
            put(&mut h, 6, 4, 4, 7); // reserved PID
            let mut wire = vec![0xf1, 0x5e, 0x48];
            wire.extend(rs::encode(&transform(&h, true), 2));
            wire.resize(1122, 0);
            for byte in wire {
                for bit in (0..8).rev() {
                    assert!(slicer.bit((byte >> bit) & 1).is_empty());
                }
            }
            assert!(slicer.candidates.iter().all(|c| c.bytes.len() < 1118));
            assert!(slicer.candidates.len() <= 4);
        }
        let f = frame(10);
        let mut found = Vec::new();
        for byte in encode(&f, true).unwrap() {
            for bit in (0..8).rev() {
                found.extend(slicer.bit((byte >> bit) & 1));
            }
        }
        assert_eq!(found, vec![f]);
        let mut corrupt = encode(&frame(50), true).unwrap();
        corrupt[4] ^= 1;
        corrupt[5] ^= 2;
        assert_eq!(decode(&corrupt[4..]), None);
    }
    #[test]
    fn maximum_packet_tracks_sample_clock_drift() {
        let f = include_bytes!("../../../testdata/il2p/maximum.ax25").to_vec();
        let audio = modulate(&f).unwrap();
        for ratio in [1.0, 1.000075, 0.999925] {
            // Linear sample-rate conversion models independent TX/RX clocks.
            let resampled: Vec<f32> = (0..((audio.len() - 1) as f64 * ratio) as usize)
                .map(|i| {
                    let position = i as f64 / ratio;
                    let index = position as usize;
                    let fraction = (position - index as f64) as f32;
                    audio[index] * (1.0 - fraction) + audio[index + 1] * fraction
                })
                .collect();
            for offset in [0, 7, 19, 35] {
                let mut rx = Demodulator::new();
                rx.process(&vec![0.0; offset]);
                for chunk in resampled.chunks(137) {
                    rx.process(chunk);
                }
                assert_eq!(
                    rx.take_frames(),
                    vec![f.clone()],
                    "ratio {ratio}, offset {offset}"
                );
            }
        }
    }
    #[test]
    fn streaming_audio_chunks_and_reset() {
        let f = frame(700);
        let a = modulate(&f).unwrap();
        let mut d = Demodulator::new();
        for chunk in a.chunks(137) {
            d.process(chunk);
        }
        assert_eq!(d.take_frames(), vec![f.clone()]);
        d.process(&a[..a.len() / 2]);
        d.reset();
        d.process(&a);
        assert_eq!(d.take_frames(), vec![f]);
    }
}
