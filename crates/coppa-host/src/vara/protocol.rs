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
    /// Request to listen for connections: LISTEN ON|OFF
    Listen(bool),
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
    /// Connected to remote station.
    Connected(String, String),
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
}

impl VaraCommand {
    /// Parse a VARA command string.
    pub fn parse(input: &str) -> Self {
        let trimmed = input.trim();
        let parts: Vec<&str> = trimmed.splitn(4, ' ').collect();

        match parts.first().map(|s| s.to_uppercase()).as_deref() {
            Some("MYCALL") => {
                if let Some(&call) = parts.get(1) {
                    VaraCommand::MyCall(call.to_string())
                } else {
                    VaraCommand::Unknown(trimmed.to_string())
                }
            }
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
            Some("LISTEN") => {
                let on = parts.get(1).map(|s| s.to_uppercase()) == Some("ON".to_string());
                VaraCommand::Listen(on)
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
    pub fn format(&self) -> String {
        match self {
            VaraResponse::Ok => "OK\r\n".to_string(),
            VaraResponse::Wrong => "WRONG\r\n".to_string(),
            VaraResponse::Version(v) => format!("VERSION {}\r\n", v),
            VaraResponse::Pending => "PENDING\r\n".to_string(),
            VaraResponse::Connected(src, dst) => format!("CONNECTED {} {}\r\n", src, dst),
            VaraResponse::Disconnected => "DISCONNECTED\r\n".to_string(),
            VaraResponse::Ptt(on) => format!("PTT {}\r\n", if *on { "ON" } else { "OFF" }),
            VaraResponse::Buffer(n) => format!("BUFFER {}\r\n", n),
            VaraResponse::Busy(b) => format!("BUSY {}\r\n", if *b { "ON" } else { "OFF" }),
            VaraResponse::Snr(snr) => format!("SNR {}\r\n", snr),
        }
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
        assert_eq!(VaraResponse::Ok.format(), "OK\r\n");
        assert_eq!(VaraResponse::Wrong.format(), "WRONG\r\n");
        assert_eq!(
            VaraResponse::Version("Coppa 0.1.0".to_string()).format(),
            "VERSION Coppa 0.1.0\r\n"
        );
        assert_eq!(
            VaraResponse::Connected("VK2ABC".to_string(), "VK3DEF".to_string()).format(),
            "CONNECTED VK2ABC VK3DEF\r\n"
        );
        assert_eq!(VaraResponse::Disconnected.format(), "DISCONNECTED\r\n");
        assert_eq!(VaraResponse::Ptt(true).format(), "PTT ON\r\n");
        assert_eq!(VaraResponse::Buffer(42).format(), "BUFFER 42\r\n");
        assert_eq!(VaraResponse::Busy(false).format(), "BUSY OFF\r\n");
        assert_eq!(VaraResponse::Snr(15).format(), "SNR 15\r\n");
    }

    #[test]
    fn test_case_insensitive() {
        assert_eq!(
            VaraCommand::parse("mycall VK2ABC"),
            VaraCommand::MyCall("VK2ABC".to_string())
        );
    }
}
