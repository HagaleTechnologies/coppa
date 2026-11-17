//! COP-170: the comparison grid. The channel names and SNR points are the
//! axes of the 2020 Winlink comparison (WGN, MPG, MPP at 30 to 0 dB); the two
//! transfer sizes are this harness's fixed 2 KB and 16 KB.

use coppa_channel::watterson::WattersonPreset;

/// SNR points in dB (3 kHz reference), highest first.
pub const SNRS_DB: [i32; 7] = [30, 25, 20, 15, 10, 5, 0];

/// Transfer sizes in bytes.
pub const SIZES: [usize; 2] = [2048, 16384];

/// Seed of trial 0 of cell 0 in the published plan.
pub const SEED_BASE: u64 = 0x1700_0000;

/// Seed distance between two cells; caps a cell at 65,536 trials.
pub const CELL_SEED_STRIDE: u64 = 0x1_0000;

/// The three channels of the 2020 study.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Channel {
    /// White Gaussian noise, no fading.
    Wgn,
    /// Watterson Good (CCIR/ITU-R F.1487 mid-latitude quiet): 0.5 ms, 0.1 Hz.
    Mpg,
    /// Watterson Poor (mid-latitude disturbed): 2 ms, 1 Hz.
    Mpp,
}

impl Channel {
    /// All channels in grid order.
    pub const ALL: [Channel; 3] = [Channel::Wgn, Channel::Mpg, Channel::Mpp];

    /// Lower-case CLI and CSV name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Channel::Wgn => "wgn",
            Channel::Mpg => "mpg",
            Channel::Mpp => "mpp",
        }
    }

    /// Parse a CLI or CSV name.
    #[must_use]
    pub fn parse(s: &str) -> Option<Channel> {
        Channel::ALL
            .into_iter()
            .find(|c| c.name().eq_ignore_ascii_case(s.trim()))
    }

    /// The Watterson preset, or `None` for WGN.
    #[must_use]
    pub fn preset(self) -> Option<WattersonPreset> {
        match self {
            Channel::Wgn => None,
            Channel::Mpg => Some(WattersonPreset::Good),
            Channel::Mpp => Some(WattersonPreset::Poor),
        }
    }

    /// Position in [`Channel::ALL`].
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Channel::Wgn => 0,
            Channel::Mpg => 1,
            Channel::Mpp => 2,
        }
    }

    /// Heading used in the rendered tables.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Channel::Wgn => "WGN (no fading)",
            Channel::Mpg => "MPG (Watterson Good: 0.5 ms, 0.1 Hz)",
            Channel::Mpp => "MPP (Watterson Poor: 2 ms, 1 Hz)",
        }
    }
}

/// One point of the grid: a channel, an SNR and a transfer size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cell {
    /// Channel.
    pub channel: Channel,
    /// SNR in dB; must be one of [`SNRS_DB`].
    pub snr_db: i32,
    /// Transfer size in bytes; must be one of [`SIZES`].
    pub size_bytes: usize,
}

impl Cell {
    /// The cell index `channel * 14 + snr_index * 2 + size_index`, or `None`
    /// when the SNR or size is off the grid.
    #[must_use]
    pub fn index(&self) -> Option<usize> {
        let snr = SNRS_DB.iter().position(|&s| s == self.snr_db)?;
        let size = SIZES.iter().position(|&s| s == self.size_bytes)?;
        Some(self.channel.index() * SNRS_DB.len() * SIZES.len() + snr * SIZES.len() + size)
    }

    /// Seed of `trial` in this cell for a plan starting at `seed_base`.
    /// Prefix-stable: more trials extend the recorded ones.
    #[must_use]
    pub fn seed(&self, seed_base: u64, trial: usize) -> Option<u64> {
        if trial as u64 >= CELL_SEED_STRIDE {
            return None;
        }
        let cell = self.index()? as u64;
        Some(
            seed_base
                .wrapping_add(cell * CELL_SEED_STRIDE)
                .wrapping_add(trial as u64),
        )
    }
}

/// Every cell of the grid restricted to the given axes, in grid order
/// (channel, SNR from high to low, size from small to large).
#[must_use]
pub fn cells(channels: &[Channel], snrs: &[i32], sizes: &[usize]) -> Vec<Cell> {
    let mut out = Vec::new();
    for channel in Channel::ALL {
        if !channels.contains(&channel) {
            continue;
        }
        for snr_db in SNRS_DB {
            if !snrs.contains(&snr_db) {
                continue;
            }
            for size_bytes in SIZES {
                if sizes.contains(&size_bytes) {
                    out.push(Cell {
                        channel,
                        snr_db,
                        size_bytes,
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn grid_is_the_2020_study_axes() {
        assert_eq!(SNRS_DB, [30, 25, 20, 15, 10, 5, 0]);
        assert_eq!(SIZES, [2048, 16384]);
        assert!(Channel::Wgn.preset().is_none());
        assert!(matches!(Channel::Mpg.preset(), Some(WattersonPreset::Good)));
        assert!(matches!(Channel::Mpp.preset(), Some(WattersonPreset::Poor)));
        assert_eq!(cells(&Channel::ALL, &SNRS_DB, &SIZES).len(), 42);
        assert_eq!(Channel::parse("MPG"), Some(Channel::Mpg));
        assert_eq!(Channel::parse("moderate"), None);
    }

    #[test]
    fn cell_index_and_seed_are_stable() {
        let first = Cell {
            channel: Channel::Wgn,
            snr_db: 30,
            size_bytes: 2048,
        };
        let mpg20 = Cell {
            channel: Channel::Mpg,
            snr_db: 20,
            size_bytes: 2048,
        };
        let last = Cell {
            channel: Channel::Mpp,
            snr_db: 0,
            size_bytes: 16384,
        };
        assert_eq!(first.seed(SEED_BASE, 0), Some(0x1700_0000));
        assert_eq!(mpg20.index(), Some(18));
        assert_eq!(mpg20.seed(SEED_BASE, 0), Some(0x1712_0000));
        assert_eq!(last.index(), Some(41));
        assert_eq!(last.seed(SEED_BASE, 0), Some(0x1729_0000));
        let off_grid = Cell {
            channel: Channel::Wgn,
            snr_db: 12,
            size_bytes: 2048,
        };
        assert_eq!(off_grid.index(), None);

        let all = cells(&Channel::ALL, &SNRS_DB, &SIZES);
        let seeds: HashSet<u64> = all
            .iter()
            .flat_map(|c| (0..8).map(move |t| c.seed(SEED_BASE, t).unwrap()))
            .collect();
        assert_eq!(seeds.len(), 42 * 8);
        // Grid order is index order.
        for (i, c) in all.iter().enumerate() {
            assert_eq!(c.index(), Some(i));
        }
    }
}
