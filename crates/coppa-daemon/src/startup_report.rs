//! What `coppad` states about itself just before `Daemon ready` (COP-50):
//! the host listeners and the PTT backend startup left in control.

use std::fmt;
use std::net::SocketAddr;

use crate::config::{PttConfig, PttSerialLine};

/// One host listener at readiness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listener {
    /// Bound; the address is the socket's own (the real port when 0 was configured).
    Listening(SocketAddr),
    /// Turned off in `[host]`.
    Disabled,
    /// This build lacks the cargo feature the listener needs. Only the
    /// WebSocket listener has one, so a `websocket` build never constructs it.
    #[cfg_attr(feature = "websocket", allow(dead_code))]
    NotBuilt,
}

impl fmt::Display for Listener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Listener::Listening(addr) => write!(f, "{addr}"),
            Listener::Disabled => f.write_str("disabled"),
            Listener::NotBuilt => f.write_str("not-built"),
        }
    }
}

/// Whether any bound listener accepts connections from other hosts. An
/// IPv4-mapped loopback address (`::ffff:127.0.0.1`) counts as loopback.
pub fn exposed<'a>(listeners: impl IntoIterator<Item = &'a Listener>) -> bool {
    listeners
        .into_iter()
        .any(|l| matches!(l, Listener::Listening(addr) if !addr.ip().to_canonical().is_loopback()))
}

/// The PTT backend actually in control after startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PttActive {
    /// Nothing keys the radio: `ptt_method = "none"`, or a rigctld that
    /// could not be reached and was replaced with NullPtt.
    None,
    Vox,
    Rigctld,
    Serial,
    Gpio,
    /// COP-52: CM108/CM119 HID GPIO PTT.
    Cm108,
}

impl PttActive {
    fn as_str(self) -> &'static str {
        match self {
            PttActive::None => "none",
            PttActive::Vox => "vox",
            PttActive::Rigctld => "rigctld",
            PttActive::Serial => "serial",
            PttActive::Gpio => "gpio",
            PttActive::Cm108 => "cm108",
        }
    }
}

/// PTT as `EventLoop::new` left it: what was configured, what is in
/// control, and how the startup release (`set_ptt(Rx)`) went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PttStartup {
    pub configured: PttConfig,
    pub rigctld_address: String,
    pub active: PttActive,
    pub release: Result<(), String>,
    /// COP-52: `[radio] ptt_invert`, shown by [`PttStartup::method`] for the
    /// backends it applies to.
    pub inverted: bool,
}

impl PttStartup {
    /// `backend_is_rigctld` is `PttBackend::is_rigctld()`: false for a rigctld
    /// that could not be reached, which `create_ptt` replaced with NullPtt.
    /// `inverted` is `[radio] ptt_invert`.
    pub fn new(
        configured: PttConfig,
        rigctld_address: &str,
        backend_is_rigctld: bool,
        release: Result<(), String>,
        inverted: bool,
    ) -> Self {
        let active = match &configured {
            PttConfig::Rigctld if backend_is_rigctld => PttActive::Rigctld,
            PttConfig::None | PttConfig::Rigctld => PttActive::None,
            PttConfig::Vox => PttActive::Vox,
            PttConfig::Serial { .. } => PttActive::Serial,
            PttConfig::Gpio { .. } => PttActive::Gpio,
            PttConfig::Cm108 { .. } => PttActive::Cm108,
        };
        Self {
            configured,
            rigctld_address: rigctld_address.to_string(),
            active,
            release,
            inverted,
        }
    }

    /// The configured method and its target, e.g. `rigctld 127.0.0.1:4532`.
    /// COP-52: a serial, gpio or cm108 method gains ` inverted` when
    /// `[radio] ptt_invert` is set, e.g. `gpio 529 inverted`.
    pub fn method(&self) -> String {
        let method = self.base_method();
        if self.inverted && self.configured.applies_ptt_invert() {
            format!("{method} inverted")
        } else {
            method
        }
    }

    fn base_method(&self) -> String {
        match &self.configured {
            PttConfig::None => "none".to_string(),
            PttConfig::Vox => "vox".to_string(),
            PttConfig::Rigctld => format!("rigctld {}", self.rigctld_address),
            PttConfig::Serial { port, line } => {
                let line = match line {
                    PttSerialLine::Dtr => "dtr",
                    PttSerialLine::Rts => "rts",
                };
                format!("serial {port} {line}")
            }
            PttConfig::Gpio { pin } => format!("gpio {pin}"),
            PttConfig::Cm108 { device, gpio } => format!("cm108 {device} gpio {gpio}"),
        }
    }

    /// The backend in control: `none`, `vox`, `rigctld`, `serial`, `gpio` or
    /// `cm108`.
    pub fn active(&self) -> &'static str {
        self.active.as_str()
    }

    /// Whether the active backend drives a PTT line coppad can release
    /// (NullPtt and VOX always report success without touching the radio).
    pub fn controls_a_line(&self) -> bool {
        matches!(
            self.active,
            PttActive::Rigctld | PttActive::Serial | PttActive::Gpio | PttActive::Cm108
        )
    }

    /// `released`, `release failed: <error>`, or `not controlled`.
    pub fn state(&self) -> String {
        if !self.controls_a_line() {
            return "not controlled".to_string();
        }
        match &self.release {
            Ok(()) => "released".to_string(),
            Err(e) => format!("release failed: {e}"),
        }
    }

    /// Why PTT keeps the daemon from calling itself ready, if it does: an
    /// unreachable rigctld (COP-32), or a backend that refused the startup
    /// release.
    pub fn problem(&self) -> Option<String> {
        if self.configured == PttConfig::Rigctld && self.active != PttActive::Rigctld {
            return Some(format!(
                "no PTT: rigctld at {} is unreachable",
                self.rigctld_address
            ));
        }
        match &self.release {
            Err(e) if self.controls_a_line() => Some(format!("PTT release at startup failed: {e}")),
            _ => None,
        }
    }

    /// The guardian's state when it was not started: `Off` where only
    /// `[radio] ptt_guardian = false` kept an eligible backend from having
    /// one, `NotUsed` everywhere else.
    pub fn guardian_not_started(&self, enabled_in_config: bool) -> Guardian {
        match self.active {
            PttActive::Rigctld | PttActive::Gpio | PttActive::Cm108 if !enabled_in_config => {
                Guardian::Off
            }
            _ => Guardian::NotUsed,
        }
    }
}

/// The COP-31 PTT guardian at readiness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Guardian {
    Running {
        pid: u32,
    },
    FailedToStart(String),
    /// Eligible, but `[radio] ptt_guardian = false`.
    Off,
    /// The PTT backend has no guardian (none, VOX, serial, unreachable rigctld).
    /// rigctld, GPIO and CM108 PTT are guarded.
    NotUsed,
}

impl fmt::Display for Guardian {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Guardian::Running { pid } => write!(f, "running, pid {pid}"),
            Guardian::FailedToStart(e) => write!(f, "failed to start: {e}"),
            Guardian::Off => f.write_str("off ([radio] ptt_guardian = false)"),
            Guardian::NotUsed => f.write_str("not used"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> Listener {
        Listener::Listening(s.parse().unwrap())
    }

    fn ptt(configured: PttConfig, active: PttActive, release: Result<(), String>) -> PttStartup {
        PttStartup {
            configured,
            rigctld_address: "127.0.0.1:4532".into(),
            active,
            release,
            inverted: false,
        }
    }

    fn cm108_config() -> PttConfig {
        PttConfig::Cm108 {
            device: "/dev/hidraw0".into(),
            gpio: 3,
        }
    }

    #[test]
    fn listener_states_print_the_address_or_why_there_is_none() {
        assert_eq!(addr("127.0.0.1:8300").to_string(), "127.0.0.1:8300");
        assert_eq!(addr("[::1]:8400").to_string(), "[::1]:8400");
        assert_eq!(Listener::Disabled.to_string(), "disabled");
        assert_eq!(Listener::NotBuilt.to_string(), "not-built");
    }

    #[test]
    fn only_a_bound_non_loopback_listener_is_exposed() {
        assert!(!exposed(&[addr("127.0.0.1:1"), addr("[::1]:2")]));
        assert!(!exposed(&[addr("[::ffff:127.0.0.1]:3")]));
        assert!(!exposed(&[Listener::Disabled, Listener::NotBuilt]));
        assert!(exposed(&[addr("127.0.0.1:1"), addr("0.0.0.0:2")]));
        assert!(exposed(&[addr("192.168.1.5:8300")]));
        assert!(exposed(&[addr("[::]:8300")]));
    }

    #[test]
    fn new_maps_an_unreachable_rigctld_to_no_active_backend() {
        let fallback = PttStartup::new(PttConfig::Rigctld, "127.0.0.1:1", false, Ok(()), false);
        assert_eq!(fallback.active, PttActive::None);
        let connected = PttStartup::new(PttConfig::Rigctld, "127.0.0.1:1", true, Ok(()), false);
        assert_eq!(connected.active, PttActive::Rigctld);
        let gpio = PttStartup::new(
            PttConfig::Gpio { pin: "17".into() },
            "",
            false,
            Ok(()),
            false,
        );
        assert_eq!(gpio.active, PttActive::Gpio);
        let cm108 = PttStartup::new(cm108_config(), "", false, Ok(()), true);
        assert_eq!(cm108.active, PttActive::Cm108);
        assert!(cm108.inverted);
    }

    #[test]
    fn cm108_method_names_the_device_and_gpio() {
        let cm108 = ptt(cm108_config(), PttActive::Cm108, Ok(()));
        assert_eq!(cm108.method(), "cm108 /dev/hidraw0 gpio 3");
        assert_eq!(cm108.active(), "cm108");
        assert!(cm108.controls_a_line());
        assert_eq!(cm108.state(), "released");
        assert_eq!(cm108.problem(), None);

        let failed = ptt(
            cm108_config(),
            PttActive::Cm108,
            Err("failed to write CM108 report to /dev/hidraw0".into()),
        );
        assert_eq!(
            failed.state(),
            "release failed: failed to write CM108 report to /dev/hidraw0"
        );
        assert_eq!(
            failed.problem(),
            Some(
                "PTT release at startup failed: failed to write CM108 report to /dev/hidraw0"
                    .to_string()
            )
        );
    }

    #[test]
    fn inverted_line_methods_say_so() {
        let inverted = |configured: PttConfig, active: PttActive| PttStartup {
            inverted: true,
            ..ptt(configured, active, Ok(()))
        };
        assert_eq!(
            inverted(PttConfig::Gpio { pin: "17".into() }, PttActive::Gpio).method(),
            "gpio 17 inverted"
        );
        let serial = PttConfig::Serial {
            port: "/dev/ttyUSB0".into(),
            line: PttSerialLine::Rts,
        };
        assert_eq!(
            inverted(serial, PttActive::Serial).method(),
            "serial /dev/ttyUSB0 rts inverted"
        );
        assert_eq!(
            inverted(cm108_config(), PttActive::Cm108).method(),
            "cm108 /dev/hidraw0 gpio 3 inverted"
        );
        assert_eq!(
            inverted(PttConfig::Rigctld, PttActive::Rigctld).method(),
            "rigctld 127.0.0.1:4532"
        );
    }

    #[test]
    fn method_names_the_target() {
        assert_eq!(
            ptt(PttConfig::None, PttActive::None, Ok(())).method(),
            "none"
        );
        assert_eq!(ptt(PttConfig::Vox, PttActive::Vox, Ok(())).method(), "vox");
        assert_eq!(
            ptt(PttConfig::Rigctld, PttActive::Rigctld, Ok(())).method(),
            "rigctld 127.0.0.1:4532"
        );
        let serial = PttConfig::Serial {
            port: "/dev/ttyUSB0".into(),
            line: PttSerialLine::Rts,
        };
        assert_eq!(
            ptt(serial, PttActive::Serial, Ok(())).method(),
            "serial /dev/ttyUSB0 rts"
        );
        let gpio = PttConfig::Gpio { pin: "17".into() };
        assert_eq!(ptt(gpio, PttActive::Gpio, Ok(())).method(), "gpio 17");
    }

    #[test]
    fn state_reports_the_startup_release_only_for_a_driven_line() {
        assert_eq!(
            ptt(PttConfig::Rigctld, PttActive::Rigctld, Ok(())).state(),
            "released"
        );
        assert_eq!(
            ptt(
                PttConfig::Rigctld,
                PttActive::Rigctld,
                Err("rigctld set_ptt failed: RPRT -1".into())
            )
            .state(),
            "release failed: rigctld set_ptt failed: RPRT -1"
        );
        let fallback = ptt(PttConfig::Rigctld, PttActive::None, Ok(()));
        assert_eq!(fallback.active(), "none");
        assert_eq!(fallback.state(), "not controlled");
        assert_eq!(
            ptt(PttConfig::Vox, PttActive::Vox, Ok(())).state(),
            "not controlled"
        );
    }

    #[test]
    fn problem_names_an_unreachable_rigctld_or_a_refused_release() {
        assert_eq!(
            ptt(PttConfig::Rigctld, PttActive::None, Ok(())).problem(),
            Some("no PTT: rigctld at 127.0.0.1:4532 is unreachable".to_string())
        );
        assert_eq!(
            ptt(
                PttConfig::Rigctld,
                PttActive::Rigctld,
                Err("RPRT -1".into())
            )
            .problem(),
            Some("PTT release at startup failed: RPRT -1".to_string())
        );
        assert_eq!(
            ptt(PttConfig::Rigctld, PttActive::Rigctld, Ok(())).problem(),
            None
        );
        // VOX and none drive no line, so a release cannot fail them.
        assert_eq!(
            ptt(PttConfig::Vox, PttActive::Vox, Err("unused".into())).problem(),
            None
        );
    }

    #[test]
    fn guardian_is_off_only_where_config_kept_it_from_running() {
        assert_eq!(
            ptt(PttConfig::Rigctld, PttActive::Rigctld, Ok(())).guardian_not_started(false),
            Guardian::Off
        );
        let gpio = PttConfig::Gpio { pin: "17".into() };
        assert_eq!(
            ptt(gpio, PttActive::Gpio, Ok(())).guardian_not_started(false),
            Guardian::Off
        );
        assert_eq!(
            ptt(cm108_config(), PttActive::Cm108, Ok(())).guardian_not_started(false),
            Guardian::Off
        );
        assert_eq!(
            ptt(cm108_config(), PttActive::Cm108, Ok(())).guardian_not_started(true),
            Guardian::NotUsed
        );
        let serial = PttConfig::Serial {
            port: "/dev/ttyUSB0".into(),
            line: PttSerialLine::Dtr,
        };
        assert_eq!(
            ptt(serial, PttActive::Serial, Ok(())).guardian_not_started(false),
            Guardian::NotUsed
        );
        assert_eq!(
            ptt(PttConfig::Rigctld, PttActive::None, Ok(())).guardian_not_started(true),
            Guardian::NotUsed
        );
    }

    #[test]
    fn guardian_states_never_claim_it_started_when_it_did_not() {
        for g in [
            Guardian::FailedToStart("spawn failed".into()),
            Guardian::Off,
            Guardian::NotUsed,
        ] {
            assert!(!g.to_string().contains("running"), "{g}");
            assert!(!g.to_string().contains("PTT guardian started"), "{g}");
        }
        assert_eq!(Guardian::Running { pid: 42 }.to_string(), "running, pid 42");
    }
}
