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

// COP-142: the std-clock ban in clippy.toml targets event_loop only.
#![allow(clippy::disallowed_methods)]

mod vara_loop;
use vara_loop::*;

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
            // COP-84: Pat's `CWID ON` turned on the closing CW ID, so A's
            // last interval carries its Morse ID after the DISCONNECT frame,
            // and A cannot hear a call until it unkeys. B dials once it has.
            a.wait("PTT OFF", DISCONNECT_TIMEOUT).await;

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
