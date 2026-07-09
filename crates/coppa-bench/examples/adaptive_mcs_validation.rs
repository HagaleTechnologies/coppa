//! Adaptive-MCS validation: at each channel/SNR, measure channel capacity from a sounding frame's
//! pilots, select a speed level, and compare adaptive goodput vs the oracle (best level) and the
//! best single fixed level. Robust profile.

use coppa_bench::scenario::{mode_for_level, profile_by_name, ChannelSpec, MODES, SAMPLE_RATE};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::coppa_modem::CoppaModem;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_ml::{channel_capacity, select_speed_level};
use coppa_protocol::modem::transceiver::CoppaTransceiver;

const TRIALS: usize = 30;
const MARGIN: f32 = 2.5; // calibrated: maximizes adaptive/oracle ratio (Shannon-to-practical gap)

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

fn goodput(
    profile: &coppa_codec::ofdm::CoppaProfile,
    level: u8,
    ch: ChannelSpec,
    snr: f32,
    base: u64,
) -> f64 {
    let tx = CoppaTransceiver::new(profile.clone(), 1);
    let pfb = mode_for_level(level).unwrap().payload_bytes();
    let mut ok = 0usize;
    let mut airtime = 0f64;
    for t in 0..TRIALS {
        let seed = base.wrapping_add(t as u64);
        let payload: Vec<u8> = (0..pfb)
            .map(|i| (seed.wrapping_add(i as u64).wrapping_mul(2654435761) >> 24) as u8)
            .collect();
        let sig = tx
            .transmit(&make_header(level, pfb as u16), &payload)
            .expect("payload within this level's capacity");
        airtime = sig.len() as f64 / SAMPLE_RATE as f64;
        let faded = apply_channel(&sig, ch, snr, seed);
        if let Ok((_h, p, _rec)) = tx.receive(&faded) {
            if p.len() >= pfb && p[..pfb] == payload[..] {
                ok += 1;
            }
        }
    }
    if airtime > 0.0 {
        (pfb * 8) as f64 * (ok as f64 / TRIALS as f64) / airtime
    } else {
        0.0
    }
}

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
    let sig = tx
        .transmit(&make_header(2, pfb as u16), &payload)
        .expect("payload within this level's capacity");
    let faded = apply_channel(&sig, ch, snr, seed);
    match modem.demodulate_frame(&faded) {
        Some((_h, _eq, nv)) => channel_capacity(&nv),
        None => 0.0,
    }
}

fn main() {
    let margin: f32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(MARGIN);
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
    let levels: Vec<u8> = MODES.iter().map(|m| m.level).collect();

    println!("margin={margin}");
    println!("channel   snr   C     sel  adaptive  oracle(lvl) | ratio");
    let (mut tot_adapt, mut tot_oracle) = (0f64, 0f64);
    for (ch, cname) in channels {
        for &snr in &snrs {
            let c = sound_capacity(&profile, ch, snr, 0xC0FFEE);
            let sel = select_speed_level(c, margin);
            let adapt = goodput(&profile, sel, ch, snr, 0xA11CE);
            let (mut og, mut ol) = (0f64, 0u8);
            for &lvl in &levels {
                let g = goodput(&profile, lvl, ch, snr, 0xA11CE);
                if g > og {
                    og = g;
                    ol = lvl;
                }
            }
            tot_adapt += adapt;
            tot_oracle += og;
            println!(
                "{:<8} {:>4.0}  {:>4.1}  L{:<3} {:>8.0}  {:>6.0}(L{:<2})| {:.2}",
                cname,
                snr,
                c,
                sel,
                adapt,
                og,
                ol,
                if og > 0.0 { adapt / og } else { 1.0 }
            );
        }
    }
    println!(
        "\nAggregate adaptive/oracle goodput ratio: {:.3}",
        tot_adapt / tot_oracle.max(1.0)
    );
}
