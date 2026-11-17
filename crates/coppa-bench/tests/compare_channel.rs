use coppa_bench::compare::channel_check::measure;

#[test]
fn all_presets_within_tolerance() {
    for index in 0..3 {
        // Use the production audio path at a smaller sample rate. The longer
        // Good observation covers several of its 7.5 s coherence intervals.
        let doppler_duration = if index == 0 { 600 } else { 120 };
        let result = measure(index, 8000, 600, doppler_duration);
        assert!(result.within_tolerance, "{result:?}");
    }
}
