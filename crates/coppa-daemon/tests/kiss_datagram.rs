//! COP-138: real TCP KISS clients across two OFDM event loops on a half-duplex virtual cable.
//! Virtual time does not exercise real-time decoder stalls; tools/reticulum-loop does.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use coppa_audio::{AudioRingConsumer, AudioRingProducer};
use coppa_daemon::config::DaemonConfig;
use coppa_daemon::event_loop::{forward_data_out, EventLoop};
use coppa_daemon::vara_wiring::serve_vara;
use coppa_host::kiss::{kiss_encode_data, KissDecoder, KissFrame};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// One cable tick: 20 ms of 48 kHz audio.
const CABLE_TICK: Duration = Duration::from_millis(20);
const CABLE_SAMPLES: usize = 960;

/// A running station: its ports, and its keyed state as seen on the command
/// port by an always-reading monitor client.
struct Station {
    kiss_addr: std::net::SocketAddr,
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
    let kiss_listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let kiss_addr = kiss_listener.local_addr().unwrap();
    let data_addr = data_listener.local_addr().unwrap();

    let mut config = DaemonConfig::default();
    config.engine.callsign = call.into();
    config.engine.profile = "HF_STANDARD".into();
    config.host.vara_enabled = true;
    config.host.vara_command_port = cmd_addr.port();
    config.host.vara_data_port = data_addr.port();
    config.host.kiss_enabled = true;
    config.host.kiss_port = kiss_addr.port();
    let host = config.host.clone();

    let (in_producer, in_consumer) = coppa_audio::audio_ring(config.audio.buffer_size);
    let (out_producer, out_consumer) = coppa_audio::audio_ring(config.audio.buffer_size);

    let mut event_loop = EventLoop::new(config).unwrap();
    let (response_tx, response_rx) = tokio::sync::mpsc::unbounded_channel();
    event_loop.set_response_tx(response_tx);
    event_loop.set_audio_in(in_consumer);
    event_loop.set_audio_out(out_producer);
    coppa_daemon::kiss_wiring::serve_kiss(&mut event_loop, kiss_listener, &host);
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
            kiss_addr,
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

fn payload(len: usize, seed: u8) -> Vec<u8> {
    let mut data: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_add(seed)).collect();
    data[..7].copy_from_slice(&[0, 0, 0xC0, 0xDB, 0xDC, 0xDD, 0]);
    data
}

async fn read_frame(stream: &mut TcpStream) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(60), async {
        let mut decoder = KissDecoder::new();
        let mut bytes = [0; 2048];
        loop {
            let n = stream.read(&mut bytes).await.unwrap();
            assert_ne!(n, 0, "KISS client closed");
            let frames = decoder.push(&bytes[..n]);
            if !frames.is_empty() {
                assert_eq!(frames.len(), 1, "one datagram is one frame");
                if let KissFrame::Data(data) = frames.into_iter().next().unwrap() {
                    return data;
                }
                panic!("expected a KISS data frame");
            }
        }
    })
    .await
    .expect("peer received no datagram within 60 virtual seconds")
}

async fn assert_silent(stream: &mut TcpStream) {
    let mut byte = [0];
    assert!(
        tokio::time::timeout(Duration::from_secs(20), stream.read(&mut byte))
            .await
            .is_err(),
        "unexpected host delivery"
    );
}

#[tokio::test(start_paused = true)]
async fn a_reticulum_datagram_crosses_the_hf_modem() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (a, aa) = station("N1CALL").await;
            let (b, ba) = station("N0CALL").await;
            spawn_cable(&a, aa, &b, ba);
            let mut tx = TcpStream::connect(a.kiss_addr).await.unwrap();
            let mut rx = TcpStream::connect(b.kiss_addr).await.unwrap();
            let mut local = TcpStream::connect(a.kiss_addr).await.unwrap();
            let mut vara = TcpStream::connect(b.data_addr).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            let data = payload(403, 3);
            tx.write_all(&kiss_encode_data(&data)).await.unwrap();
            assert_eq!(read_frame(&mut rx).await, data);
            assert_silent(&mut rx).await;
            assert_silent(&mut local).await;
            assert_silent(&mut vara).await;
        })
        .await;
}

#[tokio::test(start_paused = true)]
async fn a_full_reticulum_mtu_datagram_crosses_both_ways() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (a, aa) = station("N1CALL").await;
            let (b, ba) = station("N0CALL").await;
            spawn_cable(&a, aa, &b, ba);
            let mut ac = TcpStream::connect(a.kiss_addr).await.unwrap();
            let mut bc = TcpStream::connect(b.kiss_addr).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            let data = payload(500, 11);
            ac.write_all(&kiss_encode_data(&data)).await.unwrap();
            assert_eq!(read_frame(&mut bc).await, data);
            tokio::time::sleep(Duration::from_secs(2)).await;
            let reply = payload(500, 71);
            bc.write_all(&kiss_encode_data(&reply)).await.unwrap();
            assert_eq!(read_frame(&mut ac).await, reply);
        })
        .await;
}

#[tokio::test(start_paused = true)]
async fn a_kiss_frame_split_across_tcp_writes_crosses_whole() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (a, aa) = station("N1CALL").await;
            let (b, ba) = station("N0CALL").await;
            spawn_cable(&a, aa, &b, ba);
            let mut tx = TcpStream::connect(a.kiss_addr).await.unwrap();
            let mut rx = TcpStream::connect(b.kiss_addr).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            let data = payload(500, 9);
            let wire = kiss_encode_data(&data);
            let split = wire.iter().position(|b| *b == 0xDB).unwrap() + 1;
            tx.write_all(&wire[..split]).await.unwrap();
            tx.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.write_all(&wire[split..]).await.unwrap();
            assert_eq!(read_frame(&mut rx).await, data);
        })
        .await;
}
