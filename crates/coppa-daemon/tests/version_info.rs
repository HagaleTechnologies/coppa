//! COP-178: a host asks coppad's VARA command port for its versions over a
//! real socket. The greeting and `VERSION` keep their old shape;
//! `VERSIONINFO` gives the crate semver, the air-protocol version and the
//! host-API version as separate fields; each query gets exactly one reply.

use std::time::Duration;

use coppa_daemon::config::DaemonConfig;
use coppa_daemon::event_loop::{forward_data_out, EventLoop};
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

/// The next line that is not unsolicited telemetry, with its `\r` kept.
async fn next_reply<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> String {
    loop {
        let line = read_line(reader).await;
        if ["BUSY", "BUFFER", "IAMALIVE", "SNR", "PTT"]
            .iter()
            .any(|t| line.starts_with(t))
        {
            continue;
        }
        return line;
    }
}

#[tokio::test]
async fn versioninfo_reports_separate_versions_with_one_reply_per_query() {
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
            let data_senders = serve_vara(&mut event_loop, cmd_listener, data_listener, &host);
            tokio::spawn(forward_data_out(response_rx, vec![data_senders]));
            tokio::task::spawn_local(async move {
                let _ = event_loop.run().await;
            });

            let cmd = TcpStream::connect(cmd_addr).await.unwrap();
            let _data = TcpStream::connect(data_addr).await.unwrap();
            let (read_half, mut write_half) = cmd.into_split();
            let mut reader = BufReader::new(read_half);

            let legacy = format!("VERSION Coppa {}\r", env!("CARGO_PKG_VERSION"));
            assert_eq!(
                read_line(&mut reader).await,
                legacy,
                "the greeting is unchanged"
            );

            // Each query's reply is the very next non-telemetry line, so an
            // extra OK after any of them would show up as the next reply.
            let info = format!(
                "VERSIONINFO crate_version={} air_protocol_version=1 host_api_version=1\r",
                env!("CARGO_PKG_VERSION")
            );
            for (command, want) in [
                ("VERSION\r", legacy.as_str()),
                ("VERSIONINFO\r", info.as_str()),
                ("versioninfo\r", info.as_str()),
                ("VERSIONINFO extra\r", "WRONG\r"),
                // Sentinel: proves nothing followed the malformed query.
                ("VERSION\r", legacy.as_str()),
            ] {
                write_half.write_all(command.as_bytes()).await.unwrap();
                assert_eq!(
                    next_reply(&mut reader).await,
                    want,
                    "reply to {command:?} (air protocol v1, host API v1)"
                );
            }
        })
        .await;
}
