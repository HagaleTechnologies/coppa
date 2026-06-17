//! TNC (Terminal Node Controller) daemon mode.
//! Wires AFSK 1200 modem <-> CPAL audio <-> KISS TCP server <-> PTT control.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;

use coppa_radio::{NullPtt, PttControl, PttState, VoxPtt};

/// Configuration for TNC daemon mode.
pub struct TncConfig {
    pub kiss_port: u16,
    /// Address the KISS TCP server binds to. Defaults to "127.0.0.1" (loopback only).
    pub bind_address: String,
    pub rig_address: Option<String>,
    pub vox_mode: bool,
    pub sample_rate: u32,
}

impl Default for TncConfig {
    fn default() -> Self {
        Self {
            kiss_port: 8001,
            bind_address: "127.0.0.1".to_string(),
            rig_address: None,
            vox_mode: false,
            sample_rate: 48000,
        }
    }
}

/// Run the TNC daemon.
///
/// Wires together:
/// - KISS TCP server (accepts AX.25 frames from host apps)
/// - AFSK 1200 demodulator (decodes audio → AX.25 frames)
/// - AFSK 1200 modulator (encodes AX.25 frames → audio)
/// - PTT control (keys the radio during TX)
/// - CPAL audio I/O (optional, enabled with `cpal-backend` feature)
pub async fn run_tnc(config: TncConfig) -> Result<()> {
    println!(
        "TNC mode: KISS port {}, sample rate {} Hz",
        config.kiss_port, config.sample_rate
    );

    // -------------------------------------------------------------------------
    // 1. KISS server
    // -------------------------------------------------------------------------
    let mut kiss_server =
        coppa_host::kiss::KissServer::with_bind_addr(config.kiss_port, config.bind_address.clone());
    let tx_receiver = kiss_server
        .take_tx_receiver()
        .expect("take_tx_receiver called twice");
    let rx_sender = kiss_server.rx_sender();

    // Spawn the KISS TCP accept loop
    tokio::spawn(async move {
        if let Err(e) = kiss_server.start().await {
            eprintln!("KISS server error: {}", e);
        }
    });

    println!("KISS server listening on port {}", config.kiss_port);

    // -------------------------------------------------------------------------
    // 2. PTT
    // -------------------------------------------------------------------------
    let mut ptt: Box<dyn PttControl> = if config.vox_mode {
        println!("PTT: VOX mode");
        Box::new(VoxPtt::new())
    } else if let Some(ref addr) = config.rig_address {
        println!("PTT: rigctld at {}", addr);
        match coppa_radio::rigctld::RigctldClient::connect(addr) {
            Ok(client) => Box::new(client),
            Err(e) => {
                eprintln!(
                    "Failed to connect to rigctld ({}), falling back to NullPtt",
                    e
                );
                Box::new(NullPtt::new())
            }
        }
    } else {
        println!("PTT: null (no PTT configured)");
        Box::new(NullPtt::new())
    };

    // -------------------------------------------------------------------------
    // 3. Audio ring buffers
    // -------------------------------------------------------------------------
    let (audio_in_prod, audio_in_cons) = coppa_audio::audio_ring(65536);
    let (mut audio_out_prod, audio_out_cons) = coppa_audio::audio_ring(65536);

    // -------------------------------------------------------------------------
    // 4. Shutdown flag
    // -------------------------------------------------------------------------
    let shutdown = Arc::new(AtomicBool::new(false));

    // -------------------------------------------------------------------------
    // 5. Demod thread
    // -------------------------------------------------------------------------
    let (demod_tx, mut demod_rx) = mpsc::channel::<Vec<u8>>(64);
    let demod_shutdown = shutdown.clone();

    tokio::task::spawn_blocking(move || {
        let mut demod = coppa_codec::afsk::Demodulator::new();
        let mut audio_in = audio_in_cons;
        let mut buf = vec![0.0f32; 1024];

        loop {
            if demod_shutdown.load(Ordering::Acquire) || audio_in.is_abandoned() {
                break;
            }
            let n = audio_in.read(&mut buf);
            if n > 0 {
                demod.process(&buf[..n]);
                for frame in demod.take_frames() {
                    // Best-effort send; if the main loop is gone, stop.
                    if demod_tx.blocking_send(frame).is_err() {
                        return;
                    }
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    });

    // -------------------------------------------------------------------------
    // 6. CPAL audio I/O (optional)
    // -------------------------------------------------------------------------
    #[cfg(feature = "cpal-backend")]
    {
        use coppa_audio::{AudioSink, AudioSource};

        let sample_rate = config.sample_rate;
        let buf_size = 8192usize;

        // Input: CPAL mic → audio_in ring buffer
        let mut mic_prod = audio_in_prod;
        let input_shutdown = shutdown.clone();
        match coppa_audio::CpalSource::new(sample_rate, buf_size) {
            Ok(mut source) => {
                if let Err(e) = source.start() {
                    eprintln!("Failed to start audio input: {}", e);
                } else {
                    println!("Audio input started ({}Hz)", sample_rate);
                    tokio::task::spawn_blocking(move || {
                        let mut buf = vec![0.0f32; 1024];
                        loop {
                            if mic_prod.is_abandoned() || input_shutdown.load(Ordering::Acquire) {
                                break;
                            }
                            match source.read(&mut buf) {
                                Ok(n) if n > 0 => {
                                    mic_prod.write(&buf[..n]);
                                }
                                _ => {
                                    std::thread::sleep(std::time::Duration::from_millis(10));
                                }
                            }
                        }
                    });
                }
            }
            Err(e) => eprintln!("No audio input device: {}", e),
        }

        // Output: audio_out ring buffer → CPAL speaker
        let mut spk_cons = audio_out_cons;
        let output_shutdown = shutdown.clone();
        match coppa_audio::CpalSink::new(sample_rate, buf_size) {
            Ok(mut sink) => {
                if let Err(e) = sink.start() {
                    eprintln!("Failed to start audio output: {}", e);
                } else {
                    println!("Audio output started ({}Hz)", sample_rate);
                    tokio::task::spawn_blocking(move || {
                        let mut buf = vec![0.0f32; 1024];
                        loop {
                            if spk_cons.is_abandoned() || output_shutdown.load(Ordering::Acquire) {
                                break;
                            }
                            let n = spk_cons.read(&mut buf);
                            if n > 0 {
                                let _ = sink.write(&buf[..n]);
                            } else {
                                std::thread::sleep(std::time::Duration::from_millis(10));
                            }
                        }
                    });
                }
            }
            Err(e) => eprintln!("No audio output device: {}", e),
        }
    }

    // Suppress unused variable warnings when cpal-backend is not enabled.
    #[cfg(not(feature = "cpal-backend"))]
    {
        let _ = audio_in_prod;
        let _ = audio_out_cons;
    }

    // -------------------------------------------------------------------------
    // 7. Main event loop
    // -------------------------------------------------------------------------
    let mut tx_receiver = tx_receiver;

    println!("TNC ready.");

    loop {
        tokio::select! {
            // RX path: demod thread produced a frame → forward to KISS clients
            Some(frame) = demod_rx.recv() => {
                let _ = rx_sender.send(frame);
            }

            // TX path: KISS client sent a frame → modulate, key PTT, play audio
            Some(ax25_data) = tx_receiver.recv() => {
                let samples = coppa_codec::afsk::modulate(&ax25_data);
                let _ = ptt.set_ptt(PttState::Tx);
                audio_out_prod.write(&samples);
                let drain_ms = (samples.len() as u64 * 1000) / config.sample_rate as u64;
                tokio::time::sleep(Duration::from_millis(drain_ms + 50)).await;
                let _ = ptt.set_ptt(PttState::Rx);
            }

            // Ctrl-C: graceful shutdown
            _ = tokio::signal::ctrl_c() => {
                println!("\nTNC shutting down...");
                shutdown.store(true, Ordering::Release);
                break;
            }
        }
    }

    Ok(())
}
