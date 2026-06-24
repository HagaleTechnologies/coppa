//! Benchmark suite for Coppa DSP and codec performance.

use coppa_codec::traits::{FecCodec, Modem};
use coppa_codec::BpskModem;
use coppa_dsp::agc::AdaptiveAgc;
use coppa_dsp::fft::FftProcessor;
use coppa_dsp::filter::RrcFilter;
use coppa_engine::CoppaCore;
use coppa_protocol::fec::convolutional::{ConvEncoder, ViterbiDecoder};
use criterion::{criterion_group, criterion_main, Criterion};
use num_complex::Complex32;
use std::hint::black_box;

fn bench_encode(c: &mut Criterion) {
    let core = CoppaCore::new();
    let message = "CQ CQ CQ DE VK2ABC VK2ABC K";

    c.bench_function("engine_encode", |b| {
        b.iter(|| {
            let _ = core.encode(black_box(message)).unwrap();
        });
    });
}

fn bench_decode(c: &mut Criterion) {
    let core = CoppaCore::new();
    let message = "CQ CQ CQ DE VK2ABC K";
    let samples = core.encode(message).unwrap();

    c.bench_function("engine_decode", |b| {
        b.iter(|| {
            let _ = core.decode(black_box(&samples)).unwrap();
        });
    });
}

fn bench_agc(c: &mut Criterion) {
    let mut agc = AdaptiveAgc::new(1.0, 64);
    let samples: Vec<f32> = (0..4096).map(|i| 0.1 * (i as f32 * 0.1).sin()).collect();

    c.bench_function("agc_4096_samples", |b| {
        b.iter(|| {
            agc.reset();
            let _ = agc.process(black_box(&samples));
        });
    });
}

fn bench_rrc_filter(c: &mut Criterion) {
    let filter = RrcFilter::new(0.35, 8, 6);
    let samples: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.1).sin()).collect();

    c.bench_function("rrc_filter_4096", |b| {
        b.iter(|| {
            let _ = filter.filter(black_box(&samples));
        });
    });
}

fn bench_fft(c: &mut Criterion) {
    let fft = FftProcessor::new(256);
    let samples: Vec<Complex32> = (0..256)
        .map(|i| Complex32::new((i as f32 * 0.1).sin(), 0.0))
        .collect();

    c.bench_function("fft_256", |b| {
        b.iter(|| {
            let _ = fft.forward(black_box(&samples));
        });
    });

    let fft_1024 = FftProcessor::new(1024);
    let samples_1024: Vec<Complex32> = (0..1024)
        .map(|i| Complex32::new((i as f32 * 0.1).sin(), 0.0))
        .collect();

    c.bench_function("fft_1024", |b| {
        b.iter(|| {
            let _ = fft_1024.forward(black_box(&samples_1024));
        });
    });
}

fn bench_conv_encode(c: &mut Criterion) {
    let mut encoder = ConvEncoder::new();
    let data: Vec<u8> = (0..128).map(|i| i & 1).collect();

    c.bench_function("conv_encode_128bits", |b| {
        b.iter(|| {
            let _ = encoder.encode(black_box(&data));
        });
    });
}

fn bench_viterbi_decode(c: &mut Criterion) {
    let mut encoder = ConvEncoder::new();
    let decoder = ViterbiDecoder::new();
    let data: Vec<u8> = (0..128).map(|i| i & 1).collect();
    let encoded = encoder.encode(&data);
    let soft: Vec<f32> = encoded
        .iter()
        .map(|&b| if b == 1 { 1.0 } else { -1.0 })
        .collect();

    c.bench_function("viterbi_decode_128bits", |b| {
        b.iter(|| {
            let _ = decoder.decode(black_box(&soft));
        });
    });
}

fn bench_bpsk_modulate(c: &mut Criterion) {
    let modem = BpskModem::new();
    let bits: Vec<u8> = (0..128).map(|i| i & 1).collect();

    c.bench_function("bpsk_modulate_128bits", |b| {
        b.iter(|| {
            let _ = modem.modulate(black_box(&bits)).unwrap();
        });
    });
}

fn bench_bpsk_demodulate(c: &mut Criterion) {
    let mut modem = BpskModem::new();
    let bits: Vec<u8> = (0..64).map(|i| i & 1).collect();
    let samples = modem.modulate(&bits).unwrap();

    c.bench_function("bpsk_demodulate_64bits", |b| {
        b.iter(|| {
            modem.reset();
            let _ = modem.demodulate_soft(black_box(&samples)).unwrap();
        });
    });
}

criterion_group!(
    benches,
    bench_encode,
    bench_decode,
    bench_agc,
    bench_rrc_filter,
    bench_fft,
    bench_conv_encode,
    bench_viterbi_decode,
    bench_bpsk_modulate,
    bench_bpsk_demodulate,
);
criterion_main!(benches);
