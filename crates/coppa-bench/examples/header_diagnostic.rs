//! Header failure-contribution diagnostic.
//!
//! The frame header is hard-decision BPSK with no FEC and no integrity check. The future-work
//! backlog flagged that headers fail occasionally even when the payload is recoverable, which would
//! lose otherwise-good frames. Before committing to a soft/FEC-protected header (a wire-format
//! change), this quantifies the prize: across the channel x SNR x level grid, of every frame that
//! `receive()` drops, what fraction was lost to a corrupt header vs a genuine payload limit?
//!
//! Attribution is precise. `receive()` only uses two header fields for decode — `speed_level`
//! (selects the demod/decoder) and `payload_len` (sizes the output). Corruption in the other fields
//! (version/phy_mode/bandwidth/fec_type/seq_num) is parsed but ignored, so it cannot lose a frame.
//! A failed frame is therefore HEADER-caused only if the header is unparseable (invalid frame_type
//! aborts demod) or a decode-relevant field is wrong; otherwise it is PAYLOAD-caused.
//!
//! Robust profile, levels 2/3/6 (BPSK/QPSK/16QAM 1/2). Output is parseable:
//! `DATA <channel> <snr> <level> <trials> <fail> <hdr_caused> <payload_caused>`.

use coppa_bench::scenario::{mode_for_level, profile_by_name, ChannelSpec, SAMPLE_RATE};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::coppa_modem::CoppaModem;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_protocol::modem::transceiver::CoppaTransceiver;

const TRIALS: usize = 100;

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

/// Per-(channel, snr, level) tally.
#[derive(Default)]
struct Tally {
    fail: usize,
    /// Failures where the header was unparseable or a decode-relevant field (speed_level /
    /// payload_len) was wrong — a protected header could have prevented these.
    hdr_caused: usize,
    /// Failures where the decode-relevant header fields were correct but the payload still failed —
    /// a genuine PHY/FEC limit that a protected header cannot help.
    payload_caused: usize,
}

fn run_cell(
    profile: &coppa_codec::ofdm::CoppaProfile,
    level: u8,
    ch: ChannelSpec,
    snr: f32,
    base: u64,
) -> Tally {
    let tx = CoppaTransceiver::new(profile.clone(), 1);
    let modem = CoppaModem::new(profile.clone(), 1);
    let pfb = mode_for_level(level).unwrap().payload_bytes();
    let truth = make_header(level, pfb as u16);
    let mut tally = Tally::default();

    for t in 0..TRIALS {
        let seed = base.wrapping_add(t as u64);
        let payload: Vec<u8> = (0..pfb)
            .map(|i| (seed.wrapping_add(i as u64).wrapping_mul(2654435761) >> 24) as u8)
            .collect();
        let sig = tx
            .transmit(&truth, &payload)
            .expect("payload within this level's capacity");
        let faded = apply_channel(&sig, ch, snr, seed);

        let ok =
            matches!(tx.receive(&faded), Ok((_h, p)) if p.len() >= pfb && p[..pfb] == payload[..]);
        if ok {
            continue;
        }

        tally.fail += 1;
        // Attribute the failure. Re-demodulate to recover the parsed header (receive() discards it
        // on error). None => unparseable header (invalid frame_type) or sync loss — frame aborted
        // before payload decode, so header-caused for our purposes.
        match modem.demodulate_frame(&faded) {
            None => tally.hdr_caused += 1,
            Some((parsed, _sym, _nv)) => {
                let decode_relevant_ok = parsed.speed_level == truth.speed_level
                    && parsed.payload_len == truth.payload_len;
                if decode_relevant_ok {
                    tally.payload_caused += 1;
                } else {
                    tally.hdr_caused += 1;
                }
            }
        }
    }
    tally
}

fn main() {
    let base: u64 = std::env::args()
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
    let levels = [2u8, 3, 6]; // BPSK 1/2, QPSK 1/2, 16QAM 1/2
    let snrs = [6.0f32, 12.0, 18.0, 24.0, 30.0];

    eprintln!("header diagnostic base=0x{base:X} trials={TRIALS}");
    println!("DATA channel snr level trials fail hdr_caused payload_caused");
    for (ch, cname) in channels {
        for &level in &levels {
            for &snr in &snrs {
                let tally = run_cell(&profile, level, ch, snr, base);
                println!(
                    "DATA {cname} {snr:.0} {level} {TRIALS} {} {} {}",
                    tally.fail, tally.hdr_caused, tally.payload_caused
                );
            }
        }
    }
}
