//! MCS calibration: across the channel × SNR grid, emit the sounded channel capacity C and each
//! speed level's FER, so per-level minimum-capacity thresholds C_min(L) can be derived. Robust
//! profile. Pass a seed as arg 1 (default 0xCA11B for calibration; use a different seed to validate
//! generalization). Output is parseable: `DATA <channel> <snr> <C> <level> <fer>`.

use coppa_bench::scenario::{mode_for_level, profile_by_name, ChannelSpec, MODES, SAMPLE_RATE};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::coppa_modem::CoppaModem;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_ml::channel_capacity;
use coppa_protocol::modem::transceiver::CoppaTransceiver;

const TRIALS: usize = 40;

fn apply_channel(sig: &[f32], ch: ChannelSpec, snr: f32, seed: u64) -> Vec<f32> {
    match ch {
        ChannelSpec::Awgn => coppa_channel::awgn_seeded(sig, snr, seed ^ 0x5555),
        ChannelSpec::Watterson(p) => {
            let f = coppa_channel::watterson::watterson(
                sig,
                SAMPLE_RATE as f32,
                &p.config(),
                seed ^ 0x3333,
            );
            coppa_channel::awgn_seeded(&f, snr, seed ^ 0x5555)
        }
    }
}

fn make_header(level: u8, len: u16) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: level,
        seq_num: 0,
        payload_len: len,
    }
}

fn fer(
    profile: &coppa_codec::ofdm::CoppaProfile,
    level: u8,
    ch: ChannelSpec,
    snr: f32,
    base: u64,
) -> f64 {
    let tx = CoppaTransceiver::new(profile.clone(), 1);
    let pfb = mode_for_level(level).unwrap().payload_bytes();
    let mut ok = 0usize;
    for t in 0..TRIALS {
        let seed = base.wrapping_add(t as u64);
        let payload: Vec<u8> = (0..pfb)
            .map(|i| (seed.wrapping_add(i as u64).wrapping_mul(2654435761) >> 24) as u8)
            .collect();
        let sig = tx.transmit(&make_header(level, pfb as u16), &payload);
        let faded = apply_channel(&sig, ch, snr, seed);
        if let Ok((_h, p)) = tx.receive(&faded) {
            if p.len() >= pfb && p[..pfb] == payload[..] {
                ok += 1;
            }
        }
    }
    1.0 - ok as f64 / TRIALS as f64
}

/// Sounded capacity, AVERAGED over several frames — a single sounding frame is too noisy on deep
/// fading (one frame can land in a deep fade), so average to get a stable channel-quality estimate.
fn sound_capacity(
    profile: &coppa_codec::ofdm::CoppaProfile,
    ch: ChannelSpec,
    snr: f32,
    seed: u64,
) -> f32 {
    let tx = CoppaTransceiver::new(profile.clone(), 1);
    let modem = CoppaModem::new(profile.clone(), 1);
    let pfb = mode_for_level(2).unwrap().payload_bytes();
    let payload = vec![0x5Au8; pfb];
    let sig = tx.transmit(&make_header(2, pfb as u16), &payload);
    let mut acc = 0.0f32;
    let mut n = 0usize;
    for s in 0..8u64 {
        let faded = apply_channel(
            &sig,
            ch,
            snr,
            seed.wrapping_add(s.wrapping_mul(0x9E37_79B9)),
        );
        if let Some((_h, _eq, nv)) = modem.demodulate_soft_coded(&faded) {
            acc += channel_capacity(&nv);
            n += 1;
        }
    }
    if n > 0 {
        acc / n as f32
    } else {
        0.0
    }
}

fn main() {
    let seed: u64 = std::env::args()
        .nth(1)
        .and_then(|s| {
            u64::from_str_radix(s.trim_start_matches("0x"), 16)
                .ok()
                .or_else(|| s.parse().ok())
        })
        .unwrap_or(0x000C_A11B);
    let profile = profile_by_name("robust").unwrap();
    let channels = [
        (ChannelSpec::Awgn, "AWGN"),
        (ChannelSpec::Watterson(WattersonPreset::Good), "Good"),
        (
            ChannelSpec::Watterson(WattersonPreset::Moderate),
            "Moderate",
        ),
        (ChannelSpec::Watterson(WattersonPreset::Poor), "Poor"),
    ];
    let snrs = [6.0f32, 12.0, 18.0, 24.0, 30.0];
    eprintln!("calibration seed=0x{seed:X}");
    for (ch, cname) in channels {
        for &snr in &snrs {
            let c = sound_capacity(&profile, ch, snr, seed);
            for m in MODES {
                let f = fer(&profile, m.level, ch, snr, seed);
                println!("DATA {cname} {snr:.0} {c:.2} {} {f:.3}", m.level);
            }
        }
    }
}
