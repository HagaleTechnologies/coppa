//! Capability model for Coppa's mono f32 audio streams.

/// Channels in every CPAL stream Coppa opens. Samples are `f32`.
pub const COPPA_STREAM_CHANNELS: u16 = 1;

/// Input capture or output playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
}

/// One configuration range a device advertises for one direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigRange {
    pub channels: u16,
    pub min_sample_rate: u32,
    pub max_sample_rate: u32,
    /// The backend's display name for the sample format, such as `f32`.
    pub sample_format: String,
    pub is_f32: bool,
}

/// Whether advertised ranges include Coppa's stream at a given rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    Yes,
    No(String),
}

/// Check for a mono f32 range containing `sample_rate`, explaining the first mismatch.
pub fn coppa_stream_support(ranges: &[ConfigRange], sample_rate: u32) -> Support {
    if ranges.is_empty() {
        return Support::No("advertises no configurations".into());
    }
    let mut channels: Vec<_> = ranges.iter().map(|r| r.channels).collect();
    channels.sort_unstable();
    channels.dedup();
    if !channels.contains(&COPPA_STREAM_CHANNELS) {
        let listed = if channels.len() > 4 {
            format!("{}-{}", channels[0], channels[channels.len() - 1])
        } else {
            channels
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Support::No(format!("no 1-channel mode; offers {listed} ch"));
    }
    let mut mono_rates: Vec<_> = ranges
        .iter()
        .filter(|r| r.channels == COPPA_STREAM_CHANNELS)
        .map(|r| (r.min_sample_rate, r.max_sample_rate))
        .collect();
    mono_rates.sort_unstable();
    mono_rates.dedup();
    let at_rate = |r: &ConfigRange| {
        r.channels == COPPA_STREAM_CHANNELS
            && r.min_sample_rate <= sample_rate
            && sample_rate <= r.max_sample_rate
    };
    if !ranges.iter().any(at_rate) {
        let listed = if mono_rates.len() > 4 {
            format!(
                "{}-{} Hz",
                mono_rates[0].0,
                mono_rates
                    .iter()
                    .map(|(_, max)| max)
                    .max()
                    .expect("nonempty mono ranges")
            )
        } else {
            mono_rates
                .iter()
                .map(|(min, max)| {
                    if min == max {
                        format!("{min} Hz")
                    } else {
                        format!("{min}-{max} Hz")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Support::No(format!(
            "no mono mode at {sample_rate} Hz; mono rates: {listed}"
        ));
    }
    if ranges.iter().filter(|r| at_rate(r)).any(|r| r.is_f32) {
        return Support::Yes;
    }
    let mut formats = Vec::<&str>::new();
    for r in ranges.iter().filter(|r| at_rate(r)) {
        if !formats.contains(&r.sample_format.as_str()) {
            formats.push(&r.sample_format);
        }
    }
    let listed = if formats.len() > 4 {
        format!("{}, …", formats[..4].join(", "))
    } else {
        formats.join(", ")
    };
    Support::No(format!(
        "no f32 mono mode at {sample_rate} Hz; formats: {listed}"
    ))
}

/// One enumerated device and its support in each direction.
/// `None` means no configurations were advertised or the query failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSupport {
    pub name: String,
    pub id: String,
    pub input: Option<Support>,
    pub output: Option<Support>,
}

/// Result of resolving and test-opening the device Coppa would use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceProbe {
    NoBackend,
    NoDefault,
    NoMatch {
        available: Vec<String>,
    },
    Opened {
        device: String,
    },
    OpenFailed {
        device: String,
        error: String,
        advertised: Option<Support>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(ch: u16, min: u32, max: u32, fmt: &str) -> ConfigRange {
        ConfigRange {
            channels: ch,
            min_sample_rate: min,
            max_sample_rate: max,
            sample_format: fmt.to_string(),
            is_f32: fmt == "f32",
        }
    }

    #[test]
    fn a_mono_f32_range_containing_the_rate_is_supported() {
        assert_eq!(
            coppa_stream_support(&[range(1, 8_000, 96_000, "f32")], 48_000),
            Support::Yes
        );
    }

    #[test]
    fn rate_bounds_are_inclusive() {
        assert_eq!(
            coppa_stream_support(&[range(1, 48_000, 48_000, "f32")], 48_000),
            Support::Yes
        );
    }

    #[test]
    fn one_supporting_range_among_many_is_enough() {
        let ranges = [
            range(2, 8_000, 96_000, "f32"),
            range(1, 48_000, 48_000, "i16"),
            range(1, 44_100, 48_000, "f32"),
        ];
        assert_eq!(coppa_stream_support(&ranges, 48_000), Support::Yes);
    }

    #[test]
    fn no_configurations_is_explained() {
        assert_eq!(
            coppa_stream_support(&[], 48_000),
            Support::No("advertises no configurations".into())
        );
    }

    #[test]
    fn a_stereo_only_device_is_explained_by_its_channel_counts() {
        let ranges = [
            range(2, 44_100, 48_000, "i16"),
            range(2, 44_100, 48_000, "f32"),
        ];
        assert_eq!(
            coppa_stream_support(&ranges, 48_000),
            Support::No("no 1-channel mode; offers 2 ch".into())
        );
    }

    #[test]
    fn many_channel_counts_are_summarised_as_a_span() {
        let ranges: Vec<_> = (2..=64)
            .map(|ch| range(ch, 44_100, 48_000, "f32"))
            .collect();
        assert_eq!(
            coppa_stream_support(&ranges, 48_000),
            Support::No("no 1-channel mode; offers 2-64 ch".into())
        );
    }

    #[test]
    fn mono_without_the_rate_is_explained_by_its_mono_rates() {
        let ranges = [
            range(1, 44_100, 44_100, "f32"),
            range(1, 96_000, 96_000, "f32"),
        ];
        assert_eq!(
            coppa_stream_support(&ranges, 48_000),
            Support::No("no mono mode at 48000 Hz; mono rates: 44100 Hz, 96000 Hz".into())
        );
    }

    #[test]
    fn mono_at_the_rate_without_f32_is_explained_by_its_formats() {
        let ranges = [
            range(1, 48_000, 48_000, "i16"),
            range(1, 8_000, 96_000, "i32"),
        ];
        assert_eq!(
            coppa_stream_support(&ranges, 48_000),
            Support::No("no f32 mono mode at 48000 Hz; formats: i16, i32".into())
        );
    }

    #[test]
    fn coppa_streams_are_mono() {
        assert_eq!(COPPA_STREAM_CHANNELS, 1);
    }
}
