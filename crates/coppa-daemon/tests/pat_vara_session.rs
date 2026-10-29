//! COP-46: Pat-Vara v1.2.0's exact VARA wire sequence, end to end, against
//! two real `EventLoop` stations.
//!
//! Each station (N1CALL, N0CALL) is a real `EventLoop` running `run()`, wired
//! to its VARA server by the same `coppa_daemon::vara_wiring::serve_vara`
//! that `coppad`'s `main` uses, on OS-assigned `127.0.0.1:0` ports. The two
//! stations' audio rings are joined by a half-duplex virtual cable: every
//! 20 ms it moves 960 samples (48 kHz real time) each way, and a station that
//! is keyed hears silence, as a real half-duplex radio is deaf while it
//! transmits. A small `PatVara` emulator per station then drives the command
//! and data ports with the command sequence read from Pat-Vara v1.2.0's
//! source (`start()`'s burst, `LISTEN ON`, `WINLINK SESSION` + `CONNECT`,
//! flush on `BUFFER 0`, and `Close`'s 2 s wait, `DISCONNECT`, wait for
//! `DISCONNECTED`). The expected replies are the ones real Pat v1.0.0 saw
//! from `coppad` in the COP-46 two-process run.
//!
//! Limitation, stated plainly: the test runs in tokio virtual time
//! (`start_paused`), and virtual time does not advance while a station
//! decodes synchronously. It therefore **cannot** catch a real-time decode
//! stall such as a debug build overflowing its input ring (COP-46's
//! debug-build failure). `tools/pat-loop/run.sh`, which runs real Pat against
//! two real release `coppad` processes, covers that.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use coppa_audio::{AudioRingConsumer, AudioRingProducer};
use coppa_daemon::config::DaemonConfig;
use coppa_daemon::event_loop::{forward_data_out, EventLoop};
use coppa_daemon::vara_wiring::serve_vara;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
const FLUSH_TIMEOUT: Duration = Duration::from_secs(300);
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(60);
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// One cable tick: 20 ms of 48 kHz audio.
const CABLE_TICK: Duration = Duration::from_millis(20);
const CABLE_SAMPLES: usize = 960;

/// Unsolicited command-port lines Pat handles out of band; never a reply.
fn is_telemetry(line: &str) -> bool {
    ["BUSY ", "SNR ", "BUFFER ", "PTT ", "IAMALIVE"]
        .iter()
        .any(|t| line.starts_with(t))
}

/// A running station: its ports, and its keyed state as seen on the command
/// port by an always-reading monitor client.
struct Station {
    call: &'static str,
    cmd_addr: std::net::SocketAddr,
    data_addr: std::net::SocketAddr,
    keyed: Arc<AtomicBool>,
}

/// A station's audio ring ends, for the cable: what it hears, what it sends.
struct StationAudio {
    input: AudioRingProducer,
    output: AudioRingConsumer,
}

/// Build, wire and start one station inside the current `LocalSet`.
async fn station(call: &'static str) -> (Station, StationAudio) {
    let cmd_listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let data_listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let cmd_addr = cmd_listener.local_addr().unwrap();
    let data_addr = data_listener.local_addr().unwrap();

    let mut config = DaemonConfig::default();
    config.engine.callsign = call.into();
    config.engine.profile = "HF_STANDARD".into();
    config.host.vara_enabled = true;
    config.host.vara_command_port = cmd_addr.port();
    config.host.vara_data_port = data_addr.port();
    let host = config.host.clone();

    let (in_producer, in_consumer) = coppa_audio::audio_ring(config.audio.buffer_size);
    let (out_producer, out_consumer) = coppa_audio::audio_ring(config.audio.buffer_size);

    let mut event_loop = EventLoop::new(config).unwrap();
    let (response_tx, response_rx) = tokio::sync::mpsc::unbounded_channel();
    event_loop.set_response_tx(response_tx);
    event_loop.set_audio_in(in_consumer);
    event_loop.set_audio_out(out_producer);
    let data_senders = serve_vara(&mut event_loop, cmd_listener, data_listener, &host);
    tokio::task::spawn_local(async move {
        let _ = forward_data_out(response_rx, vec![data_senders]).await;
    });
    tokio::task::spawn_local(async move {
        let _ = event_loop.run().await;
    });

    // PTT monitor: a second command client that reads every line, so the
    // server never evicts it as slow (COP-34), and tracks the keyed state.
    let keyed = Arc::new(AtomicBool::new(false));
    let monitor = TcpStream::connect(cmd_addr).await.unwrap();
    let flag = keyed.clone();
    tokio::task::spawn_local(async move {
        let (read_half, _write_half) = monitor.into_split();
        let mut reader = BufReader::new(read_half);
        loop {
            let mut line = Vec::new();
            match reader.read_until(b'\r', &mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => match String::from_utf8_lossy(&line).trim_end() {
                    "PTT ON" => flag.store(true, Ordering::SeqCst),
                    "PTT OFF" => flag.store(false, Ordering::SeqCst),
                    _ => {}
                },
            }
        }
    });

    (
        Station {
            call,
            cmd_addr,
            data_addr,
            keyed,
        },
        StationAudio {
            input: in_producer,
            output: out_consumer,
        },
    )
}

/// One direction of the cable: `from`'s transmitted audio (or silence when it
/// is not keyed) to `to`'s input, and silence while `to` is itself keyed.
fn cable_step(
    from_out: &mut AudioRingConsumer,
    to_in: &mut AudioRingProducer,
    to_keyed: &AtomicBool,
    buf: &mut [f32],
) {
    buf.fill(0.0);
    let read = from_out.read(buf);
    if to_keyed.load(Ordering::SeqCst) {
        buf[..read].fill(0.0);
    }
    to_in.write(buf);
}

/// Join two stations' audio, both ways, for the rest of the test.
fn spawn_cable(a: &Station, mut a_audio: StationAudio, b: &Station, mut b_audio: StationAudio) {
    let a_keyed = a.keyed.clone();
    let b_keyed = b.keyed.clone();
    tokio::task::spawn_local(async move {
        let mut tick = tokio::time::interval(CABLE_TICK);
        let mut buf = vec![0.0f32; CABLE_SAMPLES];
        loop {
            tick.tick().await;
            cable_step(&mut a_audio.output, &mut b_audio.input, &b_keyed, &mut buf);
            cable_step(&mut b_audio.output, &mut a_audio.input, &a_keyed, &mut buf);
        }
    });
}

/// What Pat-Vara does on the two VARA sockets, and what it sees back.
struct PatVara {
    call: &'static str,
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    data: TcpStream,
    /// Every non-empty command-port line, in arrival order (for diagnostics).
    seen: Vec<String>,
    /// Every `BUFFER n` value, in arrival order.
    buffers: Vec<usize>,
}

impl PatVara {
    /// Connect both sockets, check the greeting, and send `start()`'s burst.
    async fn open(station: &Station) -> PatVara {
        let cmd = TcpStream::connect(station.cmd_addr).await.unwrap();
        let data = TcpStream::connect(station.data_addr).await.unwrap();
        let (read_half, writer) = cmd.into_split();
        let mut pat = PatVara {
            call: station.call,
            reader: BufReader::new(read_half),
            writer,
            data,
            seen: Vec::new(),
            buffers: Vec::new(),
        };
        let greeting = pat.next_reply().await;
        assert_eq!(
            greeting,
            format!("VERSION Coppa {}", env!("CARGO_PKG_VERSION")),
            "{}: the greeting comes first; lines so far: {:?}",
            pat.call,
            pat.seen
        );
        for cmd in [
            "PUBLIC ON".to_string(),
            "CWID ON".to_string(),
            "COMPRESSION TEXT".to_string(),
            format!("MYCALL {}", station.call),
            "LISTEN OFF".to_string(),
            "BW2300".to_string(),
        ] {
            pat.send(&cmd).await;
        }
        let mut replies = Vec::new();
        for _ in 0..6 {
            replies.push(pat.next_reply().await);
        }
        assert_eq!(
            replies,
            ["WRONG", "WRONG", "OK", "OK", "OK", "OK"],
            "{}: Pat's start burst replies, in order; lines so far: {:?}",
            pat.call,
            pat.seen
        );
        pat
    }

    async fn send(&mut self, cmd: &str) {
        self.writer
            .write_all(format!("{cmd}\r").as_bytes())
            .await
            .unwrap();
    }

    /// The next command-port line, split on `\r` with empty strings skipped,
    /// as Pat-Vara reads them. Records `BUFFER` values.
    async fn next_line(&mut self) -> Option<String> {
        loop {
            let mut raw = Vec::new();
            let n = self.reader.read_until(b'\r', &mut raw).await.unwrap();
            if n == 0 {
                return None;
            }
            let text = String::from_utf8(raw).expect("VARA lines are UTF-8");
            assert!(
                !text.starts_with('\n'),
                "{}: a VARA line must not start with \\n (CRLF framing): {:?}",
                self.call,
                text
            );
            let line = text.trim_end_matches('\r').to_string();
            if line.is_empty() {
                continue;
            }
            if let Some(n) = line.strip_prefix("BUFFER ") {
                self.buffers
                    .push(n.parse().expect("BUFFER carries a count"));
            }
            self.seen.push(line.clone());
            return Some(line);
        }
    }

    /// The next line that is not telemetry: the reply to the last command,
    /// or a session event.
    async fn next_reply(&mut self) -> String {
        let fut = async {
            loop {
                match self.next_line().await {
                    Some(line) if is_telemetry(&line) => continue,
                    Some(line) => return Some(line),
                    None => return None,
                }
            }
        };
        match tokio::time::timeout(REPLY_TIMEOUT, fut).await {
            Ok(Some(line)) => line,
            Ok(None) => panic!("{}: command port closed; lines: {:?}", self.call, self.seen),
            Err(_) => panic!("{}: no reply in time; lines: {:?}", self.call, self.seen),
        }
    }

    /// Send `cmd` and assert its reply.
    async fn command(&mut self, cmd: &str, expect: &str) {
        self.send(cmd).await;
        let reply = self.next_reply().await;
        assert_eq!(
            reply, expect,
            "{}: reply to {cmd:?}; lines so far: {:?}",
            self.call, self.seen
        );
    }

    /// Wait (bounded) for a line starting with `prefix`, skipping others.
    async fn wait(&mut self, prefix: &str, limit: Duration) -> String {
        let fut = async {
            loop {
                match self.next_line().await {
                    Some(line) if line.starts_with(prefix) => return Some(line),
                    Some(_) => continue,
                    None => return None,
                }
            }
        };
        match tokio::time::timeout(limit, fut).await {
            Ok(Some(line)) => line,
            Ok(None) => panic!(
                "{}: command port closed waiting for {prefix:?}; lines: {:?}",
                self.call, self.seen
            ),
            Err(_) => panic!(
                "{}: timed out waiting for {prefix:?}; lines: {:?}",
                self.call, self.seen
            ),
        }
    }

    /// Write session data to the data port, as Pat's B2F writer does.
    async fn write_data(&mut self, bytes: &[u8]) {
        self.data.write_all(bytes).await.unwrap();
    }

    /// Pat's `Flush`: wait until the modem reports `BUFFER 0` after the
    /// queued bytes were counted.
    async fn flush(&mut self) {
        let fut = async {
            // Only a nonzero count proves the write was counted; a stale
            // `BUFFER 0` from before it must not end the flush.
            let mut counted = false;
            loop {
                match self.next_line().await {
                    Some(line) if line == "BUFFER 0" => {
                        if counted {
                            return true;
                        }
                    }
                    Some(line) if line.starts_with("BUFFER ") => counted = true,
                    Some(_) => continue,
                    None => return false,
                }
            }
        };
        match tokio::time::timeout(FLUSH_TIMEOUT, fut).await {
            Ok(true) => {}
            Ok(false) => panic!(
                "{}: command port closed in flush; lines: {:?}",
                self.call, self.seen
            ),
            Err(_) => panic!(
                "{}: flush never saw BUFFER 0; lines: {:?}",
                self.call, self.seen
            ),
        }
    }

    /// Read exactly `len` bytes of session data from the data port.
    async fn read_data(&mut self, len: usize) -> Vec<u8> {
        let mut buf = vec![0u8; len];
        match tokio::time::timeout(FLUSH_TIMEOUT, self.data.read_exact(&mut buf)).await {
            Ok(Ok(_)) => buf,
            Ok(Err(e)) => panic!("{}: data port read failed: {e}", self.call),
            Err(_) => panic!(
                "{}: timed out reading {len} data bytes; command lines: {:?}",
                self.call, self.seen
            ),
        }
    }

    /// Pat's `Close`: 2 s after the last write, `DISCONNECT`, then wait for
    /// `DISCONNECTED`.
    async fn close(&mut self) {
        tokio::time::sleep(Duration::from_secs(2)).await;
        self.send("DISCONNECT").await;
        self.wait("DISCONNECTED", DISCONNECT_TIMEOUT).await;
    }
}

/// Assert a `CONNECTED` line is exactly `expect` and has the 3+ tokens
/// Pat-Vara indexes without a bounds check (it panics on fewer).
fn assert_connected(call: &str, line: &str, expect: &str) {
    assert!(
        line.split_whitespace().count() >= 3,
        "{call}: Pat panics on a CONNECTED line with fewer than 3 tokens: {line:?}"
    );
    assert_eq!(line, expect, "{call}: CONNECTED line");
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| b' ' + ((i as u8).wrapping_mul(7).wrapping_add(seed) % 94))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn pat_vara_session_connects_exchanges_and_disconnects_both_ways() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let wall = std::time::Instant::now();
            let (a_station, a_audio) = station("N1CALL").await;
            let (b_station, b_audio) = station("N0CALL").await;
            spawn_cable(&a_station, a_audio, &b_station, b_audio);

            let mut a = PatVara::open(&a_station).await;
            let mut b = PatVara::open(&b_station).await;

            // Session 1: B listens, A dials B.
            b.command("LISTEN ON", "OK").await;
            a.command("WINLINK SESSION", "WRONG").await;
            a.command("CONNECT N1CALL N0CALL", "OK").await;
            let expect = "CONNECTED N1CALL N0CALL 2400";
            let line = a.wait("CONNECTED", CONNECT_TIMEOUT).await;
            assert_connected("N1CALL", &line, expect);
            let line = b.wait("CONNECTED", CONNECT_TIMEOUT).await;
            assert_connected("N0CALL", &line, expect);
            println!("session 1: {expect}");

            // B2F: the answering station's banner first, then the caller's
            // proposal-sized block, more than one chunk at HF_STANDARD.
            let banner = b";FW: N0CALL\r[Pat-1.0.0-B2FHMG$]\r; N1CALL DE N0CALL ()>\r";
            b.write_data(banner).await;
            b.flush().await;
            assert_eq!(
                a.read_data(banner.len()).await,
                banner,
                "banner, byte-exact"
            );

            let block = pattern(400, 3);
            let before = a.buffers.len();
            a.write_data(&block).await;
            a.flush().await;
            assert_eq!(
                b.read_data(block.len()).await,
                block,
                "400 B block, byte-exact"
            );
            let counts = &a.buffers[before..];
            let peak = counts
                .iter()
                .enumerate()
                .max_by_key(|(_, n)| **n)
                .map(|(i, _)| i)
                .expect("A reported BUFFER while sending");
            assert!(
                counts[peak..].windows(2).all(|w| w[1] <= w[0]),
                "BUFFER must not grow after its peak while draining: {counts:?}"
            );
            assert_eq!(counts.last(), Some(&0), "BUFFER ends at 0: {counts:?}");

            a.close().await;
            b.wait("DISCONNECTED", DISCONNECT_TIMEOUT).await;
            println!("session 1: DISCONNECTED on both stations");

            // Session 2, roles reversed on the same stations: a listener that
            // takes repeated calls depends on the state reset between them.
            a.command("LISTEN ON", "OK").await;
            b.command("WINLINK SESSION", "WRONG").await;
            b.command("CONNECT N0CALL N1CALL", "OK").await;
            let expect = "CONNECTED N0CALL N1CALL 2400";
            let line = b.wait("CONNECTED", CONNECT_TIMEOUT).await;
            assert_connected("N0CALL", &line, expect);
            let line = a.wait("CONNECTED", CONNECT_TIMEOUT).await;
            assert_connected("N1CALL", &line, expect);
            println!("session 2: {expect}");

            let banner = b";FW: N1CALL\r[Pat-1.0.0-B2FHMG$]\r; N0CALL DE N1CALL ()>\r";
            a.write_data(banner).await;
            a.flush().await;
            assert_eq!(b.read_data(banner.len()).await, banner, "reversed banner");
            let reply = pattern(40, 11);
            b.write_data(&reply).await;
            b.flush().await;
            assert_eq!(a.read_data(reply.len()).await, reply, "reversed reply");

            b.close().await;
            a.wait("DISCONNECTED", DISCONNECT_TIMEOUT).await;
            println!(
                "session 2: DISCONNECTED on both stations; wall {:.1?}",
                wall.elapsed()
            );
        })
        .await;
}
