//! COP-40: honest rigctld frequency polling and `PttBackend` retention,
//! exercised over a real TCP socket against a fake rigctld.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use coppa_radio::band::{gate, FrequencyEvidence, GateRefusal};
use coppa_radio::{PttBackend, PttControl, PttState, RadioControl, RigctldClient};

/// A fake rigctld serving one client: answers `f` with whatever `answer`
/// currently holds, `T <n>` with `RPRT 0`, and `t` with the last keyed state.
/// Returns the address and the shared `f` answer, so a test can retune.
fn fake_rigctld(answer: &str) -> (String, Arc<Mutex<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let answer = Arc::new(Mutex::new(answer.to_string()));
    let shared = Arc::clone(&answer);
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut ptt = "0".to_string();
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            let reply = match line.trim() {
                "f" => shared.lock().unwrap().clone(),
                "t" => ptt.clone(),
                cmd if cmd.starts_with("T ") => {
                    ptt = cmd[2..].to_string();
                    "RPRT 0".to_string()
                }
                _ => "RPRT -1".to_string(),
            };
            if writeln!(writer, "{reply}").is_err() {
                break;
            }
        }
    });
    (addr, answer)
}

/// A fake rigctld that accepts one connection and immediately closes it.
fn dead_rigctld() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        drop(stream);
    });
    addr
}

#[test]
fn poll_frequency_returns_some_on_a_clean_answer() {
    let (addr, _) = fake_rigctld("144500000");
    let mut client = RigctldClient::connect(&addr).unwrap();
    assert_eq!(client.poll_frequency(), Some(144_500_000));
}

#[test]
fn query_frequency_returns_the_dial_frequency() {
    let (addr, _) = fake_rigctld("144500000");
    let mut client = RigctldClient::connect(&addr).unwrap();
    assert_eq!(client.query_frequency().unwrap(), 144_500_000);
}

#[test]
fn query_frequency_reports_a_non_numeric_answer_verbatim() {
    let (addr, _) = fake_rigctld("RPRT -5");
    let mut client = RigctldClient::connect(&addr).unwrap();
    assert_eq!(
        client.query_frequency().unwrap_err().to_string(),
        r#"rigctld answered "RPRT -5" to `f`"#
    );
}

#[test]
fn query_frequency_reports_a_peer_that_closed() {
    let addr = dead_rigctld();
    let mut client = RigctldClient::connect(&addr).unwrap();
    let error = client.query_frequency().unwrap_err().to_string();
    assert!(!error.contains("answered"), "transport error: {error}");
}

#[test]
fn poll_frequency_returns_none_when_the_peer_dies() {
    let addr = dead_rigctld();
    let mut client = RigctldClient::connect(&addr).unwrap();
    assert_eq!(client.poll_frequency(), None);
}

#[test]
fn poll_frequency_returns_none_on_a_garbage_answer() {
    let (addr, _) = fake_rigctld("CHKVFO?");
    let mut client = RigctldClient::connect(&addr).unwrap();
    assert_eq!(client.poll_frequency(), None);
}

/// Regression pin: the fail-open `RadioControl::get_frequency` keeps its old
/// behavior (cached construction default on a garbage answer); only the new
/// `poll_frequency` is honest.
#[test]
fn radio_control_get_frequency_is_unchanged() {
    let (addr, _) = fake_rigctld("CHKVFO?");
    let mut client = RigctldClient::connect(&addr).unwrap();
    assert_eq!(
        RadioControl::get_frequency(&mut client).unwrap(),
        14_074_000
    );
}

#[test]
fn ptt_backend_rigctld_delegates_ptt_and_polls_frequency() {
    let (addr, answer) = fake_rigctld("144500000");
    let mut backend = PttBackend::Rigctld(RigctldClient::connect(&addr).unwrap());
    assert!(backend.is_rigctld());
    backend.set_ptt(PttState::Tx).unwrap();
    assert_eq!(backend.get_ptt().unwrap(), PttState::Tx);
    backend.set_ptt(PttState::Rx).unwrap();
    assert_eq!(backend.get_ptt().unwrap(), PttState::Rx);
    assert_eq!(backend.poll_frequency(), Some(144_500_000));

    // A retune is seen on the very next poll, and the gate follows it.
    *answer.lock().unwrap() = "14074000".to_string();
    let live = backend.poll_frequency().map(FrequencyEvidence::Live);
    assert_eq!(live, Some(FrequencyEvidence::Live(14_074_000)));
    assert_eq!(
        gate(5600, live.as_ref()),
        Err(GateRefusal::BelowThreshold {
            evidence_hz: 14_074_000,
            occupied_hz: 5600
        })
    );
}

/// A poll whose reply arrives after the 2 s read timeout must not leave that
/// late reply to be read by the next poll as the current frequency: the rig
/// answered "on 2m" late, was then retuned to 20m, and the next poll must
/// see 20m (or nothing), never the stale 2m.
#[test]
fn a_late_reply_is_never_read_as_the_next_polls_answer() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let polls = Arc::new(Mutex::new(0u32));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let polls = Arc::clone(&polls);
            std::thread::spawn(move || {
                let mut writer = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    if line.trim() != "f" {
                        continue;
                    }
                    let first = {
                        let mut n = polls.lock().unwrap();
                        *n += 1;
                        *n == 1
                    };
                    let reply = if first {
                        // Answer "2m", but only after the client gave up.
                        std::thread::sleep(std::time::Duration::from_millis(2500));
                        "144500000"
                    } else {
                        "14074000"
                    };
                    if writeln!(writer, "{reply}").is_err() {
                        break;
                    }
                }
            });
        }
    });

    let mut client = RigctldClient::connect(&addr).unwrap();
    assert_eq!(client.poll_frequency(), None, "the first poll timed out");
    std::thread::sleep(std::time::Duration::from_millis(700));
    assert_eq!(
        client.poll_frequency(),
        Some(14_074_000),
        "the next poll reads the rig's current answer, not the stale late one"
    );
}
