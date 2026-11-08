//! COP-122: the operator dashboard at 390 CSS px, in a real browser.
//!
//! Serves `coppa_daemon::web::INDEX_HTML`, the exact bytes `coppad` serves at
//! `GET /`, from a loopback HTTP listener. Drives headless Chrome over the
//! DevTools protocol at 390 x 844 with mobile and touch emulation, and checks
//! every control the page renders: at least 44 x 44 CSS px, inside the
//! viewport, and hit by a tap at its centre. The page's own `handle()` plays
//! the daemon's `hello` and `status` messages, so no daemon and no socket are
//! needed.
//!
//! With no Chrome or Chromium found, the test prints why and passes, unless
//! `COPPA_REQUIRE_BROWSER=1` is set (`ci-full.yml`'s `test-full` job sets
//! it). `COPPA_CHROME` names the browser binary when it is not on `PATH`.

#![cfg(feature = "websocket")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use coppa_daemon::web::INDEX_HTML;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

/// The phone width from the ticket, in CSS px.
const WIDTH: f64 = 390.0;
/// A phone-shaped viewport height, in CSS px.
const HEIGHT: f64 = 844.0;
/// The smallest touch target the page may render, in CSS px (WCAG 2.5.5,
/// Apple's guidance, and the ticket's figure).
const MIN_TARGET: f64 = 44.0;

/// How long the browser may take to print its DevTools URL.
const STARTUP: Duration = Duration::from_secs(30);
/// How long any one DevTools command or awaited event may take.
const STEP: Duration = Duration::from_secs(10);

/// Browser names searched on `PATH` when `COPPA_CHROME` is not set.
const BROWSERS: [&str; 4] = [
    "chromium",
    "chromium-browser",
    "google-chrome",
    "google-chrome-stable",
];

/// One measured page state: what it is, and how many controls it renders.
struct Expected {
    label: &'static str,
    controls: usize,
    token_form_hidden: bool,
}

/// The states `MEASURE` returns, in order. Before the first `status` the
/// page shows the token form (input and Connect), `Auto`, TUNE (Seconds,
/// Two-tone, Single tone) and the command box (MYCALL, Set, Call station,
/// Call, Disconnect, Send input, Send). `hello` hides the token form and
/// each `status` adds one button per speed level: 8 on HF, 9 on VHF
/// (`coppa-engine`'s `engine.rs`).
const STATES: [Expected; 3] = [
    Expected {
        label: "before the first status",
        controls: 13,
        token_form_hidden: false,
    },
    Expected {
        label: "after hello and an HF status (levels 1 to 8)",
        controls: 19,
        token_form_hidden: true,
    },
    Expected {
        label: "after a VHF status (levels 1 to 9)",
        controls: 20,
        token_form_hidden: true,
    },
];

/// Measures every rendered control, plays `hello` and an 8-level `status`
/// through the page's own `handle()`, measures again, plays a 9-level
/// `status`, and measures a third time. A control that renders no box
/// (`display: none`, or inside a hidden parent) is not on screen and is
/// skipped; a disabled one counts.
const MEASURE: &str = r##"(() => {
  const CONTROLS = "button, input, select, textarea, a[href], [role=button], [tabindex]";
  const name = (el) => el.id ? "#" + el.id
    : el.dataset && el.dataset.level ? "level " + el.dataset.level
    : el.tagName.toLowerCase() + " \"" + el.textContent.trim() + "\"";
  const measure = () => {
    const controls = [];
    for (const el of document.querySelectorAll(CONTROLS)) {
      const box = el.getBoundingClientRect();
      if (box.width === 0 && box.height === 0) continue;
      el.scrollIntoView({ block: "center", inline: "nearest" });
      const r = el.getBoundingClientRect();
      const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      controls.push({
        name: name(el),
        width: r.width,
        height: r.height,
        left: r.left,
        right: r.right,
        hit: hit !== null && (hit === el || el.contains(hit)),
        hitName: hit === null ? "nothing" : hit.id ? "#" + hit.id : hit.tagName.toLowerCase(),
      });
    }
    window.scrollTo(0, 0);
    return {
      innerWidth: window.innerWidth,
      scrollWidth: document.documentElement.scrollWidth,
      tokenFormDisplay: getComputedStyle(document.getElementById("token-form")).display,
      controls: controls,
    };
  };
  const levels = (n) => Array.from({ length: n }, (_, i) => i + 1);
  const states = [measure()];
  handle({ type: "hello", version: 1 });
  handle({ type: "status", speed_levels: levels(8) });
  states.push(measure());
  handle({ type: "status", speed_levels: levels(9) });
  states.push(measure());
  return states;
})()"##;

/// Scrolls speed level 3 into view and returns its centre.
const LEVEL_3_CENTRE: &str = r##"(() => {
  const button = document.querySelector('#level-buttons button[data-level="3"]');
  if (button === null) return null;
  button.scrollIntoView({ block: "center", inline: "nearest" });
  const r = button.getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
})()"##;

/// What a tap on level 3 shows with no daemon behind the page.
const TAP_RESULT: &str = "Speed level 3: refused: not connected";

#[tokio::test]
async fn every_dashboard_control_is_a_44px_touch_target_at_390px() {
    let Some(binary) = find_browser() else {
        let required = std::env::var("COPPA_REQUIRE_BROWSER").is_ok_and(|value| value == "1");
        assert!(
            !required,
            "COPPA_REQUIRE_BROWSER=1 but no browser was found: set COPPA_CHROME to a Chrome or \
             Chromium binary, or put one of {BROWSERS:?} on PATH"
        );
        eprintln!(
            "skipping dashboard_touch: no Chrome or Chromium found (set COPPA_CHROME to a browser \
             binary, or COPPA_REQUIRE_BROWSER=1 to fail instead of skipping)"
        );
        return;
    };

    let url = serve_page();
    let (_browser, devtools_url) = Browser::launch(&binary);
    let mut cdp = Cdp::connect(&devtools_url).await;

    let target = cdp
        .call(None, "Target.createTarget", json!({ "url": "about:blank" }))
        .await;
    let target_id = target["targetId"]
        .as_str()
        .expect("Target.createTarget returns a targetId")
        .to_string();
    let attached = cdp
        .call(
            None,
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
        )
        .await;
    let session = attached["sessionId"]
        .as_str()
        .expect("Target.attachToTarget returns a sessionId")
        .to_string();
    let session = Some(session.as_str());

    cdp.call(
        session,
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": WIDTH, "height": HEIGHT, "deviceScaleFactor": 3, "mobile": true }),
    )
    .await;
    cdp.call(
        session,
        "Emulation.setTouchEmulationEnabled",
        json!({ "enabled": true, "maxTouchPoints": 5 }),
    )
    .await;
    cdp.call(session, "Page.enable", json!({})).await;
    cdp.events.clear();
    let navigated = cdp
        .call(session, "Page.navigate", json!({ "url": url }))
        .await;
    assert!(
        navigated.get("errorText").is_none(),
        "Page.navigate to {url} failed: {navigated}"
    );
    cdp.wait_event(session, "Page.loadEventFired").await;
    let ready = format!(
        "document.readyState === \"complete\" && location.href === {} && typeof handle === \"function\"",
        json!(url)
    );
    cdp.wait_until(session, &ready).await;

    let measured = cdp.evaluate(session, MEASURE).await;
    let states = measured
        .as_array()
        .expect("the measuring script returns an array");
    assert_eq!(
        states.len(),
        STATES.len(),
        "one measurement per state: {measured}"
    );

    let mut problems = Vec::new();
    for (expected, state) in STATES.iter().zip(states) {
        check_state(expected, state, &mut problems);
    }
    tap_level_3(&mut cdp, session, &mut problems).await;

    assert!(
        problems.is_empty(),
        "the dashboard at {WIDTH} px fails COP-122's touch bar:\n  {}",
        problems.join("\n  ")
    );
}

/// Adds one line to `problems` for every way `state` misses the bar.
fn check_state(expected: &Expected, state: &Value, problems: &mut Vec<String>) {
    let label = expected.label;
    let inner_width = number(state, "innerWidth");
    if inner_width != WIDTH {
        problems.push(format!(
            "{label}: innerWidth is {inner_width}, expected {WIDTH}"
        ));
    }
    let scroll_width = number(state, "scrollWidth");
    if scroll_width > WIDTH {
        problems.push(format!(
            "{label}: the page is {scroll_width} px wide, so it scrolls sideways at {WIDTH} px"
        ));
    }

    let controls = state["controls"]
        .as_array()
        .expect("every state lists its controls");
    if controls.len() != expected.controls {
        problems.push(format!(
            "{label}: {} controls rendered, expected {}. If a control was added or removed on \
             purpose, update the count in STATES and keep every new control at least {MIN_TARGET} \
             x {MIN_TARGET} px",
            controls.len(),
            expected.controls
        ));
    }
    for control in controls {
        let name = control["name"].as_str().unwrap_or("an unnamed control");
        let width = number(control, "width");
        let height = number(control, "height");
        if width < MIN_TARGET || height < MIN_TARGET {
            problems.push(format!(
                "{label}: {name} is {width:.1} x {height:.1} px, below {MIN_TARGET} x {MIN_TARGET}"
            ));
        }
        let left = number(control, "left");
        let right = number(control, "right");
        if left < 0.0 || right > WIDTH {
            problems.push(format!(
                "{label}: {name} spans {left:.1} to {right:.1} px, outside the {WIDTH} px viewport"
            ));
        }
        if control["hit"] != json!(true) {
            problems.push(format!(
                "{label}: a tap at the centre of {name} lands on {}",
                control["hitName"]
            ));
        }
    }

    let token_form = state["tokenFormDisplay"].as_str().unwrap_or("");
    if expected.token_form_hidden && token_form != "none" {
        problems.push(format!(
            "{label}: the token form is still shown (display: {token_form}) after hello hid it"
        ));
    }
}

/// Taps speed level 3 with a touch event and checks the page's answer.
async fn tap_level_3(cdp: &mut Cdp, session: Option<&str>, problems: &mut Vec<String>) {
    let centre = cdp.evaluate(session, LEVEL_3_CENTRE).await;
    if centre.is_null() {
        problems.push("no speed level 3 button to tap".to_string());
        return;
    }
    let x = number(&centre, "x");
    let y = number(&centre, "y");
    cdp.call(
        session,
        "Input.dispatchTouchEvent",
        json!({ "type": "touchStart", "touchPoints": [{ "x": x, "y": y }] }),
    )
    .await;
    cdp.call(
        session,
        "Input.dispatchTouchEvent",
        json!({ "type": "touchEnd", "touchPoints": [] }),
    )
    .await;

    let read = "document.getElementById(\"level-result\").textContent";
    let expected = json!(TAP_RESULT);
    let shown = timeout(STEP, async {
        loop {
            let text = cdp.evaluate(session, read).await;
            if text == expected {
                return text;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    if shown.is_err() {
        let text = cdp.evaluate(session, read).await;
        problems.push(format!(
            "a touch tap on speed level 3 left #level-result reading {text}, expected \
             {expected} within {STEP:?}"
        ));
    }
}

fn number(value: &Value, key: &str) -> f64 {
    value[key]
        .as_f64()
        .unwrap_or_else(|| panic!("{key} is a number in {value}"))
}

/// `COPPA_CHROME` if set, else the first of `BROWSERS` found on `PATH`.
fn find_browser() -> Option<PathBuf> {
    let configured = std::env::var_os("COPPA_CHROME").unwrap_or_default();
    if !configured.is_empty() {
        return Some(PathBuf::from(configured));
    }
    let path = std::env::var_os("PATH")?;
    for name in BROWSERS {
        let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(&file);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Serves `INDEX_HTML` for every request on a loopback port; returns its URL.
fn serve_page() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
    let address = listener.local_addr().expect("the listener's address");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            // One thread per connection: a speculative connection that never
            // sends a request must not hold up the real one.
            std::thread::spawn(move || {
                let _ = answer(stream);
            });
        }
    });
    format!("http://{address}/")
}

fn answer(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(STEP))?;
    let mut head = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") && head.len() < 64 * 1024 {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Ok(());
        }
        head.extend_from_slice(&chunk[..read]);
    }
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        INDEX_HTML.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.write_all(INDEX_HTML.as_bytes())?;
    stream.flush()
}

/// A headless browser child and its profile directory; dropping it kills
/// the browser and removes the directory.
struct Browser {
    child: Child,
    profile: PathBuf,
}

impl Browser {
    /// Starts `binary` and returns it with its browser-level DevTools URL.
    fn launch(binary: &Path) -> (Self, String) {
        let profile =
            std::env::temp_dir().join(format!("coppa-dashboard-touch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::create_dir_all(&profile).expect("create the browser profile directory");
        let mut child = Command::new(binary)
            .args([
                "--headless=new",
                "--no-sandbox",
                "--disable-gpu",
                "--remote-debugging-port=0",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("start {}: {error}", binary.display()));
        let stderr = child.stderr.take().expect("the browser's stderr is piped");
        let browser = Self { child, profile };

        // Read stderr on a thread so the wait is bounded, and keep draining
        // it afterwards so a full pipe never stalls the browser.
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut sent = false;
            let mut before = Vec::new();
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if sent {
                    continue;
                }
                match line.strip_prefix("DevTools listening on ") {
                    Some(url) => {
                        let _ = sender.send(Ok(url.trim().to_string()));
                        sent = true;
                    }
                    None => before.push(line),
                }
            }
            if !sent {
                let _ = sender.send(Err(before.join("\n")));
            }
        });
        let url = match receiver.recv_timeout(STARTUP) {
            Ok(Ok(url)) => url,
            Ok(Err(output)) => panic!(
                "{} exited before DevTools listened:\n{output}",
                binary.display()
            ),
            Err(_) => panic!(
                "{} printed no DevTools URL within {STARTUP:?}",
                binary.display()
            ),
        };
        (browser, url)
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A DevTools protocol client: JSON commands with increasing ids, and the
/// events that arrive while it waits for a reply.
struct Cdp {
    socket: Socket,
    next_id: u64,
    events: Vec<Value>,
}

impl Cdp {
    async fn connect(url: &str) -> Self {
        let (socket, _) = timeout(STEP, tokio_tungstenite::connect_async(url))
            .await
            .unwrap_or_else(|_| panic!("connect to {url}: no answer within {STEP:?}"))
            .unwrap_or_else(|error| panic!("connect to {url}: {error}"));
        Self {
            socket,
            next_id: 0,
            events: Vec::new(),
        }
    }

    /// Sends one command and returns its `result`; a protocol error panics.
    async fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let mut command = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            command["sessionId"] = json!(session);
        }
        timeout(STEP, self.socket.send(Message::text(command.to_string())))
            .await
            .unwrap_or_else(|_| panic!("{method}: not sent within {STEP:?}"))
            .unwrap_or_else(|error| panic!("{method}: {error}"));
        let reply = timeout(STEP, async {
            loop {
                let message = self.read().await;
                if message["id"] == json!(id) {
                    return message;
                }
                self.events.push(message);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{method}: no reply within {STEP:?}"));
        let error = reply.get("error");
        assert!(error.is_none(), "{method} failed: {reply}");
        reply["result"].clone()
    }

    /// Waits for the event `method` on `session`, including one that arrived
    /// while an earlier command waited for its reply.
    async fn wait_event(&mut self, session: Option<&str>, method: &str) {
        let wanted = |event: &Value| {
            event["method"] == json!(method) && event["sessionId"].as_str() == session
        };
        let seen = self.events.iter().position(wanted);
        if let Some(index) = seen {
            self.events.remove(index);
            return;
        }
        timeout(STEP, async {
            loop {
                let message = self.read().await;
                if wanted(&message) {
                    return;
                }
                self.events.push(message);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("no {method} within {STEP:?}"));
    }

    /// Evaluates `expression` in the page and returns its value as JSON.
    async fn evaluate(&mut self, session: Option<&str>, expression: &str) -> Value {
        let result = self
            .call(
                session,
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true }),
            )
            .await;
        let exception = result.get("exceptionDetails");
        assert!(
            exception.is_none(),
            "the page threw evaluating {expression}: {result}"
        );
        result["result"]["value"].clone()
    }

    /// Polls `expression` until it is `true`.
    async fn wait_until(&mut self, session: Option<&str>, expression: &str) {
        timeout(STEP, async {
            loop {
                if self.evaluate(session, expression).await == json!(true) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{expression} not true within {STEP:?}"));
    }

    async fn read(&mut self) -> Value {
        loop {
            let message = self
                .socket
                .next()
                .await
                .expect("the DevTools socket stays open")
                .expect("a DevTools message");
            if message.is_text() {
                let text = message.to_text().expect("a text message");
                return serde_json::from_str(text).expect("DevTools sends JSON");
            }
        }
    }
}
