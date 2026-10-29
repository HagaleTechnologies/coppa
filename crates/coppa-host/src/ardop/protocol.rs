//! ARDOP command/response protocol parser and formatter.
//!
//! The vocabulary is fixed by Pat's verified client behaviour
//! (`wl2k-go/transport/ardop/tnc.go`): commands are acked by echoing them
//! back, a malformed command answers `WRONG`, a semantically failing one
//! `FAULT <text>`, and `NEWSTATE <state> ` keeps the trailing space real
//! TNCs send.

/// Commands received from an ARDOP client on the command port.
///
/// The split between a typed variant and [`ArdopCommand::Echo`] is data both
/// the daemon and `docs/ARDOP.md` consume: typed variants carry meaning
/// coppa acts on, `Echo` is the accepted-and-echoed periphery (verified
/// real-TNC commands no verified client sends), and `Unknown` is answered
/// `WRONG`.
#[derive(Debug, Clone, PartialEq)]
pub enum ArdopCommand {
    /// `INITIALIZE`: reset the TNC to defaults. Acked by echo.
    Initialize,
    /// `STATE`: ask for the current state word. Replied with the bare word.
    State,
    /// `CODEC TRUE|FALSE`: audio codec gate. Acked by echo; coppa models no
    /// codec gate.
    Codec(bool),
    /// `PROTOCOLMODE ARQ|FEC`. Acked by echo; coppa speaks ARQ only.
    ProtocolMode(String),
    /// `ARQTIMEOUT <s>`: how long the *TNC* waits. Recorded, not enforced
    /// (coppa's own COP-38 retry budget governs reality).
    ArqTimeout(u32),
    /// `LISTEN TRUE|FALSE`: answer inbound connects. Anything other than
    /// `TRUE`/`FALSE` parses as `Unknown`.
    Listen(bool),
    /// `MYCALL <call>`: set the station callsign (feeds the station-ID gate).
    MyCall(String),
    /// `GRIDSQUARE <grid>`: stored, unused by ARQ.
    GridSquare(String),
    /// `ARQBW <bw>`: bandwidth token, stored as a label only (coppa's speed
    /// ladder does not model ARDOP bandwidths).
    ArqBw(String),
    /// `VERSION`: ask for the version line.
    Version,
    /// `ARQCALL <target> [<repeats>]`: the dial. The server turns it into a
    /// `ConnectRequest`; the ack is the eventual outcome, never an echo.
    ArqCall {
        target: String,
        /// The repeat count, when the client sent a parseable one. Accepted,
        /// not used: COP-38 owns the retry budget.
        repeats: Option<u32>,
    },
    /// `DISCONNECT`: end the session. The server turns it into a
    /// `DisconnectRequest`.
    Disconnect,
    /// `CANCELPENDING`: drop a pending (not yet established) connect attempt.
    CancelPending,
    /// `CWID TRUE|FALSE`: Morse ID option. Echo-ack only; coppa keys no
    /// Morse ID.
    CwId(bool),
    /// `SENDID`: transmit a station-ID frame now.
    SendId,
    /// A known-adjacent real-TNC command no verified client sends
    /// (`SENDVER`, `ARQCALLBYNAME`, `GRACETIMERS`/`GRACETIMES`,
    /// `SENDBUFFER`, `FECSEND`, `MYAUX`/`MYAUXCALL`, `TRIMBLE`): accepted
    /// and echoed, no semantics emulated (Winlink Express is closed source;
    /// a spurious `WRONG` would break its init).
    Echo(String),
    /// Anything else: answered `WRONG`.
    Unknown(String),
}

impl ArdopCommand {
    /// Parse an ARDOP command line (case-insensitive, like VARA's parser).
    pub fn parse(input: &str) -> Self {
        let trimmed = input.trim();
        let mut parts = trimmed.split_whitespace();
        let verb = parts.next().map(|s| s.to_uppercase());
        let arg = parts.next();
        match verb.as_deref() {
            Some("INITIALIZE") => ArdopCommand::Initialize,
            Some("STATE") => ArdopCommand::State,
            Some("CODEC") => match arg.map(|s| s.to_uppercase()).as_deref() {
                Some("TRUE") => ArdopCommand::Codec(true),
                Some("FALSE") => ArdopCommand::Codec(false),
                _ => ArdopCommand::Unknown(trimmed.to_string()),
            },
            Some("PROTOCOLMODE") => match arg {
                Some(mode) => ArdopCommand::ProtocolMode(mode.to_string()),
                None => ArdopCommand::Unknown(trimmed.to_string()),
            },
            Some("ARQTIMEOUT") => match arg.and_then(|s| s.parse::<u32>().ok()) {
                Some(seconds) => ArdopCommand::ArqTimeout(seconds),
                None => ArdopCommand::Unknown(trimmed.to_string()),
            },
            Some("LISTEN") => match arg.map(|s| s.to_uppercase()).as_deref() {
                Some("TRUE") => ArdopCommand::Listen(true),
                Some("FALSE") => ArdopCommand::Listen(false),
                _ => ArdopCommand::Unknown(trimmed.to_string()),
            },
            Some("MYCALL") => match arg {
                Some(call) => ArdopCommand::MyCall(call.to_string()),
                None => ArdopCommand::Unknown(trimmed.to_string()),
            },
            Some("GRIDSQUARE") => match arg {
                Some(grid) => ArdopCommand::GridSquare(grid.to_string()),
                None => ArdopCommand::Unknown(trimmed.to_string()),
            },
            Some("ARQBW") => match arg {
                Some(bw) => ArdopCommand::ArqBw(bw.to_string()),
                None => ArdopCommand::Unknown(trimmed.to_string()),
            },
            Some("VERSION") => ArdopCommand::Version,
            Some("ARQCALL") => match arg {
                Some(target) => ArdopCommand::ArqCall {
                    target: target.to_string(),
                    repeats: parts.next().and_then(|r| r.parse::<u32>().ok()),
                },
                // Semantic arity failure: answered WRONG like an unknown verb.
                None => ArdopCommand::Unknown(trimmed.to_string()),
            },
            Some("DISCONNECT") => ArdopCommand::Disconnect,
            Some("CANCELPENDING") => ArdopCommand::CancelPending,
            Some("CWID") => match arg.map(|s| s.to_uppercase()).as_deref() {
                Some("TRUE") => ArdopCommand::CwId(true),
                Some("FALSE") => ArdopCommand::CwId(false),
                _ => ArdopCommand::Unknown(trimmed.to_string()),
            },
            Some("SENDID") => ArdopCommand::SendId,
            // The accepted-and-echoed periphery: real TNC commands a verified
            // client never sends, kept as raw strings.
            Some(
                "SENDVER" | "ARQCALLBYNAME" | "GRACETIMERS" | "GRACETIMES" | "SENDBUFFER"
                | "FECSEND" | "MYAUX" | "MYAUXCALL" | "TRIMBLE",
            ) => ArdopCommand::Echo(trimmed.to_string()),
            _ => ArdopCommand::Unknown(trimmed.to_string()),
        }
    }

    /// True for the accepted-and-echoed periphery: the daemon replies with
    /// the echo and emulates no semantics. This is the split `docs/ARDOP.md`
    /// prints as its second table column.
    pub fn is_echo_only(&self) -> bool {
        matches!(self, ArdopCommand::Echo(_))
    }
}

/// Responses sent to an ARDOP client on the command port.
#[derive(Debug, Clone, PartialEq)]
pub enum ArdopResponse {
    /// Echo the command back: the ARDOP ack for every accepted command.
    Echo(String),
    /// `FAULT <text>`: the command failed semantically.
    Fault(String),
    /// `WRONG`: unrecognised or unparseable command.
    Wrong,
    /// `NEWSTATE <state> ` — note the trailing space, preserved for
    /// byte-compatibility with real TNC output.
    NewState(String),
    /// `BUSY TRUE|FALSE` — TRUE/FALSE, not VARA's ON/OFF.
    Busy(bool),
    /// `PTT TRUE|FALSE`.
    Ptt(bool),
    /// `CONNECTED <callsign> <bandwidth>`: the dial succeeded. The
    /// bandwidth token echoes the client's stored `ARQBW` (default
    /// `500MAX`); it is a label, not a coppa speed level.
    Connected(String, String),
    /// `TARGET <callsign>`: a pending inbound ARQ request. Pat's listen
    /// mode requires it before `CONNECTED` to accept an inbound session.
    Target(String),
    /// `DISCONNECTED`: session over.
    Disconnected,
    /// `BUFFER <n>`: unacked host bytes (COP-30's definition).
    Buffer(usize),
    /// The `VERSION` line.
    Version(String),
}

impl ArdopResponse {
    /// Format the response as the wire line, ending in a bare `\r` (Pat
    /// splits what it reads on `\r`).
    pub fn format(&self) -> String {
        match self {
            ArdopResponse::Echo(line) => format!("{line}\r"),
            ArdopResponse::Fault(text) => format!("FAULT {text}\r"),
            ArdopResponse::Wrong => "WRONG\r".to_string(),
            // Real TNCs send the trailing space; Pat tolerates it, so keep
            // it for byte-compatibility.
            ArdopResponse::NewState(state) => format!("NEWSTATE {state} \r"),
            ArdopResponse::Busy(on) => format!("BUSY {}\r", if *on { "TRUE" } else { "FALSE" }),
            ArdopResponse::Ptt(on) => format!("PTT {}\r", if *on { "TRUE" } else { "FALSE" }),
            ArdopResponse::Connected(call, bw) => format!("CONNECTED {call} {bw}\r"),
            ArdopResponse::Target(call) => format!("TARGET {call}\r"),
            ArdopResponse::Disconnected => "DISCONNECTED\r".to_string(),
            ArdopResponse::Buffer(n) => format!("BUFFER {n}\r"),
            ArdopResponse::Version(v) => format!("VERSION {v}\r"),
        }
    }

    /// The `VERSION` line coppa sends in reply to a `VERSION` command:
    /// honest about what it is (Pat displays without parsing; impersonating
    /// an ARDOP version string would lie in operator logs).
    pub fn coppa_version() -> Self {
        ArdopResponse::Version(concat!("Coppa ", env!("CARGO_PKG_VERSION")).to_string())
    }

    /// True for the telemetry lines a client may miss: `BUSY`. The next
    /// change sends a fresh value; until one does, a client that missed a
    /// line holds the stale one. Every other line is a state change or a
    /// reply, and must reach the client or end its connection (the same
    /// policy as `VaraResponse::is_disposable`).
    pub fn is_disposable(&self) -> bool {
        matches!(self, ArdopResponse::Busy(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command of Pat's verified init/dial/close sequence parses to
    /// its typed variant (COP-97 §Design/2).
    #[test]
    fn parse_pat_init_sequence() {
        assert_eq!(ArdopCommand::parse("INITIALIZE"), ArdopCommand::Initialize);
        assert_eq!(ArdopCommand::parse("STATE"), ArdopCommand::State);
        assert_eq!(ArdopCommand::parse("CODEC TRUE"), ArdopCommand::Codec(true));
        assert_eq!(
            ArdopCommand::parse("PROTOCOLMODE ARQ"),
            ArdopCommand::ProtocolMode("ARQ".into())
        );
        assert_eq!(
            ArdopCommand::parse("ARQTIMEOUT 90"),
            ArdopCommand::ArqTimeout(90)
        );
        assert_eq!(
            ArdopCommand::parse("LISTEN FALSE"),
            ArdopCommand::Listen(false)
        );
        assert_eq!(
            ArdopCommand::parse("MYCALL N1CALL"),
            ArdopCommand::MyCall("N1CALL".into())
        );
        assert_eq!(
            ArdopCommand::parse("GRIDSQUARE FN31PR"),
            ArdopCommand::GridSquare("FN31PR".into())
        );
        assert_eq!(
            ArdopCommand::parse("ARQBW 500MAX"),
            ArdopCommand::ArqBw("500MAX".into())
        );
        assert_eq!(ArdopCommand::parse("VERSION"), ArdopCommand::Version);
        assert_eq!(
            ArdopCommand::parse("ARQCALL N0CALL 3"),
            ArdopCommand::ArqCall {
                target: "N0CALL".into(),
                repeats: Some(3)
            }
        );
        assert_eq!(ArdopCommand::parse("DISCONNECT"), ArdopCommand::Disconnect);
        assert_eq!(
            ArdopCommand::parse("CANCELPENDING"),
            ArdopCommand::CancelPending
        );
        assert_eq!(ArdopCommand::parse("CWID TRUE"), ArdopCommand::CwId(true));
        assert_eq!(ArdopCommand::parse("SENDID"), ArdopCommand::SendId);
    }

    /// Known-adjacent real-TNC commands parse to the echo-ack variant and
    /// are kept as raw strings.
    #[test]
    fn parse_accepts_and_echoes_adjacent_commands() {
        for line in [
            "SENDVER",
            "ARQCALLBYNAME X",
            "GRACETIMERS",
            "SENDBUFFER",
            "FECSEND",
            "MYAUXCALL N1CALL",
            "TRIMBLE 1",
        ] {
            let parsed = ArdopCommand::parse(line);
            assert_eq!(parsed, ArdopCommand::Echo(line.to_string()), "{line}");
            assert!(parsed.is_echo_only(), "{line}");
        }
    }

    #[test]
    fn parse_unknown_is_wrong() {
        assert_eq!(
            ArdopCommand::parse("FOOBAR"),
            ArdopCommand::Unknown("FOOBAR".into())
        );
        // Semantic arity failure is Unknown (answered WRONG), not a typed
        // half-command.
        assert_eq!(
            ArdopCommand::parse("ARQCALL"),
            ArdopCommand::Unknown("ARQCALL".into())
        );
        // Lowercase input accepted (VARA precedent).
        assert_eq!(
            ArdopCommand::parse("mycall n1call"),
            ArdopCommand::MyCall("n1call".into())
        );
    }

    /// The exact bytes of every reply Pat's parsers read literally.
    #[test]
    fn format_replies_exact_bytes() {
        assert_eq!(
            ArdopResponse::Echo("ARQTIMEOUT 90".into()).format(),
            "ARQTIMEOUT 90\r"
        );
        assert_eq!(ArdopResponse::Fault("busy".into()).format(), "FAULT busy\r");
        assert_eq!(ArdopResponse::Wrong.format(), "WRONG\r");
        assert_eq!(
            ArdopResponse::NewState("DISCONNECTED".into()).format(),
            "NEWSTATE DISCONNECTED \r"
        );
        assert_eq!(ArdopResponse::Busy(true).format(), "BUSY TRUE\r");
        assert_eq!(ArdopResponse::Ptt(false).format(), "PTT FALSE\r");
        assert_eq!(
            ArdopResponse::Connected("N0CALL".into(), "500MAX".into()).format(),
            "CONNECTED N0CALL 500MAX\r"
        );
        assert_eq!(
            ArdopResponse::Target("N0CALL".into()).format(),
            "TARGET N0CALL\r"
        );
        assert_eq!(ArdopResponse::Disconnected.format(), "DISCONNECTED\r");
        assert_eq!(ArdopResponse::Buffer(128).format(), "BUFFER 128\r");
    }

    #[test]
    fn version_is_honest() {
        assert_eq!(
            ArdopResponse::coppa_version().format(),
            format!("VERSION Coppa {}\r", env!("CARGO_PKG_VERSION"))
        );
    }

    /// `BUSY` may be dropped for a client falling behind; every state line
    /// and reply may not (mirrors `VaraResponse::is_disposable`).
    #[test]
    fn disposable_telemetry_matches_vara_policy() {
        assert!(ArdopResponse::Busy(true).is_disposable());
        assert!(ArdopResponse::Busy(false).is_disposable());
        for line in [
            ArdopResponse::NewState("CONNECTED".into()),
            ArdopResponse::Connected("N0CALL".into(), "500MAX".into()),
            ArdopResponse::Disconnected,
            ArdopResponse::Target("N1CALL".into()),
            ArdopResponse::Fault("busy".into()),
            ArdopResponse::Echo("STATE".into()),
            ArdopResponse::Wrong,
            ArdopResponse::Ptt(true),
            ArdopResponse::Buffer(0),
            ArdopResponse::coppa_version(),
        ] {
            assert!(!line.is_disposable(), "{line:?}");
        }
    }
}
