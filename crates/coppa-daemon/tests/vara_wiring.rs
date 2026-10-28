//! COP-46: `coppa_daemon::vara_wiring::serve_vara` is the one place the VARA
//! server is wired into the event loop; `coppad`'s `main` calls it, and so
//! does this test. A command written on the socket must reach the running
//! loop and get its reply, which proves the event bridge is installed.

use std::time::Duration;

use coppa_daemon::config::DaemonConfig;
use coppa_daemon::event_loop::EventLoop;
use coppa_daemon::vara_wiring::serve_vara;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// Read one `\r`-terminated line, failing (not hanging) if none arrives.
async fn read_line<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> String {
    let mut line = Vec::new();
    let n = tokio::time::timeout(Duration::from_secs(10), reader.read_until(b'\r', &mut line))
        .await
        .expect("timed out waiting for a VARA response line")
        .expect("reading a VARA response line should not error");
    assert!(n > 0, "connection closed before a response line arrived");
    String::from_utf8(line).expect("VARA response lines are UTF-8")
}

#[tokio::test]
async fn serve_vara_answers_commands_from_the_running_loop() {
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

            assert_eq!(
                read_line(&mut reader).await,
                format!("VERSION Coppa {}\r", env!("CARGO_PKG_VERSION")),
                "the greeting comes first"
            );
            write_half.write_all(b"MYCALL N1CALL\r").await.unwrap();
            // Skip unsolicited telemetry (BUSY/BUFFER/IAMALIVE); the reply to
            // MYCALL comes from the event loop through the wired bridge.
            loop {
                let line = read_line(&mut reader).await;
                let line = line.trim_end();
                if ["BUSY", "BUFFER", "IAMALIVE", "SNR", "PTT"]
                    .iter()
                    .any(|t| line.starts_with(t))
                {
                    continue;
                }
                assert_eq!(line, "OK", "MYCALL must be answered OK");
                break;
            }
        })
        .await;
}
