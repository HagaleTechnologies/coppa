//! COP-97: a Pat-ARDOP client's exact wire sequence, end to end, against two
//! real `EventLoop` stations over the ARDOP TNC host interface.
//!
//! Each station (N1CALL, N0CALL) is a real `EventLoop` running `run()`,
//! wired to its ARDOP server by the same `coppa_daemon::ardop_wiring::
//! serve_ardop` that `coppad`'s `main` uses, on OS-assigned ports where the
//! data port is the command port's ASCII-increment derivation (numeric case:
//! cmd+1). The two stations' audio rings are joined by the same half-duplex
//! virtual cable as `pat_vara_session.rs`, and a `PatArdop` emulator per
//! station drives the command and data ports with the command sequence read
//! from Pat's ARDOP transport (`wl2k-go/transport/ardop/tnc.go`): the init
//! burst (echo acks, `STATE` answered with `STATE DISC`), `LISTEN TRUE`,
//! `ARQCALL` with no echo and `NEWSTATE ISS` then `CONNECTED`, `TARGET`
//! before `CONNECTED` on the listener (Pat's listen gate), framed data both
//! ways (`<len:u16 BE>` in, `<len:u16 BE><ARQ><payload>` out), flow control
//! on `BUFFER 0`, and `DISCONNECT` → `DISCONNECTED`.
//!
//! Limitation, stated plainly (the `pat_vara_session.rs` precedent): the
//! test runs in tokio virtual time (`start_paused`), and virtual time does
//! not advance while a station decodes synchronously, so it cannot catch a
//! real-time decode stall. `tools/pat-loop/run.sh --transport ardop`, which
//! runs real Pat against two real release `coppad` processes, covers that.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use coppa_audio::{AudioRingConsumer, AudioRingProducer};
use coppa_daemon::ardop_wiring::serve_ardop;
use coppa_daemon::config::DaemonConfig;
use coppa_daemon::event_loop::{forward_data_out, EventLoop};
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
    ["BUSY ", "BUFFER ", "PTT "]
        .iter()
        .any(|t| line.starts_with(t))
}

/// Pat's data-address derivation: ASCII-increment of the last character of
/// the command address (equal to cmd+1 for numeric ports, which this test's
/// stations guarantee by binding the derived port).
fn ascii_increment(addr: &str) -> String {
    let mut bytes = addr.as_bytes().to_vec();
    let last = bytes.len() - 1;
    bytes[last] += 1;
    String::from_utf8(bytes).expect("derived data address is UTF-8")
}

/// A running station: its ports, and its keyed state as seen on the command
/// port by an always-reading monitor client.
struct Station {
    call: &'static str,
    cmd_addr: std::net::SocketAddr,
    data_addr: std::net::SocketAddr,
    keyed: Arc<AtomicBool>,
}

/// A station's audio ring ends, for the cable.
struct StationAudio {
    input: AudioRingProducer,
    output: AudioRingConsumer,
}

/// Build, wire and start one station inside the current `LocalSet`. The
/// data listener binds the ASCII-increment of the command port, so the
/// emulator's derivation matches, as it does against a real TNC's
/// 8515/8516.
async fn station(call: &'static str) -> (Station, StationAudio) {
    let (cmd_listener, data_listener) = loop {
        let cmd = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let cmd_addr = cmd.local_addr().unwrap();
        // Skip addresses whose last char is '9': ASCII-increment would leave
        // the digit range, exactly as in real Pat.
        if cmd_addr.to_string().ends_with('9') {
            continue;
        }
        let derived = ascii_increment(&cmd_addr.to_string());
        let data_addr: std::net::SocketAddr = derived.parse().expect("derived address parses");
        if let Ok(data) = TcpListener::bind(data_addr).await {
            break (cmd, data);
        }
    };
    let cmd_addr = cmd_listener.local_addr().unwrap();
    let data_addr = data_listener.local_addr().unwrap();

    let mut config = DaemonConfig::default();
    config.engine.callsign = call.into();
    config.engine.profile = "HF_STANDARD".into();
    config.host.ardop_enabled = true;
    config.host.ardop_command_port = cmd_addr.port();
    config.host.ardop_data_port = data_addr.port();
    let host = config.host.clone();

    let (in_producer, in_consumer) = coppa_audio::audio_ring(config.audio.buffer_size);
    let (out_producer, out_consumer) = coppa_audio::audio_ring(config.audio.buffer_size);

    let mut event_loop = EventLoop::new(config).unwrap();
    let (response_tx, response_rx) = tokio::sync::mpsc::unbounded_channel();
    event_loop.set_response_tx(response_tx);
    event_loop.set_audio_in(in_consumer);
    event_loop.set_audio_out(out_producer);
    let data_senders = serve_ardop(&mut event_loop, cmd_listener, data_listener, &host);
    tokio::task::spawn_local(async move {
        let _ = forward_data_out(response_rx, vec![data_senders]).await;
    });
    tokio::task::spawn_local(async move {
        let _ = event_loop.run().await;
    });

    // PTT monitor: a second command client that reads every line, so the
    // server never evicts it as slow (COP-34), and tracks the keyed state
    // from `PTT TRUE/FALSE`.
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
                    "PTT TRUE" => flag.store(true, Ordering::SeqCst),
                    "PTT FALSE" => flag.store(false, Ordering::SeqCst),
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

/// One direction of the cable: `from`'s transmitted audio (or silence when
/// it is not keyed) to `to`'s input, and silence while `to` is itself keyed.
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

/// What Pat-ARDOP does on the two ARDOP sockets, and what it sees back.
struct PatArdop {
    call: &'static str,
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    data: TcpStream,
    /// Every non-empty command-port line, in arrival order (for diagnostics).
    seen: Vec<String>,
    /// Every `BUFFER n` value, in arrival order.
    buffers: Vec<usize>,
}

impl PatArdop {
    /// Connect both sockets the way Pat does (data address derived by the
    /// ASCII increment), check there is no greeting, and send the verified
    /// init sequence, asserting every reply (the §Design/2 table).
    async fn open(station: &Station) -> PatArdop {
        let data_addr = ascii_increment(&station.cmd_addr.to_string());
        assert_eq!(
            data_addr.parse::<std::net::SocketAddr>().unwrap().port(),
            station.data_addr.port(),
            "Pat's ASCII-increment derivation equals cmd+1 on numeric ports"
        );
        let cmd = TcpStream::connect(station.cmd_addr).await.unwrap();
        let data_addr: std::net::SocketAddr = data_addr.parse().unwrap();
        let data = TcpStream::connect(data_addr).await.unwrap();
        let (read_half, writer) = cmd.into_split();
        let mut pat = PatArdop {
            call: station.call,
            reader: BufReader::new(read_half),
            writer,
            data,
            seen: Vec::new(),
            buffers: Vec::new(),
        };
        // ARDOP TNCs send no greeting: the first line is the reply to the
        // first command (`open`'s first `next_reply` would hang otherwise).
        for cmd in [
            "INITIALIZE",
            "STATE",
            "CODEC TRUE",
            "PROTOCOLMODE ARQ",
            "ARQTIMEOUT 90",
            "LISTEN FALSE",
            format!("MYCALL {}", station.call).as_str(),
            "GRIDSQUARE FN31PR",
            "ARQBW 500MAX",
            "VERSION",
        ] {
            pat.send(cmd).await;
            let reply = pat.next_reply().await;
            let expect = match cmd {
                "STATE" => "STATE DISC".to_string(),
                "VERSION" => format!("VERSION Coppa {}", env!("CARGO_PKG_VERSION")),
                other => other.to_string(),
            };
            assert_eq!(
                reply, expect,
                "{}: reply to {cmd:?}; lines so far: {:?}",
                pat.call, pat.seen
            );
        }
        pat
    }

    async fn send(&mut self, cmd: &str) {
        self.writer
            .write_all(format!("{cmd}\r").as_bytes())
            .await
            .unwrap();
    }

    /// The next command-port line, split on `\r` with empty strings skipped,
    /// as Pat reads them. Records `BUFFER` values.
    async fn next_line(&mut self) -> Option<String> {
        loop {
            let mut raw = Vec::new();
            let n = self.reader.read_until(b'\r', &mut raw).await.unwrap();
            if n == 0 {
                return None;
            }
            let text = String::from_utf8(raw).expect("ARDOP lines are UTF-8");
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

    /// Send `cmd` and assert its exact reply.
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

    /// Write one framed data-port payload (host→TNC framing:
    /// `<len:u16 BE><payload>`).
    async fn write_data(&mut self, bytes: &[u8]) {
        let mut frame = (bytes.len() as u16).to_be_bytes().to_vec();
        frame.extend_from_slice(bytes);
        self.data.write_all(&frame).await.unwrap();
    }

    /// Read `len` payload bytes as TNC→host frames
    /// (`<len:u16 BE><ARQ><payload>`), tag checked, framing stripped.
    async fn read_data(&mut self, len: usize) -> Vec<u8> {
        let mut got = Vec::with_capacity(len);
        while got.len() < len {
            let mut header = [0u8; 5];
            tokio::time::timeout(FLUSH_TIMEOUT, self.data.read_exact(&mut header))
                .await
                .expect("timed out reading a data frame header")
                .expect("data port read should not error");
            let frame_len = u16::from_be_bytes([header[0], header[1]]) as usize;
            let tag = std::str::from_utf8(&header[2..5]).unwrap_or("???");
            assert_eq!(
                tag, "ARQ",
                "{}: session data carries the ARQ tag",
                self.call
            );
            let mut payload = vec![0u8; frame_len - 3];
            tokio::time::timeout(FLUSH_TIMEOUT, self.data.read_exact(&mut payload))
                .await
                .expect("timed out reading data frame bytes")
                .expect("data port read should not error");
            got.extend_from_slice(&payload);
        }
        got
    }

    /// Pat's `Flush`: wait until the modem reports `BUFFER 0` after the
    /// queued bytes were counted.
    async fn flush(&mut self) {
        let fut = async {
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

    /// Pat's `Close`: `DISCONNECT` (echo ack), then wait for `DISCONNECTED`.
    async fn close(&mut self) {
        self.send("DISCONNECT").await;
        assert_eq!(self.next_reply().await, "DISCONNECT", "the echo ack");
        self.wait("DISCONNECTED", DISCONNECT_TIMEOUT).await;
    }
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| b' ' + ((i as u8).wrapping_mul(7).wrapping_add(seed) % 94))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn pat_ardop_session_connects_exchanges_and_disconnects_both_ways() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let wall = std::time::Instant::now();
            let (a_station, a_audio) = station("N1CALL").await;
            let (b_station, b_audio) = station("N0CALL").await;
            spawn_cable(&a_station, a_audio, &b_station, b_audio);

            let mut a = PatArdop::open(&a_station).await;
            let mut b = PatArdop::open(&b_station).await;

            // Session 1: B listens, A dials B.
            b.command("LISTEN TRUE", "LISTEN TRUE").await;
            a.send("ARQCALL N0CALL 3").await;
            // No echo: NEWSTATE ISS, then the outcome.
            assert_eq!(a.next_reply().await, "NEWSTATE ISS ", "lines: {:?}", a.seen);
            let line = a.wait("CONNECTED ", CONNECT_TIMEOUT).await;
            assert_eq!(line, "CONNECTED N0CALL 500MAX", "lines: {:?}", a.seen);
            a.wait("NEWSTATE ISS ", REPLY_TIMEOUT).await;
            // The listener saw TARGET first (Pat's listen gate), then
            // CONNECTED, then NEWSTATE IRS.
            let line = b.wait("TARGET ", CONNECT_TIMEOUT).await;
            assert_eq!(line, "TARGET N0CALL", "lines: {:?}", b.seen);
            let line = b.wait("CONNECTED ", CONNECT_TIMEOUT).await;
            assert_eq!(line, "CONNECTED N1CALL 500MAX", "lines: {:?}", b.seen);
            b.wait("NEWSTATE IRS ", REPLY_TIMEOUT).await;
            let target_at = b
                .seen
                .iter()
                .position(|l| l.starts_with("TARGET "))
                .unwrap();
            let connected_at = b
                .seen
                .iter()
                .position(|l| l.starts_with("CONNECTED "))
                .unwrap();
            assert!(
                target_at < connected_at,
                "TARGET before CONNECTED: {:?}",
                b.seen
            );
            println!("session 1: CONNECTED, TARGET led on the listener");

            // B2F: the answering station's banner first, then the caller's
            // proposal-sized block, framed both ways, byte-exact.
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
            b.wait("NEWSTATE DISC ", REPLY_TIMEOUT).await;
            a.wait("NEWSTATE DISC ", REPLY_TIMEOUT).await;
            println!("session 1: DISCONNECTED on both stations");

            // Session 2, roles reversed on the same stations: the state
            // reset works for a listener that takes repeated calls.
            a.command("LISTEN TRUE", "LISTEN TRUE").await;
            b.send("ARQCALL N1CALL 3").await;
            assert_eq!(b.next_reply().await, "NEWSTATE ISS ", "lines: {:?}", b.seen);
            let line = b.wait("CONNECTED ", CONNECT_TIMEOUT).await;
            assert_eq!(line, "CONNECTED N1CALL 500MAX", "lines: {:?}", b.seen);
            b.wait("NEWSTATE ISS ", REPLY_TIMEOUT).await;
            let line = a.wait("TARGET ", CONNECT_TIMEOUT).await;
            assert_eq!(line, "TARGET N1CALL", "lines: {:?}", a.seen);
            let line = a.wait("CONNECTED ", CONNECT_TIMEOUT).await;
            assert_eq!(line, "CONNECTED N0CALL 500MAX", "lines: {:?}", a.seen);
            println!("session 2: CONNECTED, reversed");

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
