//! COP-24: every speed level an HF engine ships must decode through SSB.

use coppa_channel::{awgn_seeded, ssb_filter};
use coppa_engine::{CoppaCore, HF_STANDARD};
use rand::{rngs::StdRng, Rng, SeedableRng};

#[test]
fn every_shipped_hf_level_decodes_through_ssb() {
    let levels = CoppaCore::from_profile(&HF_STANDARD).available_speed_levels();
    let results = std::thread::scope(|scope| {
        let threads: Vec<_> = levels
            .iter()
            .map(|&level| {
                scope.spawn(move || {
                    let mut tx = CoppaCore::from_profile(&HF_STANDARD);
                    tx.set_speed_level(level).unwrap();
                    let mut failures = 0;
                    for trial in 0..10_u64 {
                        let seed = 24_100 + u64::from(level) * 100 + trial;
                        let mut rng = StdRng::seed_from_u64(seed);
                        let mut payload = [0_u8; 24];
                        rng.fill_bytes(&mut payload);
                        let samples = tx.encode_bytes(&payload).unwrap();
                        let filtered = ssb_filter(&samples, 48_000.0);
                        let received = awgn_seeded(&filtered, 30.0, seed);
                        let rx = CoppaCore::from_profile(&HF_STANDARD);
                        match rx.decode_bytes(&received) {
                            Ok(decoded) if decoded == payload => {}
                            result => {
                                eprintln!("HF level {level}, trial {trial}: {result:?}");
                                failures += 1;
                            }
                        }
                    }
                    (level, failures)
                })
            })
            .collect();
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    for &(level, failures) in &results {
        eprintln!("HF level {level}: {failures}/10 frame errors at 30 dB through SSB");
    }
    for (level, failures) in results {
        assert!(
            failures <= 1,
            "HF level {level}: FER {failures}/10 exceeds 10%"
        );
    }
    assert_eq!(levels, (1..=8).collect::<Vec<_>>());
}
