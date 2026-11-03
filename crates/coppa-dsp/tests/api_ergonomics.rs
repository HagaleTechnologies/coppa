//! COP-90 (H-134): what a downstream crate (skimmer) needs from coppa-dsp's
//! public types.

use coppa_dsp::agc::AdaptiveAgc;
use coppa_dsp::carrier_recovery::CostasLoop;
use coppa_dsp::fft::FftProcessor;
use coppa_dsp::filter::RrcFilter;
use coppa_dsp::fir::{Fir, StreamingFir};
use coppa_dsp::timing_recovery::GardnerTimingRecovery;
use coppa_dsp::{Complex, Complex32};

fn assert_debug_clone<T: std::fmt::Debug + Clone>() {}

#[test]
fn every_public_struct_is_debug_and_clone() {
    assert_debug_clone::<AdaptiveAgc>();
    assert_debug_clone::<CostasLoop>();
    assert_debug_clone::<FftProcessor>();
    assert_debug_clone::<RrcFilter>();
    assert_debug_clone::<StreamingFir>();
    assert_debug_clone::<Fir>();
    assert_debug_clone::<GardnerTimingRecovery>();
}

#[test]
fn complex32_reexport_is_num_complex_0_4() {
    let fft = FftProcessor::new(4);
    let x: Vec<Complex32> = vec![Complex::new(1.0, 0.0); 4];
    let y: Vec<num_complex::Complex32> = fft.forward(&x);
    assert_eq!(y.len(), 4);
}

#[test]
fn fft_processor_debug_shows_its_size() {
    let s = format!("{:?}", FftProcessor::new(64));
    assert!(s.contains("size: 64"), "{s}");
}

#[test]
fn clone_of_fft_processor_gives_the_same_transform() {
    let a = FftProcessor::new(8);
    let b = a.clone();
    let x: Vec<Complex32> = (0..8).map(|i| Complex::new(i as f32, 0.0)).collect();
    assert_eq!(a.forward(&x), b.forward(&x));
}
