//! COP-140: what coppad advertises over mDNS, decided from the WebSocket
//! listener's bound address. The `mdns-sd` glue below is behind the `mdns`
//! feature; this part compiles everywhere so its tests run on every CI OS.
//!
//! One host name and one service, both fixed: `coppad.local.` and
//! `coppad._coppa._tcp.local.` at the WebSocket port (the operator page at `/`
//! and the health check at `/health`). The VARA-style, ARDOP and KISS ports
//! are never advertised: they have no token and can key the transmitter.

use std::net::{IpAddr, SocketAddr};

/// The advertised host name (A records, and AAAA for `[::]`).
pub const HOST_NAME: &str = "coppad.local.";
/// The advertised DNS-SD service type.
pub const SERVICE_TYPE: &str = "_coppa._tcp.local.";
/// The advertised service instance name.
pub const INSTANCE: &str = "coppad";
/// COP-85: the advertised host and service instance names. Without an
/// instance they are [`HOST_NAME`] and [`INSTANCE`]; `coppad --instance vhf`
/// advertises `coppad-vhf.local.` and `coppad-vhf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    /// Host name, with the trailing dot.
    pub host: String,
    /// Service instance name.
    pub instance: String,
}

impl Names {
    /// The names for `coppad --instance NAME`, or the fixed names without one.
    pub fn for_instance(instance: Option<&str>) -> Self {
        match instance {
            Some(name) => Self {
                host: format!("{INSTANCE}-{name}.local."),
                instance: format!("{INSTANCE}-{name}"),
            },
            None => Self {
                host: HOST_NAME.to_string(),
                instance: INSTANCE.to_string(),
            },
        }
    }
}

/// The TXT record: no version, callsign or other station data, because any
/// host on the network can read it.
pub const TXT: [(&str, &str); 3] = [("txtvers", "1"), ("path", "/"), ("health", "/health")];

/// Which of this host's addresses the host name resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addresses {
    /// `bind_address = "0.0.0.0"`: every non-loopback IPv4 address. The
    /// listener refuses IPv6 there, so AAAA records would mislead.
    AllIpv4,
    /// `bind_address = "[::]"`: every non-loopback address.
    All,
    /// A specific `bind_address`: that address alone.
    Only(IpAddr),
}

impl std::fmt::Display for Addresses {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Addresses::AllIpv4 => f.write_str("every non-loopback IPv4 address"),
            Addresses::All => f.write_str("every non-loopback address"),
            Addresses::Only(ip) => write!(f, "{ip}"),
        }
    }
}

/// What to advertise: the bound port and the addresses behind the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advertisement {
    pub port: u16,
    pub addresses: Addresses,
}

/// The advertisement for a WebSocket listener bound to `bound`. `Err` when
/// other hosts cannot reach `bound`: a loopback address, IPv4-mapped
/// loopback included.
pub fn advertisement(bound: SocketAddr) -> Result<Advertisement, String> {
    let ip = bound.ip().to_canonical();
    if ip.is_loopback() {
        return Err(format!("{ip} is a loopback address"));
    }
    let addresses = match ip {
        IpAddr::V4(v4) if v4.is_unspecified() => Addresses::AllIpv4,
        IpAddr::V6(v6) if v6.is_unspecified() => Addresses::All,
        ip => Addresses::Only(ip),
    };
    Ok(Advertisement {
        port: bound.port(),
        addresses,
    })
}

/// The startup error for an mDNS config whose `bind_address` (as written, or
/// the address a host name bound to) other hosts cannot reach.
pub fn unreachable_bind_address(bind_address: &str) -> String {
    format!(
        "[host] mdns_enabled = true needs a bind_address other hosts can reach, but \
         bind_address = {bind_address:?} is loopback, so coppad.local would send clients to \
         a port that refuses them; set bind_address = \"0.0.0.0\" or this host's LAN address, \
         on a trusted, firewalled network"
    )
}

#[cfg(feature = "mdns")]
pub use responder::{describe, service_info, service_info_named, Note, Responder};

#[cfg(feature = "mdns")]
mod responder {
    use super::{Addresses, Advertisement, Names, SERVICE_TYPE, TXT};
    use mdns_sd::{DaemonEvent, IfKind, ServiceDaemon, ServiceInfo};

    /// How long [`Responder::stop`] waits for each of unregister and shutdown.
    const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

    /// The `mdns-sd` record set for `ad`. Building one opens no socket.
    pub fn service_info(ad: &Advertisement) -> Result<ServiceInfo, String> {
        service_info_named(ad, &Names::for_instance(None))
    }

    /// COP-85: [`service_info`] under `names` (an instance's own names).
    pub fn service_info_named(ad: &Advertisement, names: &Names) -> Result<ServiceInfo, String> {
        let (instance, host) = (names.instance.as_str(), names.host.as_str());
        let info = match ad.addresses {
            Addresses::Only(ip) => {
                ServiceInfo::new(SERVICE_TYPE, instance, host, ip, ad.port, &TXT[..])
            }
            Addresses::AllIpv4 | Addresses::All => {
                ServiceInfo::new(SERVICE_TYPE, instance, host, "", ad.port, &TXT[..])
                    .map(ServiceInfo::enable_addr_auto)
            }
        };
        let mut info = info.map_err(|e| format!("invalid mDNS service record: {e}"))?;
        if ad.addresses == Addresses::AllIpv4 {
            info.set_interfaces(vec![IfKind::IPv4]);
        }
        Ok(info)
    }

    /// What one responder event means for the operator log.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Note {
        /// Another host holds the name; `mdns-sd` picked a new one (RFC 6762
        /// section 9). Logged at WARN.
        Renamed { original: String, new_name: String },
        /// A socket or send failure. Logged at WARN.
        Failed(String),
        /// Announcements, responses and address changes. Logged at DEBUG.
        Routine(String),
    }

    impl Note {
        pub fn level(&self) -> tracing::Level {
            match self {
                Note::Renamed { .. } | Note::Failed(_) => tracing::Level::WARN,
                Note::Routine(_) => tracing::Level::DEBUG,
            }
        }

        fn log(&self) {
            match self {
                Note::Renamed { original, new_name } => tracing::warn!(
                    %original,
                    %new_name,
                    "mDNS name already in use on this network; advertising under the new name"
                ),
                Note::Failed(error) => tracing::warn!(%error, "mDNS responder error"),
                Note::Routine(what) => tracing::debug!(event = %what, "mDNS responder event"),
            }
        }
    }

    /// Classify one responder event for the log.
    pub fn describe(event: &DaemonEvent) -> Note {
        match event {
            DaemonEvent::NameChange(change) => Note::Renamed {
                original: change.original.clone(),
                new_name: change.new_name.clone(),
            },
            DaemonEvent::Error(e) => Note::Failed(e.to_string()),
            other => Note::Routine(format!("{other:?}")),
        }
    }

    /// The running responder: one `mdns-sd` thread and its sockets.
    pub struct Responder {
        daemon: ServiceDaemon,
        fullname: String,
    }

    impl Responder {
        /// Open the mDNS sockets on every non-loopback interface, log the
        /// responder's events from a task, and register `ad`. Must run inside
        /// a tokio runtime.
        pub fn start(ad: &Advertisement) -> Result<Self, String> {
            Self::start_named(ad, &Names::for_instance(None))
        }

        /// COP-85: [`Responder::start`] under `names`.
        pub fn start_named(ad: &Advertisement, names: &Names) -> Result<Self, String> {
            let info = service_info_named(ad, names)?;
            let fullname = info.get_fullname().to_string();
            let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
            let started = (|| {
                // Loopback answers would let a same-host client "find" an
                // address no other host can use.
                daemon.disable_interface(vec![IfKind::LoopbackV4, IfKind::LoopbackV6])?;
                let events = daemon.monitor()?;
                tokio::spawn(async move {
                    while let Ok(event) = events.recv_async().await {
                        describe(&event).log();
                    }
                });
                daemon.register(info)
            })();
            if let Err(e) = started {
                let _ = daemon.shutdown();
                return Err(e.to_string());
            }
            Ok(Self { daemon, fullname })
        }

        /// Send goodbye packets and stop the responder thread, waiting at most
        /// `STOP_WAIT` (one second) for each.
        pub async fn stop(self) {
            if let Ok(done) = self.daemon.unregister(&self.fullname) {
                let _ = tokio::time::timeout(STOP_WAIT, done.recv_async()).await;
            }
            if let Ok(done) = self.daemon.shutdown() {
                let _ = tokio::time::timeout(STOP_WAIT, done.recv_async()).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ad(bound: &str) -> Result<Advertisement, String> {
        advertisement(bound.parse().unwrap())
    }

    #[test]
    fn unspecified_ipv4_advertises_every_ipv4_address() {
        assert_eq!(
            ad("0.0.0.0:8400"),
            Ok(Advertisement {
                port: 8400,
                addresses: Addresses::AllIpv4
            })
        );
    }

    #[test]
    fn unspecified_ipv6_advertises_every_address() {
        assert_eq!(ad("[::]:8400").unwrap().addresses, Addresses::All);
    }

    #[test]
    fn a_specific_address_is_advertised_alone() {
        let lan: IpAddr = "192.168.1.20".parse().unwrap();
        assert_eq!(
            ad("192.168.1.20:8400").unwrap().addresses,
            Addresses::Only(lan)
        );
        assert_eq!(
            ad("[::ffff:192.168.1.20]:8400").unwrap().addresses,
            Addresses::Only(lan),
            "an IPv4-mapped address is advertised as IPv4"
        );
    }

    #[test]
    fn loopback_is_refused() {
        for (bound, named) in [
            ("127.0.0.1:8400", "127.0.0.1"),
            ("[::1]:8400", "::1"),
            ("[::ffff:127.0.0.1]:8400", "127.0.0.1"),
        ] {
            let err = ad(bound).unwrap_err();
            assert!(err.contains(named), "{bound}: {err}");
        }
    }

    #[test]
    fn the_port_is_the_bound_port() {
        assert_eq!(ad("0.0.0.0:41234").unwrap().port, 41234);
    }

    #[test]
    fn instance_names_the_host_and_the_service() {
        assert_eq!(
            Names::for_instance(None),
            Names {
                host: "coppad.local.".to_string(),
                instance: "coppad".to_string(),
            }
        );
        assert_eq!(
            Names::for_instance(Some("vhf")),
            Names {
                host: "coppad-vhf.local.".to_string(),
                instance: "coppad-vhf".to_string(),
            }
        );
    }

    #[test]
    fn names_and_txt_are_fixed() {
        assert_eq!(HOST_NAME, "coppad.local.");
        assert_eq!(SERVICE_TYPE, "_coppa._tcp.local.");
        assert_eq!(INSTANCE, "coppad");
        assert_eq!(
            TXT,
            [("txtvers", "1"), ("path", "/"), ("health", "/health")]
        );
    }

    #[test]
    fn addresses_describe_themselves_for_the_log() {
        assert_eq!(
            Addresses::AllIpv4.to_string(),
            "every non-loopback IPv4 address"
        );
        assert_eq!(Addresses::All.to_string(), "every non-loopback address");
        assert_eq!(
            Addresses::Only("192.168.1.20".parse().unwrap()).to_string(),
            "192.168.1.20"
        );
    }

    #[test]
    fn the_unreachable_bind_address_message_names_the_address() {
        assert_eq!(
            unreachable_bind_address("127.0.0.1"),
            "[host] mdns_enabled = true needs a bind_address other hosts can reach, but \
             bind_address = \"127.0.0.1\" is loopback, so coppad.local would send clients to a \
             port that refuses them; set bind_address = \"0.0.0.0\" or this host's LAN \
             address, on a trusted, firewalled network"
        );
    }
}

#[cfg(all(test, feature = "mdns"))]
mod responder_tests {
    use super::*;
    use mdns_sd::{DaemonEvent, DnsNameChange, RRType};

    fn ad(addresses: Addresses) -> Advertisement {
        Advertisement {
            port: 41234,
            addresses,
        }
    }

    #[test]
    fn service_info_carries_the_fixed_names_port_and_txt() {
        let info = service_info(&ad(Addresses::AllIpv4)).unwrap();
        assert_eq!(info.get_fullname(), "coppad._coppa._tcp.local.");
        assert_eq!(info.get_hostname(), "coppad.local.");
        assert_eq!(info.get_port(), 41234);
        assert_eq!(info.get_property_val_str("txtvers"), Some("1"));
        assert_eq!(info.get_property_val_str("path"), Some("/"));
        assert_eq!(info.get_property_val_str("health"), Some("/health"));
    }

    #[test]
    fn a_specific_address_is_set_explicitly() {
        let lan: IpAddr = "192.168.1.20".parse().unwrap();
        let info = service_info(&ad(Addresses::Only(lan))).unwrap();
        assert!(info.get_addresses().contains(&lan));
        assert!(!info.is_addr_auto());
    }

    #[test]
    fn unspecified_addresses_are_automatic() {
        for addresses in [Addresses::AllIpv4, Addresses::All] {
            let info = service_info(&ad(addresses)).unwrap();
            assert!(info.is_addr_auto(), "{addresses:?}");
            assert!(info.get_addresses().is_empty(), "{addresses:?}");
        }
    }

    #[test]
    fn name_change_is_described_for_the_log() {
        let renamed = describe(&DaemonEvent::NameChange(DnsNameChange {
            original: "coppad.local.".into(),
            new_name: "coppad-2.local.".into(),
            rr_type: RRType::A,
            intf_name: "eth0".into(),
        }));
        assert_eq!(
            renamed,
            Note::Renamed {
                original: "coppad.local.".into(),
                new_name: "coppad-2.local.".into()
            }
        );
        assert_eq!(renamed.level(), tracing::Level::WARN);

        let failed = describe(&DaemonEvent::Error(mdns_sd::Error::Msg("boom".into())));
        assert_eq!(failed.level(), tracing::Level::WARN);
        assert!(matches!(failed, Note::Failed(ref e) if e.contains("boom")));

        let routine = describe(&DaemonEvent::IpAdd("192.168.1.20".parse().unwrap()));
        assert_eq!(routine.level(), tracing::Level::DEBUG);
    }
}
