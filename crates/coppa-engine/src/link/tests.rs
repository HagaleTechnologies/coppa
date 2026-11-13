use super::*;

/// 20 ms at 48 kHz: the virtual cable's block, as coppad's session tests use.
const BLOCK: usize = 960;
const TICK: Duration = Duration::from_millis(20);

/// Two links joined by a lockstep audio cable: each tick both stations get
/// the other's previous block (960 samples = 20 ms at 48 kHz). `drop_a`/
/// `drop_b` list 0-based frame indices (`Link::frames_transmitted()` before
/// the frame keyed) whose samples the cable replaces with silence.
struct Cable {
    a: Link,
    b: Link,
    /// An optional third station that hears A's audio only.
    observer: Option<Link>,
    t: Duration,
    base: Instant,
    a_out: Vec<f32>,
    b_out: Vec<f32>,
    drop_a: Vec<u64>,
    drop_b: Vec<u64>,
    a_dropping: bool,
    b_dropping: bool,
}

impl Cable {
    fn new(mut a: LinkConfig, mut b: LinkConfig) -> Self {
        // These frozen framing/count/timer regressions exercise the v1-only contract.
        // Floor-enabled pairs run through production audio in tests/floor_session.rs.
        a.robust_floor = false;
        b.robust_floor = false;
        Self {
            a: Link::new(a).expect("station A config"),
            b: Link::new(b).expect("station B config"),
            observer: None,
            t: Duration::ZERO,
            base: Instant::now(),
            a_out: vec![0.0; BLOCK],
            b_out: vec![0.0; BLOCK],
            drop_a: Vec::new(),
            drop_b: Vec::new(),
            a_dropping: false,
            b_dropping: false,
        }
    }

    /// The default pair: B listens, A will call it.
    fn pair() -> Self {
        let mut c = Self::new(LinkConfig::new("N0AAA"), LinkConfig::new("N0BBB"));
        c.b.set_listening(true);
        c
    }

    fn now(&self) -> Instant {
        self.base + self.t
    }

    /// Run one station for one tick, applying its drop list.
    fn step(
        link: &mut Link,
        now: Instant,
        rx: &[f32],
        drops: &[u64],
        dropping: &mut bool,
    ) -> Vec<f32> {
        let before = link.frames_transmitted();
        let mut out = vec![0.0; BLOCK];
        let active = link.process(now, rx, &mut out);
        if link.frames_transmitted() > before {
            *dropping = drops.contains(&before);
        }
        if *dropping {
            out.fill(0.0);
            if active < BLOCK {
                *dropping = false;
            }
        }
        out
    }

    fn tick(&mut self) {
        let now = self.now();
        let a_in = std::mem::take(&mut self.b_out);
        let b_in = std::mem::take(&mut self.a_out);
        if let Some(c) = self.observer.as_mut() {
            let mut out = vec![0.0; BLOCK];
            c.process(now, &b_in, &mut out);
        }
        self.a_out = Self::step(&mut self.a, now, &a_in, &self.drop_a, &mut self.a_dropping);
        self.b_out = Self::step(&mut self.b, now, &b_in, &self.drop_b, &mut self.b_dropping);
        self.t += TICK;
    }

    /// Advance tick by tick until `done(&a, &b)` or `limit` virtual time;
    /// returns whether done.
    fn run_until(&mut self, limit: Duration, done: impl Fn(&Link, &Link) -> bool) -> bool {
        let end = self.t + limit;
        while self.t < end {
            self.tick();
            if done(&self.a, &self.b) {
                return true;
            }
        }
        false
    }

    /// A calls B and both reach `Connected`.
    fn connect(&mut self) {
        let now = self.now();
        self.a.connect("N0BBB", now).expect("connect");
        assert!(
            self.run_until(Duration::from_secs(60), |a, b| {
                a.state() == LinkState::Connected && b.state() == LinkState::Connected
            }),
            "handshake did not complete"
        );
    }

    fn recv_all(link: &mut Link) -> Vec<u8> {
        let mut buf = vec![0u8; link.received_len()];
        let n = link.recv(&mut buf);
        buf.truncate(n);
        buf
    }
}

fn events(link: &mut Link) -> Vec<LinkEvent> {
    std::iter::from_fn(|| link.poll_event()).collect()
}

fn connected(remote: &str) -> LinkEvent {
    LinkEvent::Connected {
        remote: remote.to_owned(),
    }
}

fn disconnected(remote: &str, reason: DisconnectReason) -> LinkEvent {
    LinkEvent::Disconnected {
        remote: remote.to_owned(),
        reason,
    }
}

/// Send `data` from A and run until B holds it all and A has nothing pending.
fn deliver(c: &mut Cable, data: &[u8], limit: Duration) -> Vec<u8> {
    c.a.send(data).expect("send");
    let len = data.len();
    assert!(
        c.run_until(limit, |a, b| b.received_len() == len
            && a.pending_tx_bytes() == 0),
        "data not delivered: B holds {} of {len}, A pending {}",
        c.b.received_len(),
        c.a.pending_tx_bytes()
    );
    Cable::recv_all(&mut c.b)
}

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 37 + 11) as u8).collect()
}

#[test]
fn connect_and_send_delivers_bytes() {
    let mut c = Cable::pair();
    c.connect();
    assert_eq!(events(&mut c.a), vec![connected("N0BBB")]);
    assert_eq!(events(&mut c.b), vec![connected("N0AAA")]);
    assert_eq!(c.a.remote(), Some("N0BBB"));
    assert_eq!(c.b.remote(), Some("N0AAA"));

    let data = b"hello from A\x00\xff\xfe binary!!".to_vec();
    assert_eq!(data.len(), 24);
    let got = deliver(&mut c, &data, Duration::from_secs(30));
    assert_eq!(got, data);
    assert_eq!(c.a.pending_tx_bytes(), 0);
    // REQ, CFM, data from A; ACK (handshake), ACK (data) from B.
    assert_eq!(c.a.frames_transmitted(), 3);
    assert_eq!(c.b.frames_transmitted(), 2);
}

#[test]
fn multi_chunk_send_arrives_in_order() {
    let mut c = Cable::pair();
    c.connect();
    let data = payload(100);
    let got = deliver(&mut c, &data, Duration::from_secs(60));
    assert_eq!(got, data);
    // REQ, CFM, then 34 + 34 + 32 bytes in three data frames.
    assert_eq!(c.a.frames_transmitted(), 5);
    assert_eq!(c.b.frames_transmitted(), 4);
}

#[test]
fn lost_data_frame_is_retransmitted() {
    let mut c = Cable::pair();
    c.drop_a = vec![2];
    c.connect();
    let data = payload(20);
    let got = deliver(&mut c, &data, Duration::from_secs(60));
    assert_eq!(got, data);
    // REQ, CFM, data (dropped), then the retransmission: no ACK has arrived
    // yet, so the cached CFM goes first (D9), then the data again.
    assert_eq!(c.a.frames_transmitted(), 5);
    assert_eq!(c.a.state(), LinkState::Connected);
}

#[test]
fn lost_connect_req_is_retried() {
    let mut c = Cable::pair();
    c.drop_a = vec![0];
    c.connect();
    // REQ (dropped), REQ, CFM.
    assert_eq!(c.a.frames_transmitted(), 3);
    assert_eq!(c.b.frames_transmitted(), 1);
    assert_eq!(events(&mut c.a), vec![connected("N0BBB")]);
    assert_eq!(events(&mut c.b), vec![connected("N0AAA")]);
}

#[test]
fn lost_connect_ack_gets_a_fresh_ack() {
    let mut c = Cable::pair();
    c.drop_b = vec![0];
    c.connect();
    // B's first ACK is lost; A's REQ retry finds B accepting and B re-ACKs.
    assert_eq!(c.a.frames_transmitted(), 3);
    assert_eq!(c.b.frames_transmitted(), 2);
    assert_eq!(events(&mut c.b), vec![connected("N0AAA")]);
}

#[test]
fn lost_connect_cfm_is_resent_before_data_retransmit() {
    let mut c = Cable::pair();
    c.drop_a = vec![1];
    let now = c.now();
    c.a.connect("N0BBB", now).expect("connect");
    assert!(c.run_until(Duration::from_secs(30), |a, _| a.state()
        == LinkState::Connected));
    // B never heard the CFM: it is still answering the handshake.
    let data = payload(24);
    c.a.send(&data).expect("send");
    assert!(
        c.run_until(Duration::from_secs(60), |a, b| b.received_len() == 24
            && a.pending_tx_bytes() == 0)
    );
    assert_eq!(Cable::recv_all(&mut c.b), data);
    // REQ, CFM (dropped), data (B drops it while accepting), CFM, data.
    assert_eq!(c.a.frames_transmitted(), 5);
    assert_eq!(c.b.state(), LinkState::Connected);
}

#[test]
fn handshake_fails_when_nobody_answers() {
    let mut a_cfg = LinkConfig::new("N0AAA");
    a_cfg.connect_retries = 1;
    a_cfg.initial_rto = Duration::from_secs(4);
    let mut c = Cable::new(a_cfg, LinkConfig::new("N0BBB"));
    c.b.set_listening(false);
    let now = c.now();
    c.a.connect("N0BBB", now).expect("connect");
    assert!(c.run_until(Duration::from_secs(30), |a, _| a.state() == LinkState::Idle));
    assert_eq!(c.a.frames_transmitted(), 2, "1 + connect_retries REQs");
    assert_eq!(
        events(&mut c.a),
        vec![disconnected("N0BBB", DisconnectReason::HandshakeFailed)]
    );
    assert_eq!(c.b.frames_transmitted(), 0);
    assert!(events(&mut c.b).is_empty());
}

#[test]
fn retries_exhausted_ends_the_link() {
    let mut a_cfg = LinkConfig::new("N0AAA");
    a_cfg.max_retransmit = 1;
    a_cfg.initial_rto = Duration::from_secs(4);
    let mut c = Cable::new(a_cfg, LinkConfig::new("N0BBB"));
    c.b.set_listening(true);
    // B's handshake ACK gets through; every later frame from B is lost.
    c.drop_b = (1..64).collect();
    c.connect();
    events(&mut c.a);
    events(&mut c.b);
    c.a.send(&payload(10)).expect("send");
    assert!(
        c.run_until(Duration::from_secs(60), |a, b| a.state() == LinkState::Idle
            && b.state() == LinkState::Idle)
    );
    assert_eq!(
        events(&mut c.a),
        vec![disconnected("N0BBB", DisconnectReason::RetriesExhausted)]
    );
    assert_eq!(
        events(&mut c.b),
        vec![disconnected("N0AAA", DisconnectReason::Remote)]
    );
    // REQ, CFM, data, CFM + data (the one retransmission), DISCONNECT.
    assert_eq!(c.a.frames_transmitted(), 6);
}

#[test]
fn graceful_disconnect_waits_for_queued_data() {
    let mut c = Cable::pair();
    c.connect();
    events(&mut c.a);
    events(&mut c.b);
    let data = payload(50);
    c.a.send(&data).expect("send");
    let now = c.now();
    c.a.disconnect(now).expect("disconnect");
    assert_eq!(c.a.state(), LinkState::Disconnecting);
    assert!(matches!(
        c.a.send(b"late"),
        Err(LinkError::NotConnected {
            state: LinkState::Disconnecting
        })
    ));
    assert!(
        c.run_until(Duration::from_secs(60), |a, b| a.state() == LinkState::Idle
            && b.state() == LinkState::Idle)
    );
    assert_eq!(Cable::recv_all(&mut c.b), data);
    assert_eq!(
        events(&mut c.a),
        vec![disconnected("N0BBB", DisconnectReason::Local)]
    );
    assert_eq!(
        events(&mut c.b),
        vec![disconnected("N0AAA", DisconnectReason::Remote)]
    );
}

#[test]
fn keepalive_is_sent_when_idle() {
    let mut a_cfg = LinkConfig::new("N0AAA");
    a_cfg.keepalive_interval = Duration::from_secs(3);
    a_cfg.session_timeout = Duration::from_secs(8);
    let mut b_cfg = LinkConfig::new("N0BBB");
    b_cfg.keepalive_interval = Duration::from_secs(3);
    b_cfg.session_timeout = Duration::from_secs(8);
    let mut c = Cable::new(a_cfg, b_cfg);
    c.b.set_listening(true);
    c.connect();
    let (a0, b0) = (c.a.frames_transmitted(), c.b.frames_received());
    // Run well past the session timeout with no data: the keepalives keep
    // both ends connected (a received keepalive refreshes the peer's
    // session, a sent one refreshes the sender's, as in coppad).
    assert!(!c.run_until(Duration::from_secs(20), |a, b| a.state()
        != LinkState::Connected
        || b.state() != LinkState::Connected));
    assert!(
        c.a.frames_transmitted() >= a0 + 2,
        "A sent no keepalives: {} -> {}",
        a0,
        c.a.frames_transmitted()
    );
    assert!(c.b.frames_received() > b0, "B heard none of them");
    assert_eq!(c.a.state(), LinkState::Connected);
    assert_eq!(c.b.state(), LinkState::Connected);
}

#[test]
fn frames_for_other_stations_are_ignored() {
    let mut c = Cable::pair();
    let mut observer = Link::new(LinkConfig::new("N0CCC")).expect("config");
    observer.set_listening(true);
    c.observer = Some(observer);
    c.connect();
    let observer = c.observer.as_mut().expect("observer");
    assert!(observer.frames_received() >= 1, "C decoded nothing");
    assert_eq!(observer.state(), LinkState::Idle);
    assert_eq!(observer.frames_transmitted(), 0);
    assert!(observer.poll_event().is_none());
}

#[test]
fn api_state_errors() {
    let now = Instant::now();
    let mut link = Link::new(LinkConfig::new("N0AAA")).expect("config");
    assert_eq!(link.state(), LinkState::Idle);
    let err = link.send(b"x").unwrap_err();
    assert!(matches!(
        err,
        LinkError::NotConnected {
            state: LinkState::Idle
        }
    ));
    assert_eq!(err.to_string(), "link is not connected (state IDLE)");
    assert!(matches!(
        link.disconnect(now),
        Err(LinkError::NotConnected { .. })
    ));
    link.connect("N0BBB", now).expect("connect");
    assert_eq!(link.state(), LinkState::Connecting);
    let err = link.connect("N0BBB", now).unwrap_err();
    assert!(matches!(
        err,
        LinkError::NotIdle {
            state: LinkState::Connecting
        }
    ));
    assert_eq!(err.to_string(), "link is not idle (state CONNECTING)");

    let err = Link::new(LinkConfig::new("N0 CALL")).unwrap_err();
    assert!(matches!(err, LinkError::InvalidCallsign { .. }));
    assert!(
        err.to_string().contains("Invalid callsign character: ' '"),
        "{err}"
    );
    assert_eq!(
        err.to_string(),
        "invalid callsign \"N0 CALL\": Invalid callsign character: ' '"
    );

    let mut idle = Link::new(LinkConfig::new("N0AAA")).expect("config");
    let err = idle.connect("TOOLONGCALL", now).unwrap_err();
    assert!(matches!(err, LinkError::InvalidCallsign { .. }));
    assert!(
        err.to_string()
            .contains("Callsign too long: 11 chars (max 8)"),
        "{err}"
    );
    assert_eq!(idle.state(), LinkState::Idle);

    let mut bad_level = LinkConfig::new("N0AAA");
    bad_level.engine.speed_level = 77;
    assert!(matches!(
        Link::new(bad_level),
        Err(LinkError::Engine(EngineError::InvalidSpeedLevel {
            level: 77
        }))
    ));

    for (state, text) in [
        (LinkState::Idle, "IDLE"),
        (LinkState::Connecting, "CONNECTING"),
        (LinkState::Connected, "CONNECTED"),
        (LinkState::Disconnecting, "DISCONNECTING"),
    ] {
        assert_eq!(state.to_string(), text);
    }
}

#[test]
fn process_writes_signal_as_prefix_and_zero_fills() {
    let base = Instant::now();
    let mut link = Link::new(LinkConfig::new("N0AAA")).expect("config");
    link.connect("N0BBB", base).expect("connect");
    let mut tx = [1.0f32; BLOCK];
    let first = link.process(base, &[], &mut tx);
    assert_eq!(first, BLOCK);
    assert!(tx.iter().any(|&s| s != 0.0), "no signal written");
    assert!(link.is_transmitting());
    let mut total = first;
    let mut tick = 1u64;
    let last = loop {
        let mut tx = [1.0f32; BLOCK];
        let n = link.process(base + TICK * tick as u32, &[], &mut tx);
        total += n;
        tick += 1;
        if n < BLOCK {
            assert!(tx[n..].iter().all(|&s| s == 0.0), "tail not zeroed");
            break n;
        }
    };
    // A level-1 frame is 65,520 samples: 68 full blocks and 240 samples.
    assert_eq!(total, 65_520);
    assert_eq!(last, 65_520 % BLOCK);
    assert!(!link.is_transmitting());
    let mut tx = [1.0f32; BLOCK];
    assert_eq!(link.process(base + TICK * tick as u32, &[], &mut tx), 0);
    assert!(tx.iter().all(|&s| s == 0.0));
}

#[test]
fn rx_is_ignored_while_transmitting() {
    // A frame from B to A, encoded outside any link.
    let peer = CoppaCore::new();
    let pdu = MacPdu::new(
        MacFrameType::Keepalive,
        Callsign::new("N0AAA").expect("callsign"),
        Callsign::new("N0BBB").expect("callsign"),
        0,
        vec![],
    );
    let mut frame = peer.encode_bytes(&pdu.to_bytes()).expect("encode");
    frame.resize(frame.len().div_ceil(BLOCK) * BLOCK + 20 * BLOCK, 0.0);

    let base = Instant::now();
    let mut a = Link::new(LinkConfig::new("N0AAA")).expect("config");
    a.connect("N0BBB", base).expect("connect");
    let mut tx = [0.0f32; BLOCK];
    let mut t = 0u32;
    a.process(base, &[], &mut tx);
    // Feed the whole frame while A is keyed: none of it is decoded.
    for block in frame.chunks(BLOCK) {
        t += 1;
        a.process(base + TICK * t, block, &mut tx);
    }
    assert_eq!(a.frames_received(), 0);
    assert!(!a.is_transmitting());
    // The same frame after A unkeys is decoded.
    for block in frame.chunks(BLOCK) {
        t += 1;
        a.process(base + TICK * t, block, &mut tx);
    }
    assert_eq!(a.frames_received(), 1);
}

#[test]
fn link_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Link>();
}

#[test]
fn connect_retry_waits_for_slow_turnaround_ack() {
    let mut a = LinkConfig::new("N0AAA");
    a.turnaround = Duration::from_secs(3);
    let mut b = LinkConfig::new("N0BBB");
    b.turnaround = a.turnaround;
    let mut c = Cable::new(a, b);
    c.b.set_listening(true);
    c.connect();
    assert_eq!(c.a.frames_transmitted(), 2, "only REQ and CFM, no retry");
    assert_eq!(c.b.frames_transmitted(), 1, "only CONNECT_ACK");
}

#[test]
fn peer_frames_must_match_session_ssid() {
    let now = Instant::now();
    let mut a = Link::new(LinkConfig::new("N0AAA")).unwrap();
    let mut b = Link::new(LinkConfig::new("N0BBB")).unwrap();
    b.set_listening(true);
    a.connect("N0BBB", now).unwrap();
    let req = MacPdu::from_bytes(&a.queue.pop_front().unwrap().bytes).unwrap();
    b.handle_pdu(req.clone(), now);
    let mut foreign_req = req.clone();
    foreign_req.ssid = 1;
    b.handle_pdu(foreign_req, now);
    assert_eq!(
        b.session.as_ref().unwrap().ssid,
        0,
        "foreign REQ replaced session"
    );
    // A repeat on the same SSID still gets a fresh ACK.
    b.handle_pdu(req, now);
    let ack = MacPdu::from_bytes(&b.queue.pop_front().unwrap().bytes).unwrap();
    a.handle_pdu(ack, now);
    let cfm = MacPdu::from_bytes(&a.queue.pop_front().unwrap().bytes).unwrap();
    b.handle_pdu(cfm, now);
    assert_eq!(b.state(), LinkState::Connected);

    let data = session_data_frame(a.session.as_ref().unwrap(), 0, b"hello");
    let mut foreign_data = MacPdu::from_bytes(&data).unwrap();
    foreign_data.ssid = 1;
    b.handle_pdu(foreign_data, now);
    assert_eq!(b.received_len(), 0, "foreign DATA delivered");
    b.handle_pdu(MacPdu::from_bytes(&data).unwrap(), now);
    assert_eq!(Cable::recv_all(&mut b), b"hello");

    // Put the matching segment in flight so a wrong-SSID ACK could discard it.
    a.arq
        .as_mut()
        .unwrap()
        .tx
        .send(b"hello".to_vec(), now)
        .unwrap();
    let ack = session_ack_frame(b.session.as_ref().unwrap(), &b.arq.as_ref().unwrap().rx);
    let mut foreign_ack = MacPdu::from_bytes(&ack).unwrap();
    foreign_ack.ssid = 1;
    a.handle_pdu(foreign_ack, now);
    assert_eq!(
        a.arq.as_ref().unwrap().tx.in_flight(),
        1,
        "foreign ACK accepted"
    );
    a.handle_pdu(MacPdu::from_bytes(&ack).unwrap(), now);
    assert_eq!(a.arq.as_ref().unwrap().tx.in_flight(), 0);

    let mut disconnect = b.session.as_mut().unwrap().disconnect(now).unwrap();
    disconnect.ssid = 1;
    a.handle_pdu(disconnect.clone(), now);
    assert_eq!(
        a.state(),
        LinkState::Connected,
        "foreign DISCONNECT accepted"
    );
    disconnect.ssid = 0;
    a.handle_pdu(disconnect, now);
    assert_eq!(a.state(), LinkState::Idle);
}

/// Codex P2 on PR #210: with an audio-sized `tx` buffer the DISCONNECT
/// waveform spans many `process` calls, and `Disconnected` must not be
/// reported (nor the link go `Idle`) until its last sample has been returned.
#[test]
fn local_disconnect_is_reported_after_disconnect_frame_finishes() {
    let mut ticks_while_sending_disconnect = 0;
    let mut seen = Vec::new();
    let mut c = Cable::pair();
    c.connect();
    events(&mut c.a);
    events(&mut c.b);
    let now = c.now();
    c.a.disconnect(now).expect("disconnect");
    let end = c.t + Duration::from_secs(60);
    while c.t < end && seen.is_empty() {
        c.tick();
        seen = events(&mut c.a);
        if c.a.is_transmitting() {
            ticks_while_sending_disconnect += 1;
            assert!(seen.is_empty(), "Disconnected reported mid-DISCONNECT");
            assert_eq!(c.a.state(), LinkState::Disconnecting);
        }
    }
    assert_eq!(seen, vec![disconnected("N0BBB", DisconnectReason::Local)]);
    assert!(!c.a.is_transmitting());
    assert_eq!(c.a.state(), LinkState::Idle);
    assert!(
        ticks_while_sending_disconnect > 1,
        "DISCONNECT should span several 20 ms blocks"
    );
    // The peer heard the whole frame.
    assert!(c.run_until(Duration::from_secs(10), |_, b| b.state() == LinkState::Idle));
    assert_eq!(
        events(&mut c.b),
        vec![disconnected("N0AAA", DisconnectReason::Remote)]
    );
}

/// The same hold applies to the best-effort DISCONNECT after retry
/// exhaustion.
#[test]
fn retries_exhausted_is_reported_after_disconnect_frame_finishes() {
    let mut a_cfg = LinkConfig::new("N0AAA");
    a_cfg.max_retransmit = 1;
    a_cfg.initial_rto = Duration::from_secs(4);
    let mut c = Cable::new(a_cfg, LinkConfig::new("N0BBB"));
    c.b.set_listening(true);
    c.drop_b = (1..64).collect();
    c.connect();
    events(&mut c.a);
    c.a.send(&payload(10)).expect("send");
    let mut seen = Vec::new();
    let end = c.t + Duration::from_secs(60);
    while c.t < end && seen.is_empty() {
        c.tick();
        seen = events(&mut c.a);
        if c.a.is_transmitting() {
            assert!(seen.is_empty(), "Disconnected reported mid-frame");
        }
    }
    assert_eq!(
        seen,
        vec![disconnected("N0BBB", DisconnectReason::RetriesExhausted)]
    );
    assert_eq!(c.a.state(), LinkState::Idle);
    assert_eq!(c.a.frames_transmitted(), 6);
}

#[test]
fn floor_two_window_fallback() {
    let mut link = Link::new(LinkConfig::new("N0AAA")).unwrap();
    let base = Instant::now();
    link.connect("N0BBB", base).unwrap();
    let mut out = vec![0.0; 8 * 48_000];
    link.process(base, &[], &mut out);
    assert_eq!(link.frames_transmitted(), 1);
    link.process(base + Duration::from_millis(4999), &[], &mut out);
    assert_eq!(link.frames_transmitted(), 1);
    link.process(base + Duration::from_secs(5), &[], &mut out);
    assert_eq!(link.frames_transmitted(), 2);
    assert!(matches!(link.phy_mode().0, Some(LinkPhyMode::Ofdm { .. })));
    link.process(base + Duration::from_millis(9999), &[], &mut out);
    assert_eq!(link.frames_transmitted(), 2);
    link.process(base + Duration::from_secs(10), &[], &mut out);
    assert_eq!(link.frames_transmitted(), 3);
    assert_eq!(link.phy_mode().0, Some(LinkPhyMode::Floor));
    link.process(base + Duration::from_millis(28999), &[], &mut out);
    assert_eq!(link.frames_transmitted(), 3);
    link.process(base + Duration::from_secs(29), &[], &mut out);
    assert_eq!(link.frames_transmitted(), 4);
}

#[test]
fn floor_rto_covers_configured_turnaround() {
    let mut config = LinkConfig::new("N0AAA");
    config.turnaround = Duration::from_secs(5);
    let mut link = Link::new(config).unwrap();
    let now = Instant::now();
    let mut session = link.new_session(Callsign::new("N0BBB").unwrap(), 0, now);
    session.negotiated = Some(LinkCapabilities {
        robust_floor: true,
        ..Default::default()
    });
    link.session = Some(session);
    link.established();
    assert!(link.arq.as_ref().unwrap().tx.rto() >= Duration::from_secs(29));
    link.floor.received(LinkPhyMode::Floor);
    assert!(link.key(
        QueuedFrame {
            kind: FrameKind::Data,
            bytes: vec![0; 21],
            mode: None
        },
        now
    ));
    assert!(link.arq.as_ref().unwrap().tx.rto() >= Duration::from_secs(29));
}

#[test]
fn floor_keepalive_precedes_configured_timeout() {
    let mut a = LinkConfig::new("N0AAA");
    a.keepalive_interval = Duration::from_secs(5);
    a.session_timeout = Duration::from_secs(30);
    let mut b = a.clone();
    b.callsign = "N0BBB".to_owned();
    // Unlike Cable::new's frozen v1 fixtures, exercise default floor negotiation.
    let mut c = Cable::new(a.clone(), b.clone());
    c.a = Link::new(a).unwrap();
    c.b = Link::new(b).unwrap();
    c.b.set_listening(true);
    c.connect();
    c.run_until(Duration::from_secs(90), |_, _| false);
    assert_eq!(c.a.state(), LinkState::Connected);
    assert_eq!(c.b.state(), LinkState::Connected);
}

#[test]
fn floor_answerer_retries_before_handshake_timeout() {
    let mut link = Link::new(LinkConfig::new("N0AAD")).unwrap();
    let base = Instant::now();
    link.connect("N0AAA", base).unwrap();
    let mut out = vec![0.0; 8 * 48_000];
    for seconds in [0, 29, 58] {
        link.process(base + Duration::from_secs(seconds), &[], &mut out);
    }
    assert_eq!(link.frames_transmitted(), 3);
    assert_eq!(link.phy_mode().0, Some(LinkPhyMode::Floor));
    link.process(base + Duration::from_secs(77), &[], &mut out);
    // Stagger discovery, not floor retries: the 19-second floor response
    // window must expire before the 30-second idle handshake timeout.
    assert_eq!(link.frames_transmitted(), 4);
    assert_eq!(link.state(), LinkState::Connecting);
}

#[test]
fn floor_lost_confirmation_recovers_before_accepting_timeout() {
    let a = LinkConfig::new("N0AAA");
    let b = LinkConfig::new("N0BBB");
    let mut c = Cable::new(a.clone(), b.clone());
    c.a = Link::new(a).unwrap();
    c.b = Link::new(b).unwrap();
    c.b.set_listening(true);
    // Erase two OFDM requests and the first floor confirmation.
    c.drop_a = vec![0, 1, 3];
    c.a.connect("N0BBB", c.now()).unwrap();
    assert!(c.run_until(Duration::from_secs(60), |a, _| {
        a.state() == LinkState::Connected
    }));
    c.a.send(b"recover").unwrap();
    assert!(c.run_until(Duration::from_secs(80), |a, b| {
        a.pending_tx_bytes() == 0 && b.received_len() == 7
    }));
    assert_eq!(Cable::recv_all(&mut c.b), b"recover");
    assert_eq!(c.a.state(), LinkState::Connected);
    assert_eq!(c.b.state(), LinkState::Connected);
}

#[test]
fn floor_stagger_allows_configured_retry_window() {
    for (rto, turnaround, first_retry) in [(10, 500, 39), (6, 500, 31), (5, 1000, 30)] {
        let mut config = LinkConfig::new("N0AAD");
        config.initial_rto = Duration::from_secs(rto);
        config.turnaround = Duration::from_millis(turnaround);
        let mut link = Link::new(config).unwrap();
        let base = Instant::now();
        link.connect("N0AAA", base).unwrap();
        let mut out = vec![0.0; 8 * 48_000];
        for seconds in 0..=2 * first_retry {
            link.process(base + Duration::from_secs(seconds), &[], &mut out);
            assert_eq!(link.state(), LinkState::Connecting, "at {seconds}s");
            assert!(link.poll_event().is_none(), "premature event at {seconds}s");
            let expected_frames = if seconds < first_retry {
                1
            } else if seconds < 2 * first_retry {
                2
            } else {
                3
            };
            assert_eq!(link.frames_transmitted(), expected_frames, "at {seconds}s");
        }
        assert_eq!(link.frames_transmitted(), 3);
        assert_eq!(link.phy_mode().0, Some(LinkPhyMode::Floor));
    }
}

#[test]
fn floor_stagger_exhausts_configured_retries() {
    for (rto, turnaround, first_retry, floor_window) in [(6, 500, 31, 19), (5, 1000, 30, 20)] {
        for retries in [3, 0] {
            let mut config = LinkConfig::new("N0AAD");
            config.initial_rto = Duration::from_secs(rto);
            config.turnaround = Duration::from_millis(turnaround);
            config.connect_retries = retries;
            let mut link = Link::new(config).unwrap();
            let base = Instant::now();
            link.connect("N0AAA", base).unwrap();
            let mut out = vec![0.0; 8 * 48_000];
            let failure_at = if retries == 0 {
                first_retry
            } else {
                2 * first_retry + 2 * floor_window
            };
            let mut observed_events = Vec::new();
            // Keep ticking after failure to catch extra requests or disconnects.
            for seconds in 0..=failure_at + 5 {
                link.process(base + Duration::from_secs(seconds), &[], &mut out);
                observed_events.extend(events(&mut link));
                if seconds < failure_at {
                    assert_eq!(link.state(), LinkState::Connecting, "at {seconds}s");
                    assert!(observed_events.is_empty(), "premature event at {seconds}s");
                    let expected_frames = if seconds < first_retry {
                        1
                    } else if seconds < 2 * first_retry {
                        2
                    } else if seconds < 2 * first_retry + floor_window {
                        3
                    } else {
                        4
                    };
                    assert_eq!(link.frames_transmitted(), expected_frames, "at {seconds}s");
                    let expected_mode = if seconds < 2 * first_retry {
                        LinkPhyMode::Ofdm { level: 1 }
                    } else {
                        LinkPhyMode::Floor
                    };
                    assert_eq!(link.phy_mode().0, Some(expected_mode), "at {seconds}s");
                } else {
                    assert_eq!(link.state(), LinkState::Idle, "at {seconds}s");
                    assert_eq!(link.frames_transmitted(), u64::from(retries) + 1);
                    assert_eq!(
                        observed_events,
                        vec![disconnected("N0AAA", DisconnectReason::HandshakeFailed)]
                    );
                }
            }
        }
    }
}
