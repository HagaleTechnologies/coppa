//! Throwaway research harness for COP-97 (NOT committed as a product artifact;
//! this container is ephemeral). Speaks an ARDOP client's command sequence at
//! coppa's REAL `serve_vara` wiring over real TCP, and records exactly what the
//! current VARA-only host interface answers, so the COP-97 gap is demonstrated
//! against the running code rather than inferred from source.

use std::time::Duration;

use coppa_daemon::config::DaemonConfig;
use coppa_daemon::event_loop::EventLoop;
use coppa_daemon::vara_wiring::serve_vara;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::{TcpListener, TcpStream};

/// Read reply lines until `quiet` elapses with nothing new.
async fn drain_lines(reader: &mut BufReader<OwnedReadHalf>, quiet: Duration) -> Vec<String> {
    let mut lines = Vec::new();
    loop {
        let mut buf = Vec::new();
        match tokio::time::timeout(quiet, reader.read_until(b'\r', &mut buf)).await {
            Err(_) => break, // quiet period: no more replies pending
            Ok(Ok(0)) => {
                lines.push("<connection closed>".into());
                break;
            }
            Ok(Ok(_)) => {
                let s = String::from_utf8_lossy(&buf).trim_end().to_string();
                lines.push(s);
            }
            Ok(Err(e)) => {
                lines.push(format!("<read error: {e}>"));
                break;
            }
        }
    }
    lines
}

async fn send_and_read(
    reader: &mut BufReader<OwnedReadHalf>,
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    label: &str,
    line: &str,
) {
    writer
        .write_all(format!("{line}\r").as_bytes())
        .await
        .expect("write to command port");
    let replies = drain_lines(reader, Duration::from_millis(400)).await;
    println!("{label:18} -> {replies:?}");
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_target(false)
        .init();

    let cmd_listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let data_listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let cmd_addr = cmd_listener.local_addr().unwrap();
    let data_addr = data_listener.local_addr().unwrap();

    let mut config = DaemonConfig::default();
    config.engine.callsign = "N1CALL".into();
    config.host.vara_enabled = true;
    config.host.vara_command_port = cmd_addr.port();
    config.host.vara_data_port = data_addr.port();
    let host = config.host.clone();

    let mut event_loop = EventLoop::new(config).unwrap();
    let (response_tx, response_rx) = tokio::sync::mpsc::unbounded_channel();
    event_loop.set_response_tx(response_tx);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            serve_vara(
                &mut event_loop,
                response_rx,
                cmd_listener,
                data_listener,
                &host,
            );
            tokio::task::spawn_local(async move {
                let _ = event_loop.run().await;
            });

            let cmd = TcpStream::connect(cmd_addr).await.unwrap();
            let _data = TcpStream::connect(data_addr).await.unwrap();
            let (read_half, mut write_half) = cmd.into_split();
            let mut reader = BufReader::new(read_half);

            // --- What the port greets with today (VARA convention) ---
            let greeting = drain_lines(&mut reader, Duration::from_millis(400)).await;
            println!("on connect       -> {greeting:?}  (ARDOP TNCs send no greeting)");

            // --- The exact init sequence Pat's ardop transport sends ---
            // (wl2k-go/transport/ardop/tnc.go open()/init(); uppercase + \r)
            send_and_read(&mut reader, &mut write_half, "INITIALIZE", "INITIALIZE").await;
            send_and_read(&mut reader, &mut write_half, "STATE", "STATE").await;
            send_and_read(&mut reader, &mut write_half, "CODEC TRUE", "CODEC TRUE").await;
            send_and_read(
                &mut reader,
                &mut write_half,
                "PROTOCOLMODE ARQ",
                "PROTOCOLMODE ARQ",
            )
            .await;
            send_and_read(&mut reader, &mut write_half, "ARQTIMEOUT 90", "ARQTIMEOUT 90").await;
            send_and_read(&mut reader, &mut write_half, "LISTEN FALSE", "LISTEN FALSE").await;
            send_and_read(&mut reader, &mut write_half, "ARQBW 500MAX", "ARQBW 500MAX").await;
            send_and_read(
                &mut reader,
                &mut write_half,
                "GRIDSQUARE FN31PR",
                "GRIDSQUARE FN31PR",
            )
            .await;
            send_and_read(&mut reader, &mut write_half, "VERSION", "VERSION").await;
            send_and_read(&mut reader, &mut write_half, "MYCALL N1CALL", "MYCALL N1CALL").await;
            send_and_read(
                &mut reader,
                &mut write_half,
                "ARQCALL N0CALL 3",
                "ARQCALL N0CALL 3",
            )
            .await;
            // A VARA-style command on the same port, for contrast:
            send_and_read(&mut reader, &mut write_half, "LISTEN OFF (VARA)", "LISTEN OFF").await;

            // --- Host data with no session, on the data port (COP-29 gate) ---
            let data = TcpStream::connect(data_addr).await.unwrap();
            let (dr, mut dw) = data.into_split();
            dw.write_all(&[0xAAu8; 64]).await.unwrap();
            drop(dw);
            let mut dreader = BufReader::new(dr);
            let back = drain_lines(&mut dreader, Duration::from_millis(500)).await;
            println!(
                "data port, 64B, no session -> {back:?} (nothing echoed is expected; the COP-29 gate drops it)"
            );

            println!("\nprobe done; giving the loop a moment");
        })
        .await;
}
