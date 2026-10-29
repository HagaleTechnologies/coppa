//! Presence-aware settings for the CLI's four modem commands.
use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use coppa_daemon::config::{config_candidates, ConfigEnv, DaemonConfig, PttConfig, SharedSettings};

use crate::RigArgs;

const DEFAULT_PTT: &str = "none";
const DEFAULT_RIGCTLD: &str = "127.0.0.1:4532";
const DEFAULT_PTT_LEAD_MS: u64 = 50;
const DEFAULT_PTT_TAIL_MS: u64 = 200;

pub const ENV_VARS: [&str; 10] = [
    "COPPA_CONFIG",
    "COPPA_CALLSIGN",
    "COPPA_PROFILE",
    "COPPA_DEVICE",
    "COPPA_PTT",
    "COPPA_RIGCTLD",
    "COPPA_PTT_LEAD_MS",
    "COPPA_PTT_TAIL_MS",
    "COPPA_FREQUENCY_HZ",
    "COPPA_BAND",
];

pub const SETTINGS_HELP: &str =
    "Settings: a flag beats its COPPA_* variable, which beats coppad.toml, which beats the default.
The first config file found is ./coppad.toml, then $XDG_CONFIG_HOME/coppa/ or ~/.config/coppa/,
then /etc/coppa/ (Windows: .\\coppad.toml, then %APPDATA%\\coppa\\).
  --callsign      COPPA_CALLSIGN      [engine] callsign
  --profile       COPPA_PROFILE       [engine] profile
  --device        COPPA_DEVICE        [audio] output_device (tx, tune) / input_device (rx)
  --ptt           COPPA_PTT           [radio] ptt_method (none, vox or rigctld)
  --rigctld       COPPA_RIGCTLD       [radio] rigctld_address
  --ptt-lead-ms   COPPA_PTT_LEAD_MS   [radio] ptt_pre_delay_ms
  --ptt-tail-ms   COPPA_PTT_TAIL_MS   [radio] ptt_tail_delay_ms
  --frequency-hz  COPPA_FREQUENCY_HZ  [radio] frequency_hz
  --band          COPPA_BAND          [radio] band
  --config        COPPA_CONFIG        (the file itself)
--verbose shows each value and where it came from.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Flag(&'static str),
    Env(&'static str),
    File { key: &'static str, path: PathBuf },
    Default,
}
impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Flag(name) => write!(f, "--{name}"),
            Self::Env(name) => write!(f, "{name}"),
            Self::File { key, path } => write!(f, "{key} in {}", path.display()),
            Self::Default => write!(f, "default"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Setting<T> {
    pub value: T,
    pub source: Source,
}
impl<T> Setting<T> {
    fn new(value: T, source: Source) -> Self {
        Self { value, source }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CliEnv(HashMap<&'static str, OsString>);
impl CliEnv {
    pub fn from_process() -> Self {
        Self(
            ENV_VARS
                .into_iter()
                .filter_map(|name| std::env::var_os(name).map(|v| (name, v)))
                .collect(),
        )
    }
    #[cfg(test)]
    pub fn from_pairs(pairs: impl IntoIterator<Item = (&'static str, OsString)>) -> Self {
        Self(pairs.into_iter().collect())
    }
    fn get(&self, name: &'static str) -> Result<Option<String>> {
        match self.0.get(name) {
            None => Ok(None),
            Some(value) if value.is_empty() => Ok(None),
            Some(value) => value
                .clone()
                .into_string()
                .map(Some)
                .map_err(|_| anyhow::anyhow!("{name} is not valid UTF-8")),
        }
    }
    fn number(&self, name: &'static str) -> Result<Option<u64>> {
        self.get(name)?
            .map(|value| {
                value.parse::<u64>().map_err(|_| {
                    anyhow::anyhow!("{name}={value:?} is not a whole number of milliseconds")
                })
            })
            .transpose()
    }
    fn frequency(&self) -> Result<Option<u64>> {
        self.get("COPPA_FREQUENCY_HZ")?
            .map(|value| {
                value.parse::<u64>().map_err(|_| {
                    anyhow::anyhow!("COPPA_FREQUENCY_HZ={value:?} is not a whole number of hertz")
                })
            })
            .transpose()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Input,
    Output,
}

#[derive(Clone, Copy)]
enum FileSelection {
    Flag,
    Env,
    Search,
}

pub struct Layers {
    env: CliEnv,
    selection: FileSelection,
    pub shared: SharedSettings,
    pub path: Option<PathBuf>,
    searched: String,
}
impl Layers {
    pub fn load(config_flag: Option<&Path>, env: CliEnv) -> Result<Self> {
        let (explicit, selection) = match config_flag {
            Some(path) => (Some(path.to_path_buf()), FileSelection::Flag),
            None => match env.get("COPPA_CONFIG")? {
                Some(path) => (Some(PathBuf::from(path)), FileSelection::Env),
                None => (None, FileSelection::Search),
            },
        };
        if let Some(path) = explicit {
            let (_, shared) = DaemonConfig::load_with_shared(&path)
                .with_context(|| format!("Failed to load config {}", path.display()))?;
            return Ok(Self {
                env,
                selection,
                shared,
                path: Some(path),
                searched: String::new(),
            });
        }
        let found = DaemonConfig::discover(&config_candidates(&ConfigEnv::from_process()))?;
        let searched = found.search_summary();
        Ok(Self {
            env,
            selection,
            shared: found.shared,
            path: found.source,
            searched,
        })
    }
    #[cfg(test)]
    fn for_test(env: CliEnv, shared: SharedSettings) -> Self {
        Self {
            env,
            selection: FileSelection::Flag,
            shared,
            path: Some(PathBuf::from("station.toml")),
            searched: String::new(),
        }
    }
    fn file_source(&self, key: &'static str) -> Source {
        Source::File {
            key,
            path: self.path.clone().expect("file key has a source"),
        }
    }
    fn string(
        &self,
        flag: Option<&str>,
        flag_name: &'static str,
        env_name: &'static str,
        file: Option<&str>,
        file_key: &'static str,
    ) -> Result<Option<Setting<String>>> {
        if let Some(value) = flag {
            return Ok(Some(Setting::new(value.into(), Source::Flag(flag_name))));
        }
        if let Some(value) = self.env.get(env_name)? {
            return Ok(Some(Setting::new(value, Source::Env(env_name))));
        }
        Ok(file.map(|value| Setting::new(value.into(), self.file_source(file_key))))
    }
    fn number(
        &self,
        flag: Option<u64>,
        flag_name: &'static str,
        env_name: &'static str,
        file: Option<u64>,
        file_key: &'static str,
        default: u64,
    ) -> Result<Setting<u64>> {
        if let Some(value) = flag {
            return Ok(Setting::new(value, Source::Flag(flag_name)));
        }
        if let Some(value) = self.env.number(env_name)? {
            return Ok(Setting::new(value, Source::Env(env_name)));
        }
        if let Some(value) = file {
            return Ok(Setting::new(value, self.file_source(file_key)));
        }
        Ok(Setting::new(default, Source::Default))
    }
    pub fn profile(&self, flag: Option<&str>) -> Result<Option<Setting<String>>> {
        let result = self.string(
            flag,
            "profile",
            "COPPA_PROFILE",
            self.shared.profile.as_deref(),
            "[engine] profile",
        )?;
        if let Some(ref setting) = result {
            if coppa_engine::profiles::get_profile(&setting.value).is_none() {
                let suffix = if matches!(setting.source, Source::Flag(_)) {
                    String::new()
                } else {
                    format!(" (from {})", setting.source)
                };
                anyhow::bail!(
                    "Unknown profile: {}{}. Available: {}",
                    setting.value,
                    suffix,
                    coppa_engine::profiles::list_profiles().join(", ")
                );
            }
        }
        Ok(result)
    }
    pub fn callsign(&self, flag: Option<&str>) -> Result<Option<Setting<String>>> {
        self.string(
            flag,
            "callsign",
            "COPPA_CALLSIGN",
            self.shared.callsign.as_deref(),
            "[engine] callsign",
        )
    }
    pub fn device(
        &self,
        flag: Option<&str>,
        direction: Direction,
    ) -> Result<Option<Setting<String>>> {
        let (file, key) = match direction {
            Direction::Input => (self.shared.input_device.as_deref(), "[audio] input_device"),
            Direction::Output => (
                self.shared.output_device.as_deref(),
                "[audio] output_device",
            ),
        };
        self.string(flag, "device", "COPPA_DEVICE", file, key)
    }
    pub fn rig(&self, flags: &RigArgs) -> Result<RigSettings> {
        let device = self.device(flags.device.as_deref(), Direction::Output)?;
        let ptt = self
            .string(
                flags.ptt.as_deref(),
                "ptt",
                "COPPA_PTT",
                self.shared.ptt_method.as_deref(),
                "[radio] ptt_method",
            )?
            .unwrap_or_else(|| Setting::new(DEFAULT_PTT.into(), Source::Default));
        let rigctld = self
            .string(
                flags.rigctld.as_deref(),
                "rigctld",
                "COPPA_RIGCTLD",
                self.shared.rigctld_address.as_deref(),
                "[radio] rigctld_address",
            )?
            .unwrap_or_else(|| Setting::new(DEFAULT_RIGCTLD.into(), Source::Default));
        let lead = self.number(
            flags.ptt_lead_ms,
            "ptt-lead-ms",
            "COPPA_PTT_LEAD_MS",
            self.shared.ptt_pre_delay_ms,
            "[radio] ptt_pre_delay_ms",
            DEFAULT_PTT_LEAD_MS,
        )?;
        let tail = self.number(
            flags.ptt_tail_ms,
            "ptt-tail-ms",
            "COPPA_PTT_TAIL_MS",
            self.shared.ptt_tail_delay_ms,
            "[radio] ptt_tail_delay_ms",
            DEFAULT_PTT_TAIL_MS,
        )?;
        let (frequency_hz, band) = if flags.frequency_hz.is_some() || flags.band.is_some() {
            (
                flags
                    .frequency_hz
                    .map(|v| Setting::new(v, Source::Flag("frequency-hz"))),
                flags
                    .band
                    .as_ref()
                    .map(|v| Setting::new(v.clone(), Source::Flag("band"))),
            )
        } else {
            let env_freq = self.env.frequency()?;
            let env_band = self.env.get("COPPA_BAND")?;
            if env_freq.is_some() || env_band.is_some() {
                (
                    env_freq.map(|v| Setting::new(v, Source::Env("COPPA_FREQUENCY_HZ"))),
                    env_band.map(|v| Setting::new(v, Source::Env("COPPA_BAND"))),
                )
            } else {
                (
                    self.shared
                        .frequency_hz
                        .map(|v| Setting::new(v, self.file_source("[radio] frequency_hz"))),
                    self.shared
                        .band
                        .as_ref()
                        .map(|v| Setting::new(v.clone(), self.file_source("[radio] band"))),
                )
            }
        };
        Ok(RigSettings {
            device,
            ptt,
            rigctld,
            lead,
            tail,
            frequency_hz,
            band,
        })
    }
    pub fn print_verbose_source(&self) {
        match (self.selection, &self.path) {
            (FileSelection::Flag | FileSelection::Env, Some(path))
            | (FileSelection::Search, Some(path)) => {
                eprintln!("[verbose] Configuration file: {}", path.display())
            }
            (FileSelection::Search, None) => eprintln!(
                "[verbose] Configuration file: none found (searched: {})",
                self.searched
            ),
            (FileSelection::Flag | FileSelection::Env, None) => {
                unreachable!("explicit path was loaded")
            }
        }
    }
}

pub struct RigSettings {
    pub device: Option<Setting<String>>,
    pub ptt: Setting<String>,
    pub rigctld: Setting<String>,
    pub lead: Setting<u64>,
    pub tail: Setting<u64>,
    pub frequency_hz: Option<Setting<u64>>,
    pub band: Option<Setting<String>>,
}
impl RigSettings {
    pub fn ptt_method(&self) -> Result<&'static str> {
        if matches!(self.ptt.source, Source::Flag(_)) {
            crate::validate_ptt_method(&self.ptt.value)?;
            return Ok(match self.ptt.value.as_str() {
                "none" => "none",
                "vox" => "vox",
                _ => "rigctld",
            });
        }
        match PttConfig::parse(&self.ptt.value) {
            Ok(PttConfig::None) => Ok("none"),
            Ok(PttConfig::Vox) => Ok("vox"),
            Ok(PttConfig::Rigctld) => Ok("rigctld"),
            Ok(PttConfig::Serial { .. } | PttConfig::Gpio { .. }) => anyhow::bail!(
                "PTT method {:?} from {} is coppad-only: coppa tx and coppa tune support \"none\", \"vox\" or \"rigctld\"; pass --ptt to override",
                self.ptt.value, self.ptt.source),
            Err(error) => anyhow::bail!("PTT method {:?} from {}: {error}", self.ptt.value, self.ptt.source),
        }
    }
    pub fn frequency_evidence(&self) -> Result<Option<coppa_radio::band::FrequencyEvidence>> {
        let freq_key = self
            .frequency_hz
            .as_ref()
            .map_or("--frequency-hz".to_owned(), |v| v.source.to_string());
        let band_key = self
            .band
            .as_ref()
            .map_or("--band".to_owned(), |v| v.source.to_string());
        crate::cli_frequency_evidence(
            self.frequency_hz.as_ref().map(|v| v.value),
            self.band.as_ref().map(|v| v.value.as_str()),
            &freq_key,
            &band_key,
        )
    }
    pub fn print_verbose(&self) {
        if let Some(device) = &self.device {
            eprintln!("[verbose] Device: {} ({})", device.value, device.source);
        }
        eprintln!("[verbose] PTT: {} ({})", self.ptt.value, self.ptt.source);
        eprintln!(
            "[verbose] rigctld: {} ({})",
            self.rigctld.value, self.rigctld.source
        );
        eprintln!(
            "[verbose] PTT lead: {} ms ({})",
            self.lead.value, self.lead.source
        );
        eprintln!(
            "[verbose] PTT tail: {} ms ({})",
            self.tail.value, self.tail.source
        );
        if let Some(freq) = &self.frequency_hz {
            eprintln!("[verbose] Frequency: {} Hz ({})", freq.value, freq.source);
        }
        if let Some(band) = &self.band {
            eprintln!("[verbose] Band: {} ({})", band.value, band.source);
        }
    }
    #[cfg(test)]
    pub fn test_values(
        device: Option<&str>,
        ptt: &str,
        rigctld: &str,
        lead: u64,
        tail: u64,
        frequency_hz: Option<u64>,
        band: Option<&str>,
    ) -> Self {
        Self {
            device: device.map(|v| Setting::new(v.into(), Source::Flag("device"))),
            ptt: Setting::new(ptt.into(), Source::Flag("ptt")),
            rigctld: Setting::new(rigctld.into(), Source::Flag("rigctld")),
            lead: Setting::new(lead, Source::Flag("ptt-lead-ms")),
            tail: Setting::new(tail, Source::Flag("ptt-tail-ms")),
            frequency_hz: frequency_hz.map(|v| Setting::new(v, Source::Flag("frequency-hz"))),
            band: band.map(|v| Setting::new(v.into(), Source::Flag("band"))),
        }
    }
}

pub fn print_setting<T: fmt::Display>(name: &str, setting: &Option<Setting<T>>) {
    if let Some(setting) = setting {
        eprintln!("[verbose] {name}: {} ({})", setting.value, setting.source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_help() {
        for name in ENV_VARS {
            assert!(SETTINGS_HELP.contains(name), "{name}");
        }
        let layers = Layers::for_test(CliEnv::default(), SharedSettings::default());
        assert!(layers.profile(None).unwrap().is_none());
        let rig = layers.rig(&RigArgs::default()).unwrap();
        assert_eq!(rig.ptt.value, "none");
        assert_eq!(rig.rigctld.value, "127.0.0.1:4532");
        assert_eq!(rig.lead.value, 50);
        assert_eq!(rig.tail.value, 200);
        assert!(rig.device.is_none());
        assert!(rig.frequency_hz.is_none());
        assert!(rig.band.is_none());
        assert_eq!(rig.ptt.source, Source::Default);
        assert_eq!(rig.lead.source, Source::Default);
    }
    #[test]
    fn precedence_and_direction() {
        let shared = SharedSettings {
            callsign: Some("FILE".into()),
            profile: Some("HF_ROBUST".into()),
            input_device: Some("input".into()),
            output_device: Some("output".into()),
            ..Default::default()
        };
        let env = CliEnv::from_pairs([
            ("COPPA_CALLSIGN", OsString::from("ENV")),
            ("COPPA_DEVICE", OsString::from("env device")),
        ]);
        let layers = Layers::for_test(env, shared);
        assert_eq!(layers.callsign(None).unwrap().unwrap().value, "ENV");
        assert_eq!(
            layers.callsign(None).unwrap().unwrap().source,
            Source::Env("COPPA_CALLSIGN")
        );
        assert_eq!(
            layers.callsign(Some("FLAG")).unwrap().unwrap().source,
            Source::Flag("callsign")
        );
        assert_eq!(
            layers.callsign(Some("FLAG")).unwrap().unwrap().value,
            "FLAG"
        );
        assert_eq!(layers.profile(None).unwrap().unwrap().value, "HF_ROBUST");
        assert_eq!(
            layers.profile(None).unwrap().unwrap().source,
            layers.file_source("[engine] profile")
        );
        let file_only = Layers::for_test(
            CliEnv::default(),
            SharedSettings {
                input_device: Some("input".into()),
                output_device: Some("output".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            file_only
                .device(None, Direction::Input)
                .unwrap()
                .unwrap()
                .value,
            "input"
        );
        assert_eq!(
            file_only
                .device(None, Direction::Output)
                .unwrap()
                .unwrap()
                .value,
            "output"
        );
        assert_eq!(
            layers
                .device(None, Direction::Input)
                .unwrap()
                .unwrap()
                .value,
            "env device"
        );
        assert_eq!(
            layers
                .device(Some("flag"), Direction::Output)
                .unwrap()
                .unwrap()
                .value,
            "flag"
        );
    }
    #[test]
    fn frequency_pair_and_bad_numbers() {
        let shared = SharedSettings {
            frequency_hz: Some(14_074_000),
            ..Default::default()
        };
        let layers = Layers::for_test(CliEnv::default(), shared);
        let rig = layers
            .rig(&RigArgs {
                band: Some("2m".into()),
                ..Default::default()
            })
            .unwrap();
        assert!(rig.frequency_hz.is_none());
        assert_eq!(rig.band.unwrap().value, "2m");
        let env = CliEnv::from_pairs([("COPPA_PTT_LEAD_MS", OsString::from("abc"))]);
        assert!(Layers::for_test(env, SharedSettings::default())
            .rig(&RigArgs::default())
            .is_err());
    }
    #[test]
    fn ptt_compatibility() {
        let mut shared = SharedSettings {
            ptt_method: Some("Rigctld".into()),
            ..Default::default()
        };
        let layers = Layers::for_test(CliEnv::default(), shared.clone());
        assert_eq!(
            layers
                .rig(&RigArgs::default())
                .unwrap()
                .ptt_method()
                .unwrap(),
            "rigctld"
        );
        shared.ptt_method = Some("serial:/dev/ttyUSB0:dtr".into());
        let layers = Layers::for_test(CliEnv::default(), shared);
        assert!(layers
            .rig(&RigArgs::default())
            .unwrap()
            .ptt_method()
            .unwrap_err()
            .to_string()
            .contains("coppad-only"));
        let gpio = SharedSettings {
            ptt_method: Some("gpio:17".into()),
            ..Default::default()
        };
        let layers = Layers::for_test(CliEnv::default(), gpio);
        assert!(layers
            .rig(&RigArgs::default())
            .unwrap()
            .ptt_method()
            .unwrap_err()
            .to_string()
            .contains("coppad-only"));
        let env = CliEnv::from_pairs([("COPPA_PTT", OsString::from("bogus"))]);
        let layers = Layers::for_test(env, SharedSettings::default());
        let err = layers
            .rig(&RigArgs::default())
            .unwrap()
            .ptt_method()
            .unwrap_err();
        assert!(err.to_string().contains("COPPA_PTT"), "{err}");
        let flags = RigArgs {
            ptt: Some("Rigctld".into()),
            ..Default::default()
        };
        let err = layers.rig(&flags).unwrap().ptt_method().unwrap_err();
        assert!(
            err.to_string().contains("unrecognized --ptt value"),
            "{err}"
        );
    }
    #[test]
    fn all_rig_fields_obey_flag_environment_file_default_order() {
        let shared = SharedSettings {
            output_device: Some("file-device".into()),
            ptt_method: Some("vox".into()),
            rigctld_address: Some("file:4532".into()),
            ptt_pre_delay_ms: Some(75),
            ptt_tail_delay_ms: Some(500),
            frequency_hz: Some(14_074_000),
            band: Some("20m".into()),
            ..Default::default()
        };
        let file = Layers::for_test(CliEnv::default(), shared.clone());
        let rig = file.rig(&RigArgs::default()).unwrap();
        assert_eq!(rig.device.as_ref().unwrap().value, "file-device");
        assert_eq!(rig.ptt.value, "vox");
        assert_eq!(rig.rigctld.value, "file:4532");
        assert_eq!(rig.lead.value, 75);
        assert_eq!(rig.tail.value, 500);
        assert_eq!(rig.frequency_hz.as_ref().unwrap().value, 14_074_000);
        assert_eq!(rig.band.as_ref().unwrap().value, "20m");
        assert_eq!(
            rig.device.as_ref().unwrap().source,
            file.file_source("[audio] output_device")
        );
        assert_eq!(rig.ptt.source, file.file_source("[radio] ptt_method"));
        assert_eq!(
            rig.rigctld.source,
            file.file_source("[radio] rigctld_address")
        );
        assert_eq!(
            rig.lead.source,
            file.file_source("[radio] ptt_pre_delay_ms")
        );
        assert_eq!(
            rig.tail.source,
            file.file_source("[radio] ptt_tail_delay_ms")
        );
        assert_eq!(
            rig.frequency_hz.as_ref().unwrap().source,
            file.file_source("[radio] frequency_hz")
        );
        assert_eq!(
            rig.band.as_ref().unwrap().source,
            file.file_source("[radio] band")
        );
        let env = CliEnv::from_pairs([
            ("COPPA_DEVICE", OsString::from("env-device")),
            ("COPPA_PTT", OsString::from("rigctld")),
            ("COPPA_RIGCTLD", OsString::from("env:4532")),
            ("COPPA_PTT_LEAD_MS", OsString::from("80")),
            ("COPPA_PTT_TAIL_MS", OsString::from("600")),
            ("COPPA_FREQUENCY_HZ", OsString::from("144500000")),
            ("COPPA_BAND", OsString::from("2m")),
            ("COPPA_PROFILE", OsString::from("HF_ROBUST")),
        ]);
        let env_layers = Layers::for_test(env, shared);
        let rig = env_layers.rig(&RigArgs::default()).unwrap();
        assert_eq!(rig.device.as_ref().unwrap().value, "env-device");
        assert_eq!(rig.ptt.value, "rigctld");
        assert_eq!(rig.rigctld.value, "env:4532");
        assert_eq!(rig.lead.value, 80);
        assert_eq!(rig.tail.value, 600);
        assert_eq!(rig.frequency_hz.as_ref().unwrap().value, 144_500_000);
        assert_eq!(rig.band.as_ref().unwrap().value, "2m");
        assert_eq!(
            rig.device.as_ref().unwrap().source,
            Source::Env("COPPA_DEVICE")
        );
        assert_eq!(rig.ptt.source, Source::Env("COPPA_PTT"));
        assert_eq!(rig.rigctld.source, Source::Env("COPPA_RIGCTLD"));
        assert_eq!(rig.lead.source, Source::Env("COPPA_PTT_LEAD_MS"));
        assert_eq!(rig.tail.source, Source::Env("COPPA_PTT_TAIL_MS"));
        assert_eq!(
            rig.frequency_hz.as_ref().unwrap().source,
            Source::Env("COPPA_FREQUENCY_HZ")
        );
        assert_eq!(rig.band.as_ref().unwrap().source, Source::Env("COPPA_BAND"));
        let flags = RigArgs {
            device: Some("flag-device".into()),
            ptt: Some("none".into()),
            rigctld: Some("flag:4532".into()),
            ptt_lead_ms: Some(90),
            ptt_tail_ms: Some(700),
            frequency_hz: Some(50_000_000),
            band: Some("6m".into()),
        };
        let rig = env_layers.rig(&flags).unwrap();
        assert_eq!(rig.device.as_ref().unwrap().value, "flag-device");
        assert_eq!(rig.ptt.value, "none");
        assert_eq!(rig.rigctld.value, "flag:4532");
        assert_eq!(rig.lead.value, 90);
        assert_eq!(rig.tail.value, 700);
        assert_eq!(rig.frequency_hz.as_ref().unwrap().value, 50_000_000);
        assert_eq!(rig.band.as_ref().unwrap().value, "6m");
        assert_eq!(rig.device.as_ref().unwrap().source, Source::Flag("device"));
        assert_eq!(rig.ptt.source, Source::Flag("ptt"));
        assert_eq!(rig.rigctld.source, Source::Flag("rigctld"));
        assert_eq!(rig.lead.source, Source::Flag("ptt-lead-ms"));
        assert_eq!(rig.tail.source, Source::Flag("ptt-tail-ms"));
        assert_eq!(
            rig.frequency_hz.as_ref().unwrap().source,
            Source::Flag("frequency-hz")
        );
        assert_eq!(rig.band.as_ref().unwrap().source, Source::Flag("band"));
        assert_eq!(
            env_layers.profile(None).unwrap().unwrap().source,
            Source::Env("COPPA_PROFILE")
        );
        assert!(matches!(
            env_layers
                .profile(Some("VHF_FAST"))
                .unwrap()
                .unwrap()
                .source,
            Source::Flag("profile")
        ));
    }

    #[test]
    fn empty_and_malformed_environment_values_are_handled() {
        let env = CliEnv::from_pairs([
            ("COPPA_CALLSIGN", OsString::from("")),
            ("COPPA_PTT", OsString::from(" vox ")),
        ]);
        let shared = SharedSettings {
            callsign: Some("W5AU".into()),
            ..Default::default()
        };
        let layers = Layers::for_test(env, shared);
        assert_eq!(layers.callsign(None).unwrap().unwrap().value, "W5AU");
        assert_eq!(
            layers
                .rig(&RigArgs::default())
                .unwrap()
                .ptt_method()
                .unwrap(),
            "vox"
        );
        let env = CliEnv::from_pairs([("COPPA_FREQUENCY_HZ", OsString::from("7.0"))]);
        let err = Layers::for_test(env, SharedSettings::default())
            .rig(&RigArgs::default())
            .err()
            .unwrap();
        assert!(err.to_string().contains("COPPA_FREQUENCY_HZ"));
        let env = CliEnv::from_pairs([("COPPA_PROFILE", OsString::from("HF_STANDRD"))]);
        let err = Layers::for_test(env, SharedSettings::default())
            .profile(None)
            .err()
            .unwrap();
        assert!(err.to_string().contains("from COPPA_PROFILE"));
        let shared = SharedSettings {
            profile: Some("HF_STANDRD".into()),
            ..Default::default()
        };
        let err = Layers::for_test(CliEnv::default(), shared)
            .profile(None)
            .err()
            .unwrap();
        assert!(err.to_string().contains("[engine] profile in station.toml"));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_environment_value_names_its_variable() {
        use std::os::unix::ffi::OsStringExt;
        let env = CliEnv::from_pairs([("COPPA_DEVICE", OsString::from_vec(vec![0xff]))]);
        let err = Layers::for_test(env, SharedSettings::default())
            .device(None, Direction::Input)
            .err()
            .unwrap();
        assert!(err.to_string().contains("COPPA_DEVICE"));
    }
}
