//! Task 7 perf evidence: run in release mode.
//!
//! `cargo run --release -p coppa-protocol --example streaming_perf`
//!
//! Measures the two numbers the Task 7 brief asks for, plus a component
//! breakdown of [2] (added while investigating why it initially missed its
//! target — see the Task 7 report for the full writeup):
//! 1. 10 s of pure noise pushed through `StreamingReceiver::push_samples` in
//!    512-sample chunks, as a multiple of realtime (target: <= 0.005x realtime).
//!    [1b] repeats this on a VHF profile (no RX bandpass filter at all) to show
//!    how much of an HF profile's cost is that filter.
//! 2. One full frame decode, end-to-end through `push_samples` (target: <= 5 ms;
//!    was 54.3 ms before Task 7's per-level transceiver caches). [2b]-[2e] break
//!    that down into `receive_with_metrics`, `demodulate_frame` (sync + OFDM
//!    demod), LDPC decode, and the RX bandpass filter alone, to show where the
//!    remaining time actually goes.
use std::time::Instant;

use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::modem::streaming::StreamingReceiver;
use coppa_protocol::modem::transceiver::CoppaTransceiver;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

fn main() {
    let profile = CoppaProfile::hf_standard();
    let vhf_profile = CoppaProfile::vhf_wide();

    // --- 1. 10 s of pure noise, 512-sample chunks ---
    {
        let mut rx = StreamingReceiver::new(profile.clone(), 1);
        let mut rng = StdRng::seed_from_u64(1);
        let sr = profile.sample_rate as usize;
        let total_samples = 10 * sr;
        let chunk = 512usize;

        let start = Instant::now();
        let mut pushed = 0usize;
        while pushed < total_samples {
            let n = chunk.min(total_samples - pushed);
            let samples: Vec<f32> = (0..n)
                .map(|_| rng.random_range(-0.05f32..0.05f32))
                .collect();
            let frames = rx.push_samples(&samples);
            assert!(frames.is_empty(), "pure noise must not produce frames");
            pushed += n;
        }
        let elapsed = start.elapsed();
        let realtime_secs = total_samples as f64 / sr as f64;
        let ratio = elapsed.as_secs_f64() / realtime_secs;
        println!(
            "[1] 10s noise / 512-sample chunks: {:.3} ms wall, {:.6}x realtime (target <= 0.005x)",
            elapsed.as_secs_f64() * 1000.0,
            ratio
        );
    }

    // --- 1b. Same noise-only test on VHF profile (no RX bandpass filter at all)
    // to isolate how much of [1]'s cost is the continuous 601-tap HF RX filter.
    {
        let mut rx = StreamingReceiver::new(vhf_profile.clone(), 1);
        let mut rng = StdRng::seed_from_u64(2);
        let sr = vhf_profile.sample_rate as usize;
        let total_samples = 10 * sr;
        let chunk = 512usize;
        let start = Instant::now();
        let mut pushed = 0usize;
        while pushed < total_samples {
            let n = chunk.min(total_samples - pushed);
            let samples: Vec<f32> = (0..n)
                .map(|_| rng.random_range(-0.05f32..0.05f32))
                .collect();
            let frames = rx.push_samples(&samples);
            assert!(frames.is_empty());
            pushed += n;
        }
        let elapsed = start.elapsed();
        let realtime_secs = total_samples as f64 / sr as f64;
        let ratio = elapsed.as_secs_f64() / realtime_secs;
        println!(
            "[1b] 10s noise / 512-sample chunks, VHF (no RX bandpass): {:.3} ms wall, {:.6}x realtime",
            elapsed.as_secs_f64() * 1000.0,
            ratio
        );
    }

    // --- 2. One full frame decode, end-to-end ---
    {
        let tx = CoppaTransceiver::new(profile.clone(), 1);
        let header = CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level: 2,
            seq_num: 0,
            payload_len: 60,
        };
        let payload = vec![0xA5u8; 60];
        let frame = tx
            .transmit(&header, &payload)
            .expect("payload within this level's capacity");

        let symbol_len = profile.fft_size + profile.cp_samples;
        let mut one_shot = vec![0.0f32; 4 * symbol_len];
        one_shot.extend_from_slice(&frame);
        one_shot.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len));

        // One long-lived receiver (as a real daemon/FFI caller would use it — the
        // per-level caches in `CoppaTransceiver::new` are a one-time construction
        // cost, not part of the steady-state per-frame budget being measured
        // here), fed repeated [silence-gap + frame] segments so each trial times
        // just one frame's push_samples cost.
        let mut rx = StreamingReceiver::new(profile.clone(), 1);
        let warm_frames = rx.push_samples(&one_shot);
        assert_eq!(warm_frames.len(), 1, "warm-up decode should succeed");

        const TRIALS: usize = 20;
        let mut total = std::time::Duration::ZERO;
        for _ in 0..TRIALS {
            let mut segment = vec![0.0f32; 4 * symbol_len];
            segment.extend_from_slice(&frame);
            segment.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len));

            let start = Instant::now();
            let frames = rx.push_samples(&segment);
            total += start.elapsed();
            assert_eq!(frames.len(), 1, "frame should decode every trial");
        }
        let avg_ms = total.as_secs_f64() * 1000.0 / TRIALS as f64;
        println!(
            "[2] One full frame decode (avg of {TRIALS}, steady-state, excludes one-time \
             StreamingReceiver::new): {avg_ms:.3} ms (target <= 5 ms; was 54.3 ms)"
        );

        // --- 2b. Component breakdown: receive_with_metrics alone (no sync/ring). ---
        let tx2 = CoppaTransceiver::new(profile.clone(), 1);
        const TRIALS2: usize = 20;
        let mut total_receive = std::time::Duration::ZERO;
        for _ in 0..TRIALS2 {
            let start = Instant::now();
            let r = tx2.receive_with_metrics(&frame);
            total_receive += start.elapsed();
            assert!(r.is_ok());
        }
        println!(
            "[2b] receive_with_metrics alone (avg of {TRIALS2}): {:.3} ms",
            total_receive.as_secs_f64() * 1000.0 / TRIALS2 as f64
        );

        // --- 2c. demodulate_frame alone (sync + demod, no LDPC/interleave/etc). ---
        let modem = coppa_codec::ofdm::coppa_modem::CoppaModem::new(profile.clone(), 1);
        let mut total_demod = std::time::Duration::ZERO;
        for _ in 0..TRIALS2 {
            let start = Instant::now();
            let r = modem.demodulate_frame(&frame);
            total_demod += start.elapsed();
            assert!(r.is_some());
        }
        println!(
            "[2c] demodulate_frame alone (sync+OFDM demod, avg of {TRIALS2}): {:.3} ms",
            total_demod.as_secs_f64() * 1000.0 / TRIALS2 as f64
        );

        // --- 2d. LDPC decode_checked alone, given the actual LLRs from this frame. ---
        use coppa_protocol::fec::ldpc::codes::CodeRate;
        use coppa_protocol::fec::ldpc::LdpcCodec;
        let (_h, eq_symbols, noise_vars) = modem.demodulate_frame(&frame).unwrap();
        let mapper = coppa_codec::bpsk::BpskMapper;
        use coppa_codec::traits::ConstellationMapper;
        let coded_bits_needed = 1944usize;
        let mut llrs = Vec::with_capacity(coded_bits_needed);
        for (i, &sym) in eq_symbols.iter().take(coded_bits_needed).enumerate() {
            let nv = if i < noise_vars.len() {
                noise_vars[i].max(0.001)
            } else {
                0.01
            };
            llrs.extend(mapper.demap_soft(sym, nv));
        }
        llrs.truncate(coded_bits_needed);
        llrs.resize(coded_bits_needed, 0.0);
        for l in &mut llrs {
            *l = l.clamp(-20.0, 20.0);
        }
        let interleaver = coppa_codec::ofdm::interleaver::BlockInterleaver::new(
            coded_bits_needed,
            profile.data_carriers,
        );
        let deint = interleaver.deinterleave(&llrs);
        let codec = LdpcCodec::new(CodeRate::Rate1_2);
        let mut total_ldpc = std::time::Duration::ZERO;
        let mut converged_count = 0;
        for _ in 0..TRIALS2 {
            let start = Instant::now();
            let (_bits, converged) = codec.decode_checked(&deint);
            total_ldpc += start.elapsed();
            if converged {
                converged_count += 1;
            }
        }
        println!(
            "[2d] LDPC decode_checked alone (avg of {TRIALS2}, converged {converged_count}/{TRIALS2}): {:.3} ms",
            total_ldpc.as_secs_f64() * 1000.0 / TRIALS2 as f64
        );

        // --- 2e. RX bandpass filter_block alone, over one frame's worth of samples. ---
        let rx_bpf = coppa_dsp::fir::Fir::new(coppa_dsp::fir::design_bandpass(
            601,
            profile.sample_rate as f32,
            250.0,
            2850.0,
        ));
        let mut total_filt = std::time::Duration::ZERO;
        for _ in 0..TRIALS2 {
            let start = Instant::now();
            let filtered = rx_bpf.filter_block(&frame);
            total_filt += start.elapsed();
            std::hint::black_box(&filtered);
        }
        println!(
            "[2e] RX bandpass filter_block alone over one frame (avg of {TRIALS2}): {:.3} ms",
            total_filt.as_secs_f64() * 1000.0 / TRIALS2 as f64
        );
    }
}
