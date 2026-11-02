//! COP-83: the operator dashboard `coppad` serves at `GET /` on its WebSocket
//! port (`WebSocketServer::with_index_html`). One static file, embedded at
//! build time. The tests below pin the page's contract with the daemon and
//! its reviewed on-screen wording; they live in the library so `ci.yml`'s
//! `--lib` run executes them.

/// The dashboard page, served verbatim.
pub const INDEX_HTML: &str = include_str!("index.html");

#[cfg(test)]
mod tests {
    use super::INDEX_HTML;

    /// Every request type the page may send. Each must be a request the
    /// protocol schema knows.
    const REQUEST_TYPES: &[&str] = &[
        "mycall",
        "connect",
        "disconnect",
        "send",
        "status",
        "spectrum",
        "audio_level",
        "log",
        "tune",
        "set_speed_level",
    ];

    const WORDING: &[&str] = &[
        "Coppa operator dashboard",
        "Access token:",
        "Not connected",
        "Connecting…",
        "Refused or unreachable: check the token, and that [host] websocket_allowed_origins lists",
        "Connection lost; reconnecting in",
        "this page expects protocol version 1",
        "Daemon:",
        "running",
        "waiting for a clear channel to transmit",
        "event loop stalled for",
        "not answering for",
        "MYCALL not set",
        "PTT:",
        "s limit. Check the radio and the PTT line.",
        "Channel:",
        "No spectrum received yet. The daemon sends one only while it receives audio.",
        "RX level",
        "waiting for audio",
        "paused while transmitting",
        "Last TX level:",
        "No frame decoded yet",
        "last 2 min",
        "(last decoded frame)",
        "Speed level",
        "Auto",
        "manual",
        "auto",
        "Buffer",
        "bytes waiting to be sent or acknowledged",
        "TUNE",
        "Seconds",
        "Two-tone",
        "Single tone 1500 Hz",
        "Unavailable while transmitting",
        "Session",
        "No session",
        "Connected to",
        "In flight",
        "retries",
        "RTO",
        "Fade hold: retransmitting at a lower level",
        "Heard",
        "No stations heard yet. Stations appear here after a session connects.",
        "Log",
        "No log lines yet.",
        "RX data:",
        "MYCALL",
        "Call station",
        "Call",
        "Disconnect",
        "Send",
        "Connect a session to send",
        ": accepted",
        ": refused: ",
    ];

    #[test]
    fn page_keeps_the_cop79_contract() {
        assert!(INDEX_HTML.contains("<title>Coppa operator dashboard</title>"));
        assert!(INDEX_HTML.contains("/ws?token="));
        assert!(!INDEX_HTML.contains("WS_TOKEN"));
        assert!(!INDEX_HTML.contains("websocket_token ="));
    }

    #[test]
    fn page_never_writes_html_from_data() {
        for forbidden in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "eval(",
            "new Function",
        ] {
            assert!(
                !INDEX_HTML.contains(forbidden),
                "page must not use {forbidden}"
            );
        }
    }

    #[test]
    fn page_makes_no_external_requests() {
        for forbidden in [
            "http://",
            "https://",
            "<script src",
            "<link",
            "@import",
            "fetch(",
            "XMLHttpRequest",
            "importScripts",
        ] {
            assert!(
                !INDEX_HTML.contains(forbidden),
                "page must not contain {forbidden}"
            );
        }
    }

    /// Collect every string under a `"const"` key or in an `"enum"` array.
    fn schema_literals(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    match (key.as_str(), child) {
                        ("const", serde_json::Value::String(s)) => out.push(s.clone()),
                        ("enum", serde_json::Value::Array(items)) => out.extend(
                            items
                                .iter()
                                .filter_map(serde_json::Value::as_str)
                                .map(str::to_owned),
                        ),
                        _ => schema_literals(child, out),
                    }
                }
            }
            serde_json::Value::Array(items) => {
                for child in items {
                    schema_literals(child, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn page_sends_every_request_through_one_function() {
        assert_eq!(
            INDEX_HTML.matches(".send(").count(),
            1,
            "only request() may call .send("
        );
        for request_type in REQUEST_TYPES {
            let call = format!("request(\"{request_type}\"");
            assert!(INDEX_HTML.contains(&call), "page never sends {call}");
        }

        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/schema/websocket-protocol-v1.json"
        ))
        .expect("schema parses");
        let mut literals = Vec::new();
        schema_literals(&schema, &mut literals);
        for request_type in REQUEST_TYPES {
            assert!(
                literals.iter().any(|l| l == request_type),
                "schema has no {request_type:?} message type"
            );
        }
    }

    /// The code of the page's top-level `function {name}(`, up to the next
    /// top-level function, without `//` comment lines.
    fn page_function(name: &str) -> String {
        let start = INDEX_HTML
            .find(&format!("\nfunction {name}("))
            .unwrap_or_else(|| panic!("page defines {name}"));
        let rest = &INDEX_HTML[start + 1..];
        let end = rest
            .find("\nfunction ")
            .unwrap_or_else(|| panic!("a function follows {name}"));
        rest[..end]
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A reconnect can miss `session` pushes. `applyStatus` must compare the
    /// status reply's `remote_call` with the cached peer, not only the
    /// `connected` flag, and a socket loss must forget what the page cannot
    /// know afterwards: when the session started and whether a call is
    /// still being placed.
    #[test]
    fn page_reconciles_the_session_from_status_after_a_reconnect() {
        let apply_status = page_function("applyStatus");
        assert!(
            apply_status.contains("call !== state.session.call)"),
            "applyStatus never compares remote_call with the cached session peer"
        );
        let reset_live = page_function("resetLive");
        for line in ["state.session.since = 0;", "state.session.calling = null;"] {
            assert!(reset_live.contains(line), "resetLive lacks {line:?}");
        }
    }

    #[test]
    fn page_shows_the_reviewed_wording() {
        for text in WORDING {
            assert!(INDEX_HTML.contains(text), "page lacks {text:?}");
        }
    }
}
