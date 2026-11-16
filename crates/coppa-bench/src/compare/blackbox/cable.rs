//! Per-trial ALSA file PCMs connected by four named pipes.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Cable {
    pub dir: PathBuf,
}

impl Cable {
    pub fn create(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        for name in ["ab_in", "ab_out", "ba_in", "ba_out"] {
            let path = dir.join(name);
            let status = Command::new("mkfifo")
                .arg(&path)
                .status()
                .with_context(|| format!("creating {}", path.display()))?;
            anyhow::ensure!(status.success(), "mkfifo failed for {}", path.display());
        }
        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    pub fn station_home(&self, station: char) -> Result<PathBuf> {
        let home = self.dir.join(format!("home-{station}"));
        std::fs::create_dir_all(&home)?;
        let config = format!(
            "pcm.coppa_cmp_a_play {{ type file; slave.pcm \"null\"; file \"{0}/ab_in\"; format \"raw\" }}\n\
             pcm.coppa_cmp_a_cap {{ type file; slave.pcm \"null\"; file \"/dev/null\"; infile \"{0}/ba_out\"; format \"raw\" }}\n\
             pcm.coppa_cmp_b_play {{ type file; slave.pcm \"null\"; file \"{0}/ba_in\"; format \"raw\" }}\n\
             pcm.coppa_cmp_b_cap {{ type file; slave.pcm \"null\"; file \"/dev/null\"; infile \"{0}/ab_out\"; format \"raw\" }}\n\
             pcm.coppa_cmp_a {{ type asym; playback.pcm \"coppa_cmp_a_play\"; capture.pcm \"coppa_cmp_a_cap\"; hint {{ show on; description \"coppa-cmp-A\" }} }}\n\
             pcm.coppa_cmp_b {{ type asym; playback.pcm \"coppa_cmp_b_play\"; capture.pcm \"coppa_cmp_b_cap\"; hint {{ show on; description \"coppa-cmp-B\" }} }}\n", self.dir.display());
        std::fs::write(home.join(".asoundrc"), config)?;
        Ok(home)
    }
}
