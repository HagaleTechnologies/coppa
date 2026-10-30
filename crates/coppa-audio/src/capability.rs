//! Capability model for the audio streams Coppa opens.
//!
//! Since COP-64 Coppa opens any `f32` mode a device offers: it uses one lane
//! (channel 0 unless a channel flag picks another) of a multi-channel stream
//! and resamples when the engine rate is not offered. `choose_format` is that
//! choice, shared by the live CPAL streams and `coppa doctor` (COP-59).

use anyhow::{anyhow, Result};

/// Input capture or output playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
}

impl Direction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
        }
    }
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

/// The stream format Coppa opens on a device: its channel count (one lane
/// of it is used) and hardware rate (resampled to the engine rate if they
/// differ).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenedFormat {
    pub channels: u16,
    pub sample_rate: u32,
}

/// Whether advertised ranges include a format Coppa can open, and which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    Yes(OpenedFormat),
    No(String),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FormatRange {
    pub(crate) channels: u16,
    pub(crate) min_rate: u32,
    pub(crate) max_rate: u32,
    pub(crate) f32_format: bool,
}

/// COP-64: pick the `(channels, hardware rate)` to open for `channel`:
/// an `f32` range with that lane, at `preferred_rate` if offered, else the
/// rate nearest `engine_rate`. `label` names the direction in errors.
pub(crate) fn choose_format(
    ranges: &[FormatRange],
    label: &str,
    engine_rate: u32,
    preferred_rate: u32,
    channel: u16,
) -> Result<(u16, u32)> {
    if engine_rate == 0 || preferred_rate == 0 {
        return Err(anyhow!("audio sample rate must be positive"));
    }
    let mut best: Option<(u8, u64, u16, u32)> = None;
    let mut max_channels = 0u16;
    for range in ranges {
        if !range.f32_format {
            continue;
        }
        max_channels = max_channels.max(range.channels);
        if channel >= range.channels || range.min_rate == 0 || range.min_rate > range.max_rate {
            continue;
        }
        let preferred_supported =
            range.min_rate <= preferred_rate && preferred_rate <= range.max_rate;
        let rate = if preferred_supported {
            preferred_rate
        } else {
            engine_rate.clamp(range.min_rate, range.max_rate)
        };
        let candidate = (
            if preferred_supported { 0 } else { 1 },
            rate.abs_diff(engine_rate) as u64,
            range.channels,
            rate,
        );
        if best.is_none_or(|b| candidate < b) {
            best = Some(candidate);
        }
    }
    best.map(|(_, _, channels, rate)| (channels, rate))
        .ok_or_else(|| {
            if channel >= max_channels && max_channels > 0 {
                anyhow!(
                    "{} channel {} is unavailable; available channels: 0..{}",
                    label,
                    channel,
                    max_channels - 1
                )
            } else {
                anyhow!("no usable f32 {} audio format", label)
            }
        })
}

/// Whether Coppa can open one of `ranges` for `direction` (lane 0) with
/// `sample_rate` as the engine rate: the same `choose_format` the live
/// streams use. A device with no `f32` mode is explained by its formats.
pub fn coppa_stream_support(
    ranges: &[ConfigRange],
    direction: Direction,
    sample_rate: u32,
) -> Support {
    if ranges.is_empty() {
        return Support::No("advertises no configurations".into());
    }
    if !ranges.iter().any(|r| r.is_f32) {
        let mut formats = Vec::<&str>::new();
        for r in ranges {
            if !formats.contains(&r.sample_format.as_str()) {
                formats.push(&r.sample_format);
            }
        }
        let listed = if formats.len() > 4 {
            format!("{}, …", formats[..4].join(", "))
        } else {
            formats.join(", ")
        };
        return Support::No(format!("no f32 mode; formats: {listed}"));
    }
    let formats: Vec<_> = ranges
        .iter()
        .map(|r| FormatRange {
            channels: r.channels,
            min_rate: r.min_sample_rate,
            max_rate: r.max_sample_rate,
            f32_format: r.is_f32,
        })
        .collect();
    match choose_format(&formats, direction.label(), sample_rate, sample_rate, 0) {
        Ok((channels, sample_rate)) => Support::Yes(OpenedFormat {
            channels,
            sample_rate,
        }),
        Err(error) => Support::No(error.to_string()),
    }
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
    /// COP-64's resolver found several devices matching the name.
    Ambiguous {
        error: String,
    },
    Opened {
        device: String,
        format: OpenedFormat,
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

    fn yes(channels: u16, sample_rate: u32) -> Support {
        Support::Yes(OpenedFormat {
            channels,
            sample_rate,
        })
    }

    #[test]
    fn a_mono_f32_range_containing_the_rate_is_supported() {
        assert_eq!(
            coppa_stream_support(&[range(1, 8_000, 96_000, "f32")], Direction::Input, 48_000),
            yes(1, 48_000)
        );
    }

    #[test]
    fn rate_bounds_are_inclusive() {
        assert_eq!(
            coppa_stream_support(&[range(1, 48_000, 48_000, "f32")], Direction::Input, 48_000),
            yes(1, 48_000)
        );
    }

    #[test]
    fn mono_at_the_rate_is_preferred_among_many() {
        let ranges = [
            range(2, 8_000, 96_000, "f32"),
            range(1, 48_000, 48_000, "i16"),
            range(1, 44_100, 48_000, "f32"),
        ];
        assert_eq!(
            coppa_stream_support(&ranges, Direction::Output, 48_000),
            yes(1, 48_000)
        );
    }

    #[test]
    fn no_configurations_is_explained() {
        assert_eq!(
            coppa_stream_support(&[], Direction::Input, 48_000),
            Support::No("advertises no configurations".into())
        );
    }

    /// COP-64 x COP-59: a stereo-only f32 device is usable (Coppa takes
    /// lane 0), so doctor must not fail it as it did when streams were mono.
    #[test]
    fn a_stereo_only_f32_device_is_usable_on_lane_zero() {
        let ranges = [
            range(2, 44_100, 48_000, "i16"),
            range(2, 44_100, 48_000, "f32"),
        ];
        assert_eq!(
            coppa_stream_support(&ranges, Direction::Input, 48_000),
            yes(2, 48_000)
        );
    }

    /// Without the engine rate Coppa opens the nearest rate and resamples.
    #[test]
    fn without_the_rate_the_nearest_rate_is_chosen() {
        let ranges = [
            range(1, 44_100, 44_100, "f32"),
            range(1, 96_000, 96_000, "f32"),
        ];
        assert_eq!(
            coppa_stream_support(&ranges, Direction::Input, 48_000),
            yes(1, 44_100)
        );
    }

    #[test]
    fn a_device_without_f32_is_explained_by_its_formats() {
        let ranges = [
            range(1, 48_000, 48_000, "i16"),
            range(1, 8_000, 96_000, "i32"),
        ];
        assert_eq!(
            coppa_stream_support(&ranges, Direction::Output, 48_000),
            Support::No("no f32 mode; formats: i16, i32".into())
        );
    }

    /// The doctor support verdict and the live streams agree, because both
    /// are `choose_format` with lane 0.
    #[test]
    fn support_matches_the_live_format_choice() {
        let ranges = [
            range(2, 44_100, 44_100, "f32"),
            range(4, 8_000, 192_000, "f32"),
        ];
        let formats: Vec<_> = ranges
            .iter()
            .map(|r| FormatRange {
                channels: r.channels,
                min_rate: r.min_sample_rate,
                max_rate: r.max_sample_rate,
                f32_format: r.is_f32,
            })
            .collect();
        let (channels, sample_rate) = choose_format(&formats, "input", 48_000, 48_000, 0).unwrap();
        assert_eq!(
            coppa_stream_support(&ranges, Direction::Input, 48_000),
            yes(channels, sample_rate)
        );
    }
}
