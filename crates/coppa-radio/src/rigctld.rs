//! TCP client for hamlib rigctld protocol.
//!
//! Connects to rigctld on localhost:4532 and implements
//! RadioControl + PttControl via the text-based protocol.

use anyhow::{anyhow, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};

use crate::{PttControl, PttState, RadioControl, RadioMode};

/// rigctld client for CAT control via hamlib.
///
/// Releases PTT on drop if keyed: a client that sent `T 1` and has not had a
/// `T 0` accepted since sends `T 0` when dropped (see the `Drop` impl).
pub struct RigctldClient {
    addr: String,
    stream: TcpStream,
    reader: BufReader<TcpStream>,
    connected: bool,
    frequency: u64,
    mode: RadioMode,
    ptt: PttState,
    /// True from the moment this client sends `T 1` until a `T 0` it sent is
    /// accepted. Set before `T 1` goes out: a lost or late reply can still leave
    /// the rig keyed. `Drop` releases only when this is set, so a client that
    /// never keyed (one of several on a shared rigctld) sends nothing on drop.
    may_be_keyed: bool,
}

impl RigctldClient {
    /// Connect to rigctld at the given address (e.g., "127.0.0.1:4532").
    /// TCP establishment times out after 2 s per resolved address.
    pub fn connect(addr: &str) -> Result<Self> {
        Self::connect_timeout(addr, std::time::Duration::from_secs(2))
    }

    /// Connect with an explicit TCP establishment timeout per resolved address.
    /// Hostname resolution is synchronous and is not covered by this timeout.
    /// Read and write timeouts remain 2 s each.
    pub fn connect_timeout(addr: &str, timeout: std::time::Duration) -> Result<Self> {
        let mut last_err = anyhow!("rigctld address {} did not resolve", addr);
        let mut connected = None;
        for sock in addr.to_socket_addrs()? {
            match TcpStream::connect_timeout(&sock, timeout) {
                Ok(stream) => {
                    connected = Some(stream);
                    break;
                }
                Err(e) => last_err = anyhow!("Failed to connect to rigctld at {}: {}", sock, e),
            }
        }
        let stream = connected.ok_or(last_err)?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(2)))?;
        let reader = BufReader::new(stream.try_clone()?);

        Ok(Self {
            addr: addr.to_string(),
            stream,
            reader,
            connected: true,
            frequency: 14_074_000,
            mode: RadioMode::Usb,
            ptt: PttState::Rx,
            may_be_keyed: false,
        })
    }

    /// Ask the rig for its dial frequency (`f`) with a reason on failure.
    /// `Ok(hz)` requires a clean numeric answer and refreshes the cache. An
    /// error reports the transport failure or rigctld's answer, such as
    /// `RPRT -5` for a hamlib timeout.
    ///
    /// Any failure invalidates the connection: a timed-out read can leave
    /// the late reply buffered on the old socket. The next command must
    /// reconnect successfully before sending or reading anything, even if
    /// earlier reconnect attempts failed.
    pub fn query_frequency(&mut self) -> Result<u64> {
        let result = self.command("f").and_then(|resp| {
            let answer = resp.trim();
            answer
                .parse::<u64>()
                .map_err(|_| anyhow!("rigctld answered {answer:?} to `f`"))
        });
        match result {
            Ok(freq) => {
                self.frequency = freq;
                Ok(freq)
            }
            Err(error) => {
                self.connected = false;
                let _ = self.stream.shutdown(Shutdown::Both);
                Err(error)
            }
        }
    }

    /// Ask the rig for its dial frequency (`f`), honestly: `Some(hz)` only on
    /// a clean numeric answer (which also refreshes the cache), `None` on any
    /// transport or parse failure. Unlike `RadioControl::get_frequency`, this
    /// never falls back to the cached or construction-default value, so a
    /// regulatory check can tell "the rig says X" from "nobody answered".
    /// See [`Self::query_frequency`] for the reason when this returns `None`.
    pub fn poll_frequency(&mut self) -> Option<u64> {
        self.query_frequency().ok()
    }

    /// Replace the connection with a fresh one to the same address, dropping
    /// anything still buffered on the old socket. Bounded by a 2 s connect
    /// timeout per resolved address.
    fn reconnect(&mut self) -> Result<()> {
        let timeout = std::time::Duration::from_secs(2);
        let mut last_err = anyhow!("rigctld address {} did not resolve", self.addr);
        for sock in self.addr.to_socket_addrs()? {
            match TcpStream::connect_timeout(&sock, timeout) {
                Ok(stream) => {
                    stream.set_read_timeout(Some(timeout))?;
                    stream.set_write_timeout(Some(timeout))?;
                    self.reader = BufReader::new(stream.try_clone()?);
                    self.stream = stream;
                    self.connected = true;
                    return Ok(());
                }
                Err(e) => last_err = anyhow!("Failed to reconnect to rigctld at {}: {}", sock, e),
            }
        }
        Err(last_err)
    }

    /// Send a command and read one line of response.
    ///
    /// COP-31: any transport failure, including rigctld closing the
    /// connection (EOF, which is not an empty answer: a `T 0` that never
    /// arrived must not count as accepted), invalidates the connection, so
    /// the next command reconnects instead of reusing a dead socket or
    /// reading a late reply to this command as its own.
    fn command(&mut self, cmd: &str) -> Result<String> {
        if !self.connected {
            self.reconnect()?;
        }
        self.exchange(cmd, 1).map(|mut lines| lines.swap_remove(0))
    }

    /// Send a command and read two lines of response (e.g., mode + passband).
    fn command_two_lines(&mut self, cmd: &str) -> Result<(String, String)> {
        if !self.connected {
            self.reconnect()?;
        }
        let mut lines = self.exchange(cmd, 2)?;
        let line2 = lines.pop().unwrap_or_default();
        let line1 = lines.pop().unwrap_or_default();
        Ok((line1, line2))
    }

    /// Write `cmd` and read `count` trimmed response lines on the current
    /// connection; on any failure, mark the connection dead.
    fn exchange(&mut self, cmd: &str, count: usize) -> Result<Vec<String>> {
        let result = (|| {
            writeln!(self.stream, "{}", cmd)
                .map_err(|e| anyhow!("Failed to send command: {}", e))?;
            self.stream.flush()?;
            let mut lines = Vec::with_capacity(count);
            for n in 1..=count {
                let mut line = String::new();
                let read = self
                    .reader
                    .read_line(&mut line)
                    .map_err(|e| anyhow!("Failed to read response line {}: {}", n, e))?;
                if read == 0 {
                    return Err(anyhow!("rigctld at {} closed the connection", self.addr));
                }
                lines.push(line.trim().to_string());
            }
            Ok(lines)
        })();
        if result.is_err() {
            self.connected = false;
            let _ = self.stream.shutdown(Shutdown::Both);
        }
        result
    }

    fn parse_mode(s: &str) -> RadioMode {
        match s.to_uppercase().as_str() {
            "USB" => RadioMode::Usb,
            "LSB" => RadioMode::Lsb,
            "AM" => RadioMode::Am,
            "FM" => RadioMode::Fm,
            "PKTUSB" | "DIGU" | "DATA" => RadioMode::Digital,
            _ => RadioMode::Usb,
        }
    }

    fn mode_string(mode: RadioMode) -> &'static str {
        match mode {
            RadioMode::Usb => "USB",
            RadioMode::Lsb => "LSB",
            RadioMode::Am => "AM",
            RadioMode::Fm => "FM",
            RadioMode::Digital => "PKTUSB",
        }
    }
}

impl RadioControl for RigctldClient {
    fn get_frequency(&mut self) -> Result<u64> {
        match self.command("f") {
            Ok(resp) => {
                if let Ok(freq) = resp.trim().parse::<u64>() {
                    self.frequency = freq;
                    Ok(freq)
                } else {
                    Ok(self.frequency) // fallback to cached
                }
            }
            Err(_) => Ok(self.frequency), // fallback to cached on error
        }
    }

    fn set_frequency(&mut self, freq_hz: u64) -> Result<()> {
        let response = self.command(&format!("F {}", freq_hz))?;
        if response.starts_with("RPRT") {
            let code: i32 = response
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(-1);
            if code != 0 {
                return Err(anyhow!("rigctld set_frequency failed: {}", response));
            }
        }
        self.frequency = freq_hz;
        Ok(())
    }

    fn get_mode(&mut self) -> Result<RadioMode> {
        // rigctld 'm' command returns two lines: mode string and passband width
        match self.command_two_lines("m") {
            Ok((mode_str, _passband)) => {
                let mode = Self::parse_mode(mode_str.trim());
                self.mode = mode;
                Ok(mode)
            }
            Err(_) => Ok(self.mode), // fallback to cached on error
        }
    }

    fn set_mode(&mut self, mode: RadioMode) -> Result<()> {
        let mode_str = Self::mode_string(mode);
        let response = self.command(&format!("M {} 0", mode_str))?;
        if response.starts_with("RPRT") {
            let code: i32 = response
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(-1);
            if code != 0 {
                return Err(anyhow!("rigctld set_mode failed: {}", response));
            }
        }
        self.mode = mode;
        Ok(())
    }

    fn get_ptt(&mut self) -> Result<PttState> {
        match self.command("t") {
            Ok(resp) => {
                let state = match resp.trim() {
                    "0" => PttState::Rx,
                    _ => PttState::Tx,
                };
                self.ptt = state;
                Ok(state)
            }
            Err(_) => Ok(self.ptt), // fallback to cached on error
        }
    }

    fn set_ptt(&mut self, state: PttState) -> Result<()> {
        let val = match state {
            PttState::Rx => 0,
            PttState::Tx => 1,
        };
        if state == PttState::Tx {
            // Before `T 1` goes out: a lost or late reply can still key the rig.
            self.may_be_keyed = true;
        }
        let response = self.command(&format!("T {}", val))?;
        if response.starts_with("RPRT") {
            let code: i32 = response
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(-1);
            if code != 0 {
                return Err(anyhow!("rigctld set_ptt failed: {}", response));
            }
        }
        if state == PttState::Rx {
            // Only an accepted `T 0` clears it.
            self.may_be_keyed = false;
        }
        self.ptt = state;
        Ok(())
    }

    fn signal_strength(&mut self) -> Result<f32> {
        match self.command("l STRENGTH") {
            Ok(resp) => Ok(resp.trim().parse::<f32>().unwrap_or(0.0)),
            Err(_) => Ok(0.0),
        }
    }
}

impl PttControl for RigctldClient {
    fn set_ptt(&mut self, state: PttState) -> Result<()> {
        RadioControl::set_ptt(self, state)
    }

    fn get_ptt(&mut self) -> Result<PttState> {
        RadioControl::get_ptt(self)
    }
}

impl Drop for RigctldClient {
    /// COP-31: release PTT this client may have keyed. Best effort, and
    /// blocking: one `T 0` on the current connection and, if that fails (the
    /// socket may be dead, e.g. rigctld restarted), one more on a fresh
    /// connection -- at most two writes and reads (2 s each) plus one
    /// reconnect (2 s). Failure is logged at ERROR, not propagated (same shape
    /// as `WavSink::drop`).
    fn drop(&mut self) {
        if !self.may_be_keyed {
            return;
        }
        // A transport failure has already marked the connection dead, so
        // the retry reconnects; a refusal (`RPRT -1`) is simply retried.
        let mut result = RadioControl::set_ptt(self, PttState::Rx);
        if result.is_err() {
            result = RadioControl::set_ptt(self, PttState::Rx);
        }
        if let Err(e) = result {
            tracing::error!(
                addr = %self.addr,
                error = %e,
                "RigctldClient: failed to release PTT on drop"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_reconnect_cannot_reuse_a_late_frequency_reply() {
        use std::net::TcpListener;
        use std::sync::mpsc;
        use std::time::Duration;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let (sent_tx, sent_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Keep the existing connection alive but refuse reconnections.
            drop(listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            ready_tx.send(()).unwrap();
            let mut command = String::new();
            reader.read_line(&mut command).unwrap();
            assert_eq!(command.trim(), "f");
            reply_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            // The reply belonged to the timed-out query, before a retune.
            let _ = writeln!(stream, "144500000");
            sent_tx.send(()).unwrap();
            command.clear();
            if reader.read_line(&mut command).unwrap_or(0) > 0 {
                assert_eq!(command.trim(), "f");
                let _ = writeln!(stream, "14074000");
            }
        });
        let mut client = RigctldClient::connect(&addr).unwrap();
        client
            .stream
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(client.poll_frequency(), None, "the first query times out");
        reply_tx.send(()).unwrap();
        sent_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let next = client.poll_frequency();
        drop(client);
        server.join().unwrap();
        assert_eq!(
            next, None,
            "a failed reconnect must not reuse the old socket"
        );
    }

    #[test]
    fn test_parse_mode() {
        assert_eq!(RigctldClient::parse_mode("USB"), RadioMode::Usb);
        assert_eq!(RigctldClient::parse_mode("LSB"), RadioMode::Lsb);
        assert_eq!(RigctldClient::parse_mode("AM"), RadioMode::Am);
        assert_eq!(RigctldClient::parse_mode("FM"), RadioMode::Fm);
        assert_eq!(RigctldClient::parse_mode("PKTUSB"), RadioMode::Digital);
        assert_eq!(RigctldClient::parse_mode("DIGU"), RadioMode::Digital);
    }

    #[test]
    fn test_mode_string() {
        assert_eq!(RigctldClient::mode_string(RadioMode::Usb), "USB");
        assert_eq!(RigctldClient::mode_string(RadioMode::Lsb), "LSB");
        assert_eq!(RigctldClient::mode_string(RadioMode::Digital), "PKTUSB");
    }

    /// How the recording fake rigctld misbehaves, if at all.
    #[derive(Clone, Copy, Default)]
    struct FakeOpts {
        /// Reply `RPRT -1` to the first `T 0` (on any connection).
        fail_first_t0: bool,
        /// Read `T 1` but never answer it.
        ignore_t1: bool,
        /// Answer `T 1`, then close that connection (rigctld restarting).
        close_after_t1: bool,
    }

    /// A recording fake rigctld: accepts any number of connections and
    /// appends every command line it reads to `log`, before replying.
    struct FakeRigctld {
        addr: String,
        log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        opened: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        closed: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl FakeRigctld {
        fn start(opts: FakeOpts) -> Self {
            use std::net::TcpListener;
            use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
            use std::sync::{Arc, Mutex};

            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap().to_string();
            let log = Arc::new(Mutex::new(Vec::new()));
            let opened = Arc::new(AtomicUsize::new(0));
            let closed = Arc::new(AtomicUsize::new(0));
            let failed_t0 = Arc::new(AtomicBool::new(false));
            let ptt = Arc::new(Mutex::new("0".to_string()));
            let (log2, opened2, closed2) = (log.clone(), opened.clone(), closed.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { break };
                    opened2.fetch_add(1, Ordering::SeqCst);
                    let (log, closed) = (log2.clone(), closed2.clone());
                    let (failed_t0, ptt) = (failed_t0.clone(), ptt.clone());
                    std::thread::spawn(move || {
                        let mut writer = stream.try_clone().unwrap();
                        let mut reader = BufReader::new(stream);
                        let mut line = String::new();
                        while matches!(reader.read_line(&mut line), Ok(n) if n > 0) {
                            let cmd = line.trim().to_string();
                            line.clear();
                            log.lock().unwrap().push(cmd.clone());
                            let reply = match cmd.as_str() {
                                "f" => Some("14074000".to_string()),
                                "t" => Some(ptt.lock().unwrap().clone()),
                                "T 1" if opts.ignore_t1 => None,
                                "T 0"
                                    if opts.fail_first_t0
                                        && !failed_t0.swap(true, Ordering::SeqCst) =>
                                {
                                    Some("RPRT -1".to_string())
                                }
                                c if c.starts_with("T ") => {
                                    *ptt.lock().unwrap() = c[2..].to_string();
                                    Some("RPRT 0".to_string())
                                }
                                _ => Some("RPRT -1".to_string()),
                            };
                            if let Some(reply) = reply {
                                if writeln!(writer, "{reply}").is_err() {
                                    break;
                                }
                            }
                            if opts.close_after_t1 && cmd == "T 1" {
                                break;
                            }
                        }
                        closed.fetch_add(1, Ordering::SeqCst);
                    });
                }
            });
            Self {
                addr,
                log,
                opened,
                closed,
            }
        }

        /// Wait (bounded) until every connection the fake accepted has been
        /// closed by the client, so everything the client sent has been read
        /// and logged, then return the log.
        fn settled_log(&self) -> Vec<String> {
            use std::sync::atomic::Ordering;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let opened = self.opened.load(Ordering::SeqCst);
                if opened > 0 && self.closed.load(Ordering::SeqCst) == opened {
                    return self.log.lock().unwrap().clone();
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "fake rigctld connections never closed; log so far: {:?}",
                    self.log.lock().unwrap()
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }

    #[test]
    fn dropping_a_keyed_client_releases_ptt() {
        let fake = FakeRigctld::start(FakeOpts::default());
        let mut client = RigctldClient::connect(&fake.addr).unwrap();
        PttControl::set_ptt(&mut client, PttState::Tx).unwrap();
        drop(client);
        assert_eq!(fake.settled_log(), ["T 1", "T 0"]);
    }

    #[test]
    fn dropping_a_client_that_never_keyed_sends_nothing() {
        let fake = FakeRigctld::start(FakeOpts::default());
        let mut client = RigctldClient::connect(&fake.addr).unwrap();
        assert_eq!(client.poll_frequency(), Some(14_074_000));
        assert_eq!(PttControl::get_ptt(&mut client).unwrap(), PttState::Rx);
        drop(client);
        let log = fake.settled_log();
        assert_eq!(log, ["f", "t"]);
        assert!(!log.iter().any(|c| c == "T 0"), "log: {log:?}");
    }

    #[test]
    fn dropping_after_an_explicit_release_sends_nothing_more() {
        let fake = FakeRigctld::start(FakeOpts::default());
        let mut client = RigctldClient::connect(&fake.addr).unwrap();
        PttControl::set_ptt(&mut client, PttState::Tx).unwrap();
        PttControl::set_ptt(&mut client, PttState::Rx).unwrap();
        drop(client);
        assert_eq!(fake.settled_log(), ["T 1", "T 0"]);
    }

    #[test]
    fn a_failed_release_is_retried_on_drop() {
        let fake = FakeRigctld::start(FakeOpts {
            fail_first_t0: true,
            ..FakeOpts::default()
        });
        let mut client = RigctldClient::connect(&fake.addr).unwrap();
        PttControl::set_ptt(&mut client, PttState::Tx).unwrap();
        assert!(PttControl::set_ptt(&mut client, PttState::Rx).is_err());
        drop(client);
        assert_eq!(fake.settled_log(), ["T 1", "T 0", "T 0"]);
    }

    #[test]
    fn a_keyed_client_is_released_when_a_panic_unwinds_past_it() {
        let fake = FakeRigctld::start(FakeOpts::default());
        let client = RigctldClient::connect(&fake.addr).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let mut client = client;
            PttControl::set_ptt(&mut client, PttState::Tx).unwrap();
            panic!("boom");
        }));
        assert!(result.is_err());
        assert_eq!(fake.settled_log(), ["T 1", "T 0"]);
    }

    #[test]
    fn a_lost_reply_to_t1_still_counts_as_keyed() {
        use std::time::Duration;

        let fake = FakeRigctld::start(FakeOpts {
            ignore_t1: true,
            ..FakeOpts::default()
        });
        let mut client = RigctldClient::connect(&fake.addr).unwrap();
        client
            .stream
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        assert!(
            PttControl::set_ptt(&mut client, PttState::Tx).is_err(),
            "the `T 1` reply never arrives"
        );
        drop(client);
        assert_eq!(fake.settled_log(), ["T 1", "T 0"]);
    }

    #[test]
    fn a_keyed_client_whose_connection_died_reconnects_to_release_on_drop() {
        use std::sync::atomic::Ordering;

        let fake = FakeRigctld::start(FakeOpts {
            close_after_t1: true,
            ..FakeOpts::default()
        });
        let mut client = RigctldClient::connect(&fake.addr).unwrap();
        PttControl::set_ptt(&mut client, PttState::Tx).unwrap();
        // Wait until the fake has hung up on the keyed connection.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while fake.closed.load(Ordering::SeqCst) == 0 {
            assert!(std::time::Instant::now() < deadline, "fake never hung up");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        drop(client);
        assert_eq!(fake.settled_log(), ["T 1", "T 0"]);
        assert_eq!(
            fake.opened.load(Ordering::SeqCst),
            2,
            "the release went out on a fresh connection"
        );
    }

    #[test]
    fn a_closed_connection_is_an_error_not_an_empty_answer() {
        let fake = FakeRigctld::start(FakeOpts {
            close_after_t1: true,
            ..FakeOpts::default()
        });
        let mut client = RigctldClient::connect(&fake.addr).unwrap();
        PttControl::set_ptt(&mut client, PttState::Tx).unwrap();
        // The fake hung up after answering `T 1`; reading the next answer
        // hits EOF (or a reset), which must not read as an accepted `T 0`.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while fake.closed.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            assert!(std::time::Instant::now() < deadline, "fake never hung up");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(client.command("t").is_err());
        assert!(!client.connected, "the next command reconnects");
        // Unkeyed by a fresh connection, so the drop has nothing left to do.
        PttControl::set_ptt(&mut client, PttState::Rx).unwrap();
        drop(client);
        assert_eq!(fake.settled_log(), ["T 1", "T 0"]);
    }
}
