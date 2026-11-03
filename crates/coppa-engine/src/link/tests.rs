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
    fn new(a: LinkConfig, b: LinkConfig) -> Self {
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
