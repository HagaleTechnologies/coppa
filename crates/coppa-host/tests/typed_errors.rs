use coppa_host::HostError;
use std::error::Error;

#[test]
fn error_contract_is_thread_safe() {
    fn check<T: Error + Send + Sync + 'static>() {}
    check::<HostError>();
}

#[cfg(feature = "vara-tcp")]
#[tokio::test]
async fn vara_bind_conflicts_are_typed_for_both_ports() {
    for command_conflict in [true, false] {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = occupied.local_addr().unwrap().port();
        let server = coppa_host::vara::server::VaraServer::new(
            if command_conflict { port } else { 0 },
            if command_conflict { 0 } else { port },
        );
        let error = tokio::time::timeout(std::time::Duration::from_secs(2), server.run())
            .await
            .unwrap()
            .unwrap_err();
        assert_addr_in_use(error);
    }
}

#[cfg(any(
    feature = "vara-tcp",
    feature = "websocket",
    feature = "kiss-tnc",
    feature = "ardop"
))]
fn assert_addr_in_use(error: HostError) {
    assert!(error.source().unwrap().is::<std::io::Error>());
    match error {
        HostError::Io(source) => assert_eq!(source.kind(), std::io::ErrorKind::AddrInUse),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[cfg(feature = "websocket")]
#[tokio::test]
async fn websocket_bind_conflict_is_typed() {
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = coppa_host::websocket::WebSocketServer::new(occupied.local_addr().unwrap().port());
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), server.run())
        .await
        .unwrap()
        .unwrap_err();
    assert_addr_in_use(error);
}

#[cfg(feature = "kiss-tnc")]
#[tokio::test]
async fn kiss_bind_conflict_is_typed() {
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = coppa_host::kiss::KissServer::new(occupied.local_addr().unwrap().port());
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), server.start())
        .await
        .unwrap()
        .unwrap_err();
    assert_addr_in_use(error);
}

#[cfg(feature = "ardop")]
#[tokio::test]
async fn ardop_bind_conflicts_are_typed_for_both_ports() {
    for command_conflict in [true, false] {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = occupied.local_addr().unwrap().port();
        let server = coppa_host::ardop::ArdopServer::new(
            if command_conflict { port } else { 0 },
            if command_conflict { 0 } else { port },
        );
        let error = tokio::time::timeout(std::time::Duration::from_secs(2), server.run())
            .await
            .unwrap()
            .unwrap_err();
        assert_addr_in_use(error);
    }
}

#[cfg(feature = "ardop")]
#[tokio::test]
async fn ardop_framing_errors_preserve_fields_display_and_sources() {
    use coppa_host::ardop::frame::*;
    let err = encode_host_frame(&vec![0; MAX_FRAME + 1]).unwrap_err();
    assert_eq!(err.to_string(), "host frame exceeds MAX_FRAME");
    assert!(
        matches!(err, HostError::HostFrameTooLarge { len, max } if len == MAX_FRAME + 1 && max == MAX_FRAME)
    );
    let err = encode_tnc_frame(ARQ_TAG, &vec![0; MAX_FRAME - 2]).unwrap_err();
    assert_eq!(err.to_string(), "TNC frame exceeds MAX_FRAME");
    assert!(
        matches!(err, HostError::TncFrameTooLarge { len, max } if len == MAX_FRAME - 2 && max == MAX_FRAME - 3)
    );
    let err = decode_tnc_frame(&[0]).unwrap_err();
    assert_eq!(err.to_string(), "truncated frame: no length prefix");
    assert!(matches!(err, HostError::MissingLengthPrefix));
    let err = decode_tnc_frame(&[0, 2, 1, 2]).unwrap_err();
    assert_eq!(err.to_string(), "frame length 2 carries no tag");
    assert!(matches!(err, HostError::MissingTag { len: 2 }));
    let err = decode_tnc_frame(&[0, 4, b'A', b'R', b'Q']).unwrap_err();
    assert_eq!(
        err.to_string(),
        "frame length 4 does not match the 3 bytes present"
    );
    assert!(matches!(
        err,
        HostError::FrameLengthMismatch {
            declared: 4,
            actual: 3
        }
    ));
    let err = decode_tnc_frame(&[0, 3, 0xff, b'R', b'Q']).unwrap_err();
    assert!(err.source().unwrap().is::<std::str::Utf8Error>());
    match err {
        HostError::InvalidTagUtf8 { source } => {
            assert_eq!(source.valid_up_to(), 0);
            assert_eq!(
                HostError::InvalidTagUtf8 { source }.to_string(),
                format!("bad tag: {source}")
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
    let mut partial = &b"\0\x04ab"[..];
    let err = read_host_frame(&mut partial).await.unwrap_err();
    assert!(
        matches!(err, HostError::Io(source) if source.kind() == std::io::ErrorKind::UnexpectedEof)
    );
}
