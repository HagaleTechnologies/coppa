//! Compare adaptive MCS selectors on a HELD-OUT seed (different from calibration): calibrated
//! per-level thresholds vs the flat-margin rule vs the oracle. Robust profile, 8-frame averaged
//! sounding. Pass a seed as arg 1 (default 0x5A1AD, distinct from the 0xCA11B calibration seed).

use coppa_bench::scenario::{mode_for_level, profile_by_name, ChannelSpec, MODES, SAMPLE_RATE};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::coppa_modem::CoppaModem;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_ml::{channel_capacity, select_speed_level, select_speed_level_calibrated};
use coppa_protocol::modem::transceiver::CoppaTransceiver;

const TRIALS: usize = 40;
const MARGIN: f32 = 2.5;

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
    let (mut ok, mut airtime) = (0usize, 0f64);
    for t in 0..TRIALS {
        let seed = base.wrapping_add(t as u64);
        let payload: Vec<u8> = (0..pfb)
            .map(|i| (seed.wrapping_add(i as u64).wrapping_mul(2654435761) >> 24) as u8)
            .collect();
        let sig = tx.transmit(&make_header(level, pfb as u16), &payload);
        airtime = sig.len() as f64 / SAMPLE_RATE as f64;
        let faded = apply_channel(&sig, ch, snr, seed);
        if let Ok((_h, p)) = tx.receive(&faded) {
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
    let sig = tx.transmit(&make_header(2, pfb as u16), &payload);
    let (mut acc, mut n) = (0.0f32, 0usize);
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
        .unwrap_or(0x0005_A1AD);
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

    println!("HELD-OUT seed=0x{seed:X}  (calibration used 0xCA11B)");
    println!("channel   snr   C     cal  marg  | calGP  margGP  oracle | calR margR");
    let (mut tc, mut tm, mut to) = (0f64, 0f64, 0f64);
    for (ch, cname) in channels {
        for &snr in &snrs {
            let c = sound_capacity(&profile, ch, snr, seed);
            let lc = select_speed_level_calibrated(c);
            let lm = select_speed_level(c, MARGIN);
            let gc = goodput(&profile, lc, ch, snr, seed ^ 0xBEEF);
            let gm = goodput(&profile, lm, ch, snr, seed ^ 0xBEEF);
            let og = levels
                .iter()
                .map(|&l| goodput(&profile, l, ch, snr, seed ^ 0xBEEF))
                .fold(0f64, f64::max);
            tc += gc;
            tm += gm;
            to += og;
            println!(
                "{:<8} {:>4.0}  {:>4.1}  L{:<3} L{:<3} | {:>5.0}  {:>5.0}  {:>5.0} | {:.2} {:.2}",
                cname,
                snr,
                c,
                lc,
                lm,
                gc,
                gm,
                og,
                if og > 0.0 { gc / og } else { 1.0 },
                if og > 0.0 { gm / og } else { 1.0 }
            );
        }
    }
    println!(
        "\nAGGREGATE: calibrated/oracle = {:.3}   margin2.5/oracle = {:.3}",
        tc / to.max(1.0),
        tm / to.max(1.0)
    );
}
