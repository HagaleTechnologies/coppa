//! COP-69 research probe (throwaway measurement, not for merge).
//!
//! Measures, with real `CoppaTransceiver::transmit` calls:
//!   A. frame sample counts / airtime / overhead fractions for codewords 1..8 at
//!      the levels that matter (L2 = daemon default, L8/L9 = 64-QAM),
//!   B. the exact 21-byte session-ACK payload's airtime (is it a full frame?),
//!   C. whether the daemon's *streaming* RX path (`CoppaCore::push_samples`)
//!      can decode a multi-codeword frame today,
//!   D. a stop-and-wait airtime model assembled from the measured parts.

use coppa_codec::ofdm::coppa_modem::SPEED_LEVELS;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_engine::{CoppaCore, EngineConfig, OfdmBand};
use coppa_protocol::mac::{MacFrameType, MacPdu};
use coppa_protocol::modem::speed_levels::{max_multi_payload_for_level, max_payload_for_level};
use coppa_protocol::modem::transceiver::{CoppaTransceiver, MAX_CODEWORDS};
use coppa_protocol::transport::TransportPdu;

const SAMPLE_RATE: f64 = 48_000.0;
// Half-duplex fixed costs measured/documented by COP-60 (defaults):
const RX_TURNAROUND_S: f64 = 0.500; // [radio] turnaround_ms default
const PTT_PRE_S: f64 = 0.050; // ptt_pre_delay_ms default
const PTT_TAIL_S: f64 = 0.200; // ptt_tail_delay_ms default

fn make_header(level: u8, payload_len: u16, codewords: u8) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: level,
        seq_num: 0,
        payload_len,
        codewords,
    }
}

fn header_syms(profile: &CoppaProfile) -> usize {
    // 144 protected-header coded bits over data_carriers, one OFDM symbol each
    (144usize).div_ceil(profile.data_carriers)
}

fn payload_syms(level: u8, profile: &CoppaProfile, codewords: usize) -> usize {
    let bps = SPEED_LEVELS
        .iter()
        .find(|s| s.level == level)
        .expect("known level")
        .bits_per_symbol as usize;
    let coded_constellation_syms = 1944usize.div_ceil(bps) * codewords;
    coded_constellation_syms.div_ceil(profile.data_carriers)
}

fn main() {
    println!("=== A. frame airtime / overhead (measured via CoppaTransceiver::transmit) ===");
    let cases: Vec<(&str, CoppaProfile, u8)> = vec![
        (
            "L2 hf_standard (daemon default BPSK)",
            CoppaProfile::hf_standard(),
            2,
        ),
        ("L4 hf_standard (QPSK 3/4)", CoppaProfile::hf_standard(), 4),
        (
            "L8 hf_standard (64QAM 2/3, HF band)",
            CoppaProfile::hf_standard(),
            8,
        ),
        (
            "L8 vhf_wide (64QAM 2/3, VHF band)",
            CoppaProfile::vhf_wide(),
            8,
        ),
        (
            "L9 vhf_wide (64QAM 5/6, VHF-only)",
            CoppaProfile::vhf_wide(),
            9,
        ),
    ];
    for (label, profile, level) in &cases {
        let tx = CoppaTransceiver::new(profile.clone(), 1);
        let symbol_len = profile.fft_size + profile.cp_samples;
        let hsyms = header_syms(profile);
        for cw in [1usize, 2, 4, MAX_CODEWORDS as usize] {
            let budget = max_multi_payload_for_level(*level, cw as u8).expect("known level");
            let payload: Vec<u8> = (0..budget).map(|i| (i * 37 + 11) as u8).collect();
            let header = make_header(*level, payload.len() as u16, cw as u8);
            let samples = tx
                .transmit(&header, &payload)
                .unwrap_or_else(|e| panic!("{label} cw={cw}: transmit failed: {e:?}"));
            let nsamples = samples.len();
            let secs = nsamples as f64 / SAMPLE_RATE;
            let syms = nsamples / symbol_len;
            let predicted_syms = 3 + hsyms + payload_syms(*level, profile, cw);
            let overhead = (3 + hsyms) as f64 / syms as f64;
            let plen = payload.len();
            println!(
                "{label:40} cw={cw} payload={plen:5}B samples={nsamples:7} \
                 airtime={secs:.4}s syms={syms}(predicted {predicted_syms}) \
                 fixed_overhead={:.1}% throughput={:.0}B/s",
                overhead * 100.0,
                plen as f64 / secs,
            );
        }
    }

    println!("\n=== B. the 21-byte session ACK: airtime at the data level ===");
    let dest = coppa_protocol::mac::Callsign::new("W1AW").expect("callsign");
    let src = coppa_protocol::mac::Callsign::new("K1TTT").expect("callsign");
    let ack_pdu = TransportPdu::new_ack(0, 1, 0);
    let mac = MacPdu::new(MacFrameType::Ack, dest, src, 0, ack_pdu.to_bytes());
    let ack_bytes = mac.to_bytes();
    println!(
        "session ACK PDU bytes = {} (MacPdu::HEADER_SIZE={} + TransportPdu header=7)",
        ack_bytes.len(),
        MacPdu::HEADER_SIZE
    );
    for (label, _profile, level) in &cases {
        // Same band rule the daemon gets from a named profile (coppad default
        // HF_STANDARD => OfdmBand::Hf; VHF_FAST => OfdmBand::Vhf). Level 9 is
        // VHF-only, so it runs on a Vhf-band engine; every other level here is HF.
        let band = if *level == 9 {
            OfdmBand::Vhf
        } else {
            OfdmBand::Hf
        };
        let engine = CoppaCore::with_config(EngineConfig {
            speed_level: *level,
            ofdm_band: band,
            ..EngineConfig::default()
        });
        let ack_samples = engine
            .encode_bytes(&ack_bytes)
            .unwrap_or_else(|e| panic!("encode ACK failed: {e}"));
        let ack_secs = ack_samples.len() as f64 / SAMPLE_RATE;
        // A full-budget DATA frame from the SAME engine (same profile+level).
        let per_cw = max_payload_for_level(*level).expect("known level");
        let data_payload: Vec<u8> = (0..per_cw).map(|i| (i * 37 + 11) as u8).collect();
        let data_samples = engine
            .encode_bytes(&data_payload)
            .unwrap_or_else(|e| panic!("encode data failed: {e}"));
        let data_secs = data_samples.len() as f64 / SAMPLE_RATE;
        println!(
            "{label:40} ACK-frame airtime={ack_secs:.4}s  full-data-frame={data_secs:.4}s  \
             ACK_is_full_frame={}",
            (ack_secs - data_secs).abs() < 1e-9
        );
    }

    println!("\n=== C. can the daemon's streaming RX path decode a multi-codeword frame? ===");
    {
        // An HF-band engine at level 8 (what coppad runs with a named HF profile
        // at 64-QAM: OfdmBand::Hf keeps it on hf_standard, per COP-23).
        let mut engine = CoppaCore::with_config(EngineConfig {
            speed_level: 8,
            ofdm_band: OfdmBand::Hf,
            ..EngineConfig::default()
        });
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let symbol_len =
            CoppaProfile::hf_standard().fft_size + CoppaProfile::hf_standard().cp_samples;
        let margin = 8 * symbol_len; // leading + trailing silence, as a real RX sees

        // Control: single-codeword frame through the same streaming path.
        let per_cw = max_payload_for_level(8).expect("known level");
        let p1: Vec<u8> = (0..per_cw).map(|i| (i * 37 + 11) as u8).collect();
        let s1 = tx
            .transmit(&make_header(8, p1.len() as u16, 1), &p1)
            .expect("1-cw frame");
        let mut feed = vec![0.0f32; margin];
        feed.extend_from_slice(&s1);
        feed.extend_from_slice(&[0.0; 24000]);
        let frames = engine.push_samples(&feed);
        println!(
            "streaming RX, 1 codeword  ({}B): decoded {} frame(s) [expect 1]",
            p1.len(),
            frames.len()
        );

        // The experiment: 8-codeword frame, same streaming path.
        engine = CoppaCore::with_config(EngineConfig {
            speed_level: 8,
            ofdm_band: OfdmBand::Hf,
            ..EngineConfig::default()
        });
        let budget = max_multi_payload_for_level(8, MAX_CODEWORDS).expect("known level");
        let p8: Vec<u8> = (0..budget).map(|i| (i * 37 + 11) as u8).collect();
        let s8 = tx
            .transmit(&make_header(8, p8.len() as u16, MAX_CODEWORDS), &p8)
            .expect("8-cw frame");
        let mut feed8 = vec![0.0f32; margin];
        feed8.extend_from_slice(&s8);
        feed8.extend_from_slice(&[0.0; 48000]);
        let frames8 = engine.push_samples(&feed8);
        println!(
            "streaming RX, 8 codewords ({}B): decoded {} frame(s) [1 would mean supported] \
             payload_ok={}",
            p8.len(),
            frames8.len(),
            frames8
                .first()
                .map(|f| f.payload.as_ref().map(|p| p == &p8).unwrap_or(false))
                .unwrap_or(false)
        );

        // Batch-path control (what the unit tests exercise): same 8-cw audio.
        match tx.receive(&s8) {
            Ok((h, rx_payload, _)) => println!(
                "batch RX, 8 codewords: header.codewords={} payload_ok={} (batch path supports it)",
                h.codewords,
                rx_payload == p8
            ),
            Err(e) => println!("batch RX, 8 codewords: FAILED: {e:?}"),
        }
    }

    println!("\n=== D. stop-and-wait cycle model (measured parts + fixed costs) ===");
    // fixed per-cycle half-duplex cost: RX turnaround guard + PTT pre + tail,
    // paid twice (before and after each keyed transmission).
    let fixed_per_tx = RX_TURNAROUND_S + PTT_PRE_S + PTT_TAIL_S;
    println!("fixed half-duplex cost per keyed TX = {fixed_per_tx:.3}s (500ms turnaround guard + 50ms pre + 200ms tail)");
    for (label, profile, level, ack_kind) in [
        (
            "today: 1cw data + full-frame ACK",
            CoppaProfile::hf_standard(),
            8u8,
            "full",
        ),
        (
            "multi: 8cw data + full-frame ACK",
            CoppaProfile::hf_standard(),
            8,
            "full",
        ),
        (
            "multi: 8cw data + short ACK (hypothetical floor = preamble+header only)",
            CoppaProfile::hf_standard(),
            8,
            "short",
        ),
        (
            "today: 1cw data + full-frame ACK (L2 default)",
            CoppaProfile::hf_standard(),
            2,
            "full",
        ),
        (
            "multi: 8cw data + full-frame ACK (L2 default)",
            CoppaProfile::hf_standard(),
            2,
            "full",
        ),
        (
            "multi: 8cw data + short ACK floor (L2 default)",
            CoppaProfile::hf_standard(),
            2,
            "short",
        ),
        (
            "today: 1cw + full ACK (L9 vhf_wide)",
            CoppaProfile::vhf_wide(),
            9,
            "full",
        ),
        (
            "multi: 8cw + full ACK (L9 vhf_wide)",
            CoppaProfile::vhf_wide(),
            9,
            "full",
        ),
        (
            "multi: 8cw + short ACK floor (L9 vhf_wide)",
            CoppaProfile::vhf_wide(),
            9,
            "short",
        ),
    ] {
        let tx = CoppaTransceiver::new(profile.clone(), 1);
        let symbol_len = profile.fft_size + profile.cp_samples;
        let data_cw = if label.starts_with("multi") {
            MAX_CODEWORDS as usize
        } else {
            1
        };
        let budget = max_multi_payload_for_level(level, data_cw as u8).expect("known level");
        let payload: Vec<u8> = (0..budget).map(|i| (i * 37 + 11) as u8).collect();
        let header = make_header(level, payload.len() as u16, data_cw as u8);
        let data_s = tx.transmit(&header, &payload).expect("data frame").len() as f64 / SAMPLE_RATE;
        let ack_s = match ack_kind {
            // today's ACK: a full single-codeword frame at the data level
            "full" => {
                let a = make_header(level, 21, 1);
                tx.transmit(&a, &[0u8; 21]).expect("ack frame").len() as f64 / SAMPLE_RATE
            }
            // hypothetical floor: no payload codeword at all, preamble+header only
            "short" => (3 + header_syms(&profile)) as f64 * symbol_len as f64 / SAMPLE_RATE,
            _ => unreachable!(),
        };
        let cycle = data_s + ack_s + 2.0 * fixed_per_tx;
        println!(
            "{label:62} data={data_s:.3}s ack={ack_s:.3}s cycle={cycle:.3}s per {}B -> {:.0}B/s  ACK_share_of_airtime={:.0}%",
            payload.len(),
            payload.len() as f64 / cycle,
            ack_s / (data_s + ack_s) * 100.0
        );
    }
}
