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
        "chat",
        "chat_history",
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
        "No stations heard yet. Stations appear here after a session connects or a beacon is received.",
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
        // COP-121: the tab bar and the Chat tab.
        "Station",
        "Chat",
        "Chatting with",
        "Not connected to coppad.",
        "Loading messages…",
        "No messages yet. Messages you send and receive appear here.",
        "No session. Call a station on the Station tab to chat.",
        "'s coppad does not accept chat: it needs coppad 0.11.0 or later with its WebSocket API enabled.",
        "Connect a session to chat",
        "Type a message",
        "Message",
        " of 512 bytes",
        "Too long: ",
        "Sending…",
        "Delivered",
        "Not delivered: ",
        "Not sent: ",
        "To ",
        "From ",
        "Delivered means the other station's coppad received the message. It does not mean someone read it.",
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

    /// `applyStatus` takes `status.snr` only while the page's reading has no
    /// arrival time, so a socket loss must drop that time: a frame decoded
    /// while the socket was down is known only from `status.snr`.
    #[test]
    fn page_reconciles_the_snr_from_status_after_a_reconnect() {
        let apply_status = page_function("applyStatus");
        for line in [
            "if (!state.lastSnrAt) {",
            "if (typeof msg.snr === \"number\") state.lastSnr = msg.snr;",
        ] {
            assert!(apply_status.contains(line), "applyStatus lacks {line:?}");
        }
        let reset_live = page_function("resetLive");
        assert!(
            reset_live.contains("state.lastSnrAt = 0;"),
            "resetLive keeps the S/N arrival time across a socket loss"
        );
    }

<<<<<<< HEAD
    /// COP-122: every control is a 44 px touch target, and `hidden` hides
    /// the token form even though `.row` sets `display: flex`.
    /// `tests/dashboard_touch.rs` measures the rendered page in a real
    /// browser; this guard runs in `ci.yml`'s `--lib` job, where no browser
    /// does, so a deleted rule still fails CI.
    #[test]
    fn page_keeps_touch_sized_controls() {
        for rule in [
            "[hidden] { display: none !important; }",
            "button, input { min-height: 44px; }",
            "button { min-width: 44px;",
        ] {
            assert!(INDEX_HTML.contains(rule), "page stylesheet lacks {rule:?}");
        }
=======
    /// COP-121: every `hello` (first connect and every reconnect) asks for
    /// the chat history, which carries every receipt the page missed.
    #[test]
    fn page_reloads_chat_history_on_every_hello() {
        assert!(
            page_function("onHello").contains("request(\"chat_history\""),
            "onHello never requests chat_history"
        );
    }

    /// A `chat_history` reply replaces the page's messages; `chat` pushes
    /// upsert by `message_id` (D12).
    #[test]
    fn page_replaces_chat_from_history_and_upserts_pushes() {
        let history = page_function("applyChatHistory");
        let clear = history
            .find("state.chat.clear();")
            .expect("applyChatHistory clears the map");
        let fill = history
            .find("state.chat.set(")
            .expect("applyChatHistory fills the map");
        assert!(clear < fill, "applyChatHistory fills before it clears");

        let push = page_function("applyChat");
        assert!(
            push.contains("state.chat.set(msg.message_id"),
            "applyChat does not upsert by message_id"
        );
        assert!(!push.contains(".clear()"), "applyChat clears the map");
    }

    #[test]
    fn page_applies_receipts_by_message_id() {
        assert!(
            page_function("applyChatReceipt").contains("state.chat.get(msg.message_id)"),
            "applyChatReceipt does not look the message up by message_id"
        );
    }

    /// The limit is 512 UTF-8 bytes, not 512 characters.
    #[test]
    fn page_counts_chat_in_utf8_bytes() {
        assert!(INDEX_HTML.contains("new TextEncoder().encode("));
        assert!(
            page_function("renderChat").contains("new TextEncoder().encode(input.value).length")
        );
    }

    /// `main { display: grid; }` overrides the browser's `[hidden]` rule, so
    /// the tab panels need their own rule.
    #[test]
    fn page_hides_tab_panels_with_a_scoped_rule() {
        let style = &INDEX_HTML[..INDEX_HTML.find("</style>").expect("page has a stylesheet")];
        assert!(style.contains("[role=\"tabpanel\"][hidden] { display: none; }"));
        let tabs: Vec<&str> = INDEX_HTML
            .match_indices("role=\"tab\"")
            .map(|(at, _)| {
                let open = INDEX_HTML[..at].rfind('<').expect("tag start");
                let close = at + INDEX_HTML[at..].find('>').expect("tag end");
                &INDEX_HTML[open..=close]
            })
            .collect();
        assert_eq!(tabs.len(), 2, "page must have exactly two tabs");
        for tag in tabs {
            assert!(tag.starts_with("<button"), "tab is not a button: {tag}");
            assert!(
                tag.contains("aria-selected="),
                "tab lacks aria-selected: {tag}"
            );
        }
        let body = &INDEX_HTML[INDEX_HTML.find("</style>").expect("stylesheet ends")..];
        assert_eq!(body.matches("role=\"tabpanel\"").count(), 2);
    }

    /// A socket loss keeps the messages on screen and waits for the next
    /// `chat_history` instead of clearing them.
    #[test]
    fn page_keeps_chat_across_a_socket_loss() {
        let reset_live = page_function("resetLive");
        assert!(reset_live.contains("state.chatLoaded = false;"));
        assert!(!reset_live.contains("state.chat.clear()"));
        assert!(!reset_live.contains("state.chat = "));
>>>>>>> 2f002b5d107c59af91bb1c3947ab24e75762c3b6
    }

    #[test]
    fn page_shows_the_reviewed_wording() {
        for text in WORDING {
            assert!(INDEX_HTML.contains(text), "page lacks {text:?}");
        }
    }
}
