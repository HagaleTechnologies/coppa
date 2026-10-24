//! VARA command/response protocol parser and formatter.

/// Commands received from a VARA client on the command port.
#[derive(Debug, Clone, PartialEq)]
pub enum VaraCommand {
    /// Set the station callsign: MYCALL <callsign>
    MyCall(String),
    /// Connect to a remote station: CONNECT <source> <dest> [via <digi>]
    Connect { source: String, destination: String },
    /// Disconnect the current session.
    Disconnect,
    /// Request to listen for connections: LISTEN ON|OFF. Anything other than
    /// `ON`/`OFF` parses as `Unknown`.
    Listen(bool),
    /// Two-tone TX calibration tone (coppa extension, not a VARA command):
    /// TUNE [seconds]. A non-positive or unparseable duration falls back to
    /// `None` (the caller's default).
    Tune(Option<f32>),
    /// Set compression: COMPRESSION ON|OFF
    Compression(bool),
    /// Set bandwidth: BW500|BW2300|BW2750
    Bandwidth(u32),
    /// Abort the current operation.
    Abort,
    /// Get the software version.
    Version,
    /// Unknown command.
    Unknown(String),
}

/// Responses sent to a VARA client on the command port.
#[derive(Debug, Clone, PartialEq)]
pub enum VaraResponse {
    /// OK response.
    Ok,
    /// Wrong command.
    Wrong,
    /// VARA version string.
    Version(String),
    /// Connection pending.
    Pending,
    /// Connected to remote station: the calling station, the called station,
    /// and the occupied bandwidth in Hz.
    Connected(String, String, u32),
    /// Disconnected.
    Disconnected,
    /// PTT state change.
    Ptt(bool),
    /// Buffer count.
    Buffer(usize),
    /// Busy state.
    Busy(bool),
    /// Signal-to-noise ratio.
    Snr(i32),
    /// Keepalive, sent every 60 seconds.
    IAmAlive,
}

impl VaraCommand {
    /// Parse a VARA command string.
    pub fn parse(input: &str) -> Self {
        let trimmed = input.trim();
        let parts: Vec<&str> = trimmed.splitn(4, ' ').collect();

        match parts.first().map(|s| s.to_uppercase()).as_deref() {
            Some("MYCALL") => match trimmed.split_whitespace().nth(1) {
                Some(call) => VaraCommand::MyCall(call.to_string()),
                None => VaraCommand::Unknown(trimmed.to_string()),
            },
            Some("CONNECT") => {
                if let (Some(&src), Some(&dst)) = (parts.get(1), parts.get(2)) {
                    VaraCommand::Connect {
                        source: src.to_string(),
                        destination: dst.to_string(),
                    }
                } else {
                    VaraCommand::Unknown(trimmed.to_string())
                }
            }
            Some("DISCONNECT") => VaraCommand::Disconnect,
            Some("LISTEN") => match parts.get(1).map(|s| s.to_uppercase()).as_deref() {
                Some("ON") => VaraCommand::Listen(true),
                Some("OFF") => VaraCommand::Listen(false),
                _ => VaraCommand::Unknown(trimmed.to_string()),
            },
            Some("TUNE") => {
                // Byte-for-byte the daemon's historical rule: everything after
                // the first space, trimmed, parsed as f32, kept only if
                // positive.
                let seconds = trimmed
                    .split_once(' ')
                    .and_then(|(_, rest)| rest.trim().parse::<f32>().ok())
                    .filter(|s| *s > 0.0);
                VaraCommand::Tune(seconds)
            }
            Some("COMPRESSION") => {
                let on = parts.get(1).map(|s| s.to_uppercase()) == Some("ON".to_string());
                VaraCommand::Compression(on)
            }
            Some("BW500") => VaraCommand::Bandwidth(500),
            Some("BW2300") => VaraCommand::Bandwidth(2300),
            Some("BW2750") => VaraCommand::Bandwidth(2750),
            Some("ABORT") => VaraCommand::Abort,
            Some("VERSION") => VaraCommand::Version,
            _ => VaraCommand::Unknown(trimmed.to_string()),
        }
    }
}

impl VaraResponse {
    /// Format response as a string to send to the client.
    ///
    /// Each response ends in a bare `\r`, as VARA's own responses do. Pat
    /// splits what it reads on `\r`, so a trailing `\n` would prefix the next
    /// response.
    pub fn format(&self) -> String {
        match self {
<<<<<<< HEAD
            VaraResponse::Ok => "OK\r".to_string(),
            VaraResponse::Wrong => "WRONG\r".to_string(),
            VaraResponse::Version(v) => format!("VERSION {}\r", v),
            VaraResponse::Pending => "PENDING\r".to_string(),
            VaraResponse::Connected(src, dst) => format!("CONNECTED {} {}\r", src, dst),
            VaraResponse::Disconnected => "DISCONNECTED\r".to_string(),
            VaraResponse::Ptt(on) => format!("PTT {}\r", if *on { "ON" } else { "OFF" }),
            VaraResponse::Buffer(n) => format!("BUFFER {}\r", n),
            VaraResponse::Busy(b) => format!("BUSY {}\r", if *b { "ON" } else { "OFF" }),
            VaraResponse::Snr(snr) => format!("SNR {}\r", snr),
=======
            VaraResponse::Ok => "OK\r\n".to_string(),
            VaraResponse::Wrong => "WRONG\r\n".to_string(),
            VaraResponse::Version(v) => format!("VERSION {}\r\n", v),
            VaraResponse::Pending => "PENDING\r\n".to_string(),
            VaraResponse::Connected(src, dst, bw) => {
                format!("CONNECTED {} {} {}\r\n", src, dst, bw)
            }
            VaraResponse::Disconnected => "DISCONNECTED\r\n".to_string(),
            VaraResponse::Ptt(on) => format!("PTT {}\r\n", if *on { "ON" } else { "OFF" }),
            VaraResponse::Buffer(n) => format!("BUFFER {}\r\n", n),
            VaraResponse::Busy(b) => format!("BUSY {}\r\n", if *b { "ON" } else { "OFF" }),
            VaraResponse::Snr(snr) => format!("SNR {}\r\n", snr),
            VaraResponse::IAmAlive => "IAMALIVE\r\n".to_string(),
>>>>>>> 61477ee30f698aa9fa7e9cd388e071659359bfdc
        }
    }

    /// The `VERSION` line coppa sends as its greeting and in reply to a
    /// `VERSION` command.
    pub fn coppa_version() -> Self {
        VaraResponse::Version(concat!("Coppa ", env!("CARGO_PKG_VERSION")).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mycall() {
        assert_eq!(
            VaraCommand::parse("MYCALL VK2ABC"),
            VaraCommand::MyCall("VK2ABC".to_string())
        );
    }

    #[test]
    fn test_parse_connect() {
        assert_eq!(
            VaraCommand::parse("CONNECT VK2ABC VK3DEF"),
            VaraCommand::Connect {
                source: "VK2ABC".to_string(),
                destination: "VK3DEF".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_disconnect() {
        assert_eq!(VaraCommand::parse("DISCONNECT"), VaraCommand::Disconnect);
    }

    #[test]
    fn test_parse_listen() {
        assert_eq!(VaraCommand::parse("LISTEN ON"), VaraCommand::Listen(true));
        assert_eq!(VaraCommand::parse("LISTEN OFF"), VaraCommand::Listen(false));
    }

    #[test]
    fn test_parse_compression() {
        assert_eq!(
            VaraCommand::parse("COMPRESSION ON"),
            VaraCommand::Compression(true)
        );
    }

    #[test]
    fn test_parse_bandwidth() {
        assert_eq!(VaraCommand::parse("BW500"), VaraCommand::Bandwidth(500));
        assert_eq!(VaraCommand::parse("BW2300"), VaraCommand::Bandwidth(2300));
    }

    #[test]
    fn test_parse_version() {
        assert_eq!(VaraCommand::parse("VERSION"), VaraCommand::Version);
    }

    #[test]
    fn test_parse_unknown() {
        match VaraCommand::parse("FOOBAR") {
            VaraCommand::Unknown(s) => assert_eq!(s, "FOOBAR"),
            _ => panic!("Expected Unknown"),
        }
    }

    #[test]
    fn test_format_responses() {
        assert_eq!(VaraResponse::Ok.format(), "OK\r");
        assert_eq!(VaraResponse::Wrong.format(), "WRONG\r");
        assert_eq!(
            VaraResponse::Version("Coppa 0.1.0".to_string()).format(),
            "VERSION Coppa 0.1.0\r"
        );
        assert_eq!(VaraResponse::Pending.format(), "PENDING\r");
        assert_eq!(
<<<<<<< HEAD
            VaraResponse::Connected("VK2ABC".to_string(), "VK3DEF".to_string()).format(),
            "CONNECTED VK2ABC VK3DEF\r"
        );
        assert_eq!(VaraResponse::Disconnected.format(), "DISCONNECTED\r");
        assert_eq!(VaraResponse::Ptt(true).format(), "PTT ON\r");
        assert_eq!(VaraResponse::Buffer(42).format(), "BUFFER 42\r");
        assert_eq!(VaraResponse::Busy(false).format(), "BUSY OFF\r");
        assert_eq!(VaraResponse::Snr(15).format(), "SNR 15\r");
=======
            VaraResponse::Connected("VK2ABC".to_string(), "VK3DEF".to_string(), 2400).format(),
            "CONNECTED VK2ABC VK3DEF 2400\r\n"
        );
        assert_eq!(VaraResponse::Disconnected.format(), "DISCONNECTED\r\n");
        assert_eq!(VaraResponse::Ptt(true).format(), "PTT ON\r\n");
        assert_eq!(VaraResponse::Buffer(42).format(), "BUFFER 42\r\n");
        assert_eq!(VaraResponse::Busy(false).format(), "BUSY OFF\r\n");
        assert_eq!(VaraResponse::Snr(15).format(), "SNR 15\r\n");
        assert_eq!(VaraResponse::IAmAlive.format(), "IAMALIVE\r\n");
    }

    #[test]
    fn test_parse_tune() {
        assert_eq!(VaraCommand::parse("TUNE"), VaraCommand::Tune(None));
        assert_eq!(VaraCommand::parse("tune 2.5"), VaraCommand::Tune(Some(2.5)));
        assert_eq!(VaraCommand::parse("TUNE 0"), VaraCommand::Tune(None)); // non-positive -> default
        assert_eq!(VaraCommand::parse("TUNE abc"), VaraCommand::Tune(None));
        assert!(matches!(
            VaraCommand::parse("TUNEX"),
            VaraCommand::Unknown(_)
        ));
    }

    #[test]
    fn test_parse_listen_requires_on_or_off() {
        assert_eq!(VaraCommand::parse("listen on"), VaraCommand::Listen(true));
        assert!(matches!(
            VaraCommand::parse("LISTEN"),
            VaraCommand::Unknown(_)
        ));
        assert!(matches!(
            VaraCommand::parse("LISTEN MAYBE"),
            VaraCommand::Unknown(_)
        ));
    }

    #[test]
    fn test_parse_mycall_takes_the_first_callsign() {
        assert_eq!(
            VaraCommand::parse("MYCALL  W5AU"),
            VaraCommand::MyCall("W5AU".into())
        );
        assert_eq!(
            VaraCommand::parse("MYCALL G8BPQ G8BPQ-1 G8BPQ-2"),
            VaraCommand::MyCall("G8BPQ".into())
        );
        assert!(matches!(
            VaraCommand::parse("MYCALL"),
            VaraCommand::Unknown(_)
        ));
    }

    #[test]
    fn coppa_version_names_the_package_version() {
        assert_eq!(
            VaraResponse::coppa_version(),
            VaraResponse::Version(format!("Coppa {}", env!("CARGO_PKG_VERSION")))
        );
>>>>>>> 61477ee30f698aa9fa7e9cd388e071659359bfdc
    }

    #[test]
    fn test_case_insensitive() {
        assert_eq!(
            VaraCommand::parse("mycall VK2ABC"),
            VaraCommand::MyCall("VK2ABC".to_string())
        );
    }
}
