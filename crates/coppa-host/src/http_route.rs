//! COP-79: one listener, two kinds of client.
//!
//! The WebSocket port reads each connection's request head once and forks on
//! its shape. A WebSocket upgrade is replayed, byte for byte, into the
//! unchanged COP-53 handshake gate through [`PrefixedStream`]. Any other
//! request gets a fixed static answer: the configured page at `GET /` and
//! `GET /index.html`, the configured health status at `GET /health`
//! (COP-140), `404`, `405`, `400` or `431`. `HEAD` gets the `GET` answer
//! without its body. Routing is on request
//! shape, not path, because the gate has never looked at the path and
//! existing clients upgrade at more than `/ws`.
//!
//! No response ever contains a request byte, and nothing here logs the
//! request target or headers: a browser's target can carry `?token=`.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::websocket::Health;

/// Largest request head read before the connection is answered `431`.
pub(crate) const MAX_HTTP_HEAD_BYTES: usize = 8 * 1024;

/// Header slots for parsing; tungstenite's own server handshake allows 124.
const MAX_HEADERS: usize = 124;

/// How long, and how many bytes, [`respond_and_close`] drains after its
/// response before dropping the connection.
const LINGER: std::time::Duration = std::time::Duration::from_millis(500);
const MAX_LINGER_BYTES: usize = 64 * 1024;

/// COP-140: how long a connection has to send its whole request head. Without
/// a deadline, 16 sockets that send nothing hold every connection permit for
/// as long as they stay open, and a health probe is refused all that time.
pub(crate) const HEAD_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

const NOT_FOUND_BODY: &[u8] = b"not found\n";
const METHOD_NOT_ALLOWED_BODY: &[u8] = b"method not allowed\n";
const BAD_REQUEST_BODY: &[u8] = b"bad request\n";
const HEAD_TOO_LARGE_BODY: &[u8] = b"request header fields too large\n";
const HEALTH_OK_BODY: &[u8] = b"{\"status\":\"ok\"}\n";
const HEALTH_DEGRADED_BODY: &[u8] = b"{\"status\":\"degraded\"}\n";

const TEXT_PLAIN: &str = "text/plain; charset=utf-8";
const TEXT_HTML: &str = "text/html; charset=utf-8";
const APPLICATION_JSON: &str = "application/json";

/// What a connection's request head asks for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ClientRequest {
    /// `Connection: Upgrade` plus `Upgrade: websocket`: hand it to the gate.
    Upgrade,
    /// Anything else: answer from [`static_answer`].
    Static { method: String, path: String },
}

/// A complete request head and every byte read to get it.
#[derive(Debug)]
pub(crate) struct ParsedHead {
    /// The bytes read from the stream, head first. Replayed in full on the
    /// upgrade path, so nothing a client sent early is lost.
    pub(crate) bytes: Vec<u8>,
    pub(crate) request: ClientRequest,
}

/// Why no complete request head was read.
#[derive(Debug)]
pub(crate) enum HeadError {
    /// No `\r\n\r\n` within [`MAX_HTTP_HEAD_BYTES`].
    TooLarge,
    /// The bytes are not an HTTP/1.x request head.
    Malformed,
    /// The peer closed the connection before finishing its head.
    Closed,
    /// Reading the stream failed.
    Io(io::Error),
}

impl std::fmt::Display for HeadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeadError::TooLarge => write!(f, "request head exceeds {MAX_HTTP_HEAD_BYTES} bytes"),
            HeadError::Malformed => f.write_str("request head is not HTTP/1.x"),
            HeadError::Closed => f.write_str("peer closed before finishing its request head"),
            HeadError::Io(e) => write!(f, "reading the request head failed: {e}"),
        }
    }
}

impl HeadError {
    /// The response to write before closing, if the peer can still read one.
    pub(crate) fn response(&self) -> Option<Vec<u8>> {
        match self {
            HeadError::TooLarge => Some(static_response(
                StatusLine::HEAD_TOO_LARGE,
                false,
                false,
                TEXT_PLAIN,
                HEAD_TOO_LARGE_BODY,
            )),
            HeadError::Malformed => Some(static_response(
                StatusLine::BAD_REQUEST,
                false,
                false,
                TEXT_PLAIN,
                BAD_REQUEST_BODY,
            )),
            HeadError::Closed | HeadError::Io(_) => None,
        }
    }
}

/// Read from `stream` until it holds a complete request head, the cap, or
/// EOF, then classify the head.
pub(crate) async fn read_head<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> Result<ParsedHead, HeadError> {
    let mut bytes = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let room = MAX_HTTP_HEAD_BYTES - bytes.len();
        let n = stream
            .read(&mut chunk[..room.min(1024)])
            .await
            .map_err(HeadError::Io)?;
        if n == 0 {
            return Err(HeadError::Closed);
        }
        bytes.extend_from_slice(&chunk[..n]);

        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut request = httparse::Request::new(&mut headers);
        match request.parse(&bytes) {
            Ok(httparse::Status::Complete(_)) => {
                let request = classify(&request);
                return Ok(ParsedHead { bytes, request });
            }
            Ok(httparse::Status::Partial) if bytes.len() >= MAX_HTTP_HEAD_BYTES => {
                return Err(HeadError::TooLarge);
            }
            Ok(httparse::Status::Partial) => {}
            Err(_) => return Err(HeadError::Malformed),
        }
    }
}

/// An upgrade needs `Connection` to list `upgrade` and `Upgrade` to list
/// `websocket`, both as case-insensitive tokens separated by commas or
/// spaces, in any of their header lines. That accepts everything
/// tungstenite's own check accepts (it splits `Connection` on both and reads
/// only the first line of each), so no upgrade it would complete is routed
/// away from it; tungstenite still validates the rest of the handshake.
pub(crate) fn classify(request: &httparse::Request<'_, '_>) -> ClientRequest {
    let lists = |name: &str, token: &str| {
        request
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case(name))
            .any(|h| {
                h.value
                    .split(|&b| b == b',' || b == b' ')
                    .any(|t| t.trim_ascii().eq_ignore_ascii_case(token.as_bytes()))
            })
    };
    if lists("connection", "upgrade") && lists("upgrade", "websocket") {
        ClientRequest::Upgrade
    } else {
        ClientRequest::Static {
            method: request.method.unwrap_or_default().to_string(),
            path: request.path.unwrap_or_default().to_string(),
        }
    }
}

/// The complete response for a non-upgrade request. Only the path before any
/// `?` is matched, so `/?token=...` still gets the page, and the query is
/// never echoed or logged. `HEAD` gets the `GET` response with its body cut
/// off, so `Content-Length` still states the `GET` body's length.
pub(crate) fn static_answer(
    method: &str,
    path: &str,
    index_html: Option<&str>,
    health: Option<Health>,
) -> Vec<u8> {
    let head_only = match method {
        "GET" => false,
        "HEAD" => true,
        _ => {
            return static_response(
                StatusLine::METHOD_NOT_ALLOWED,
                true,
                false,
                TEXT_PLAIN,
                METHOD_NOT_ALLOWED_BODY,
            )
        }
    };
    let path = path.split_once('?').map_or(path, |(p, _)| p);
    let (mut response, body_len) = match (path, index_html, health) {
        ("/" | "/index.html", Some(html), _) => (
            static_response(StatusLine::OK, false, false, TEXT_HTML, html.as_bytes()),
            html.len(),
        ),
        ("/health", _, Some(health)) => {
            let body = match health {
                Health::Ok => HEALTH_OK_BODY,
                Health::Degraded => HEALTH_DEGRADED_BODY,
            };
            (
                static_response(StatusLine::OK, false, true, APPLICATION_JSON, body),
                body.len(),
            )
        }
        _ => (
            static_response(
                StatusLine::NOT_FOUND,
                false,
                false,
                TEXT_PLAIN,
                NOT_FOUND_BODY,
            ),
            NOT_FOUND_BODY.len(),
        ),
    };
    if head_only {
        response.truncate(response.len() - body_len);
    }
    response
}

/// Write `response`, half-close, then briefly drain what the peer still
/// sends (an unread request body, the rest of an oversized head). Dropping a
/// socket with unread bytes makes the kernel send a reset, and a reset can
/// discard the response before the peer reads it.
pub(crate) async fn respond_and_close<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    response: &[u8],
) {
    if stream.write_all(response).await.is_err() || stream.shutdown().await.is_err() {
        return;
    }
    let mut sink = [0u8; 1024];
    let mut drained = 0;
    let _ = tokio::time::timeout(LINGER, async {
        while drained < MAX_LINGER_BYTES {
            match stream.read(&mut sink).await {
                Ok(0) | Err(_) => break,
                Ok(n) => drained += n,
            }
        }
    })
    .await;
}

/// An HTTP/1.1 status code and its reason phrase.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StatusLine(u16, &'static str);

impl StatusLine {
    pub(crate) const OK: Self = Self(200, "OK");
    pub(crate) const BAD_REQUEST: Self = Self(400, "Bad Request");
    pub(crate) const NOT_FOUND: Self = Self(404, "Not Found");
    pub(crate) const METHOD_NOT_ALLOWED: Self = Self(405, "Method Not Allowed");
    pub(crate) const HEAD_TOO_LARGE: Self = Self(431, "Request Header Fields Too Large");
}

/// Build a whole response: status line, `Content-Type`, `Content-Length`,
/// `Allow: GET, HEAD` when `allow_get`, `Cache-Control: no-store` when
/// `no_store`, headers that stop other sites framing the page and browsers
/// sniffing a different type, `Connection: close`, then `body`.
pub(crate) fn static_response(
    status: StatusLine,
    allow_get: bool,
    no_store: bool,
    content_type: &str,
    body: &[u8],
) -> Vec<u8> {
    let StatusLine(code, reason) = status;
    let allow = if allow_get {
        "Allow: GET, HEAD\r\n"
    } else {
        ""
    };
    let cache = if no_store {
        "Cache-Control: no-store\r\n"
    } else {
        ""
    };
    let mut response = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {content_type}\r\n\
         Content-Length: {}\r\n{allow}{cache}X-Content-Type-Options: nosniff\r\n\
         X-Frame-Options: DENY\r\nContent-Security-Policy: frame-ancestors 'none'\r\n\
         Connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

/// A stream that yields `prefix` before reading from `inner`; writes go
/// straight to `inner`. Lets the handshake re-read the head [`read_head`]
/// already consumed.
pub(crate) struct PrefixedStream<S> {
    prefix: Vec<u8>,
    pos: usize,
    inner: S,
}

impl<S> PrefixedStream<S> {
    pub(crate) fn new(prefix: Vec<u8>, inner: S) -> Self {
        Self {
            prefix,
            pos: 0,
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for PrefixedStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.pos < this.prefix.len() {
            let rest = &this.prefix[this.pos..];
            let n = rest.len().min(buf.remaining());
            buf.put_slice(&rest[..n]);
            this.pos += n;
            if this.pos == this.prefix.len() {
                this.prefix = Vec::new();
                this.pos = 0;
            }
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PrefixedStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn head_of(bytes: &[u8]) -> Result<ParsedHead, HeadError> {
        let (mut client, mut server) = tokio::io::duplex(64 * 1024);
        client.write_all(bytes).await.unwrap();
        drop(client);
        read_head(&mut server).await
    }

    #[tokio::test]
    async fn upgrade_head_is_classified_as_upgrade_and_kept_whole() {
        let raw = b"GET /ws HTTP/1.1\r\nHost: x\r\nConnection: keep-alive, Upgrade\r\n\
                    Upgrade: WebSocket\r\n\r\nextra";
        let head = head_of(raw).await.unwrap();
        assert_eq!(head.request, ClientRequest::Upgrade);
        assert_eq!(head.bytes, raw);
    }

    #[tokio::test]
    async fn space_separated_connection_tokens_still_upgrade_like_tungstenite() {
        let head = head_of(
            b"GET / HTTP/1.1\r\nConnection: keep-alive Upgrade\r\nUpgrade: websocket\r\n\r\n",
        )
        .await
        .unwrap();
        assert_eq!(head.request, ClientRequest::Upgrade);
    }

    #[tokio::test]
    async fn upgrade_header_without_connection_upgrade_is_static() {
        let head = head_of(b"GET / HTTP/1.1\r\nUpgrade: websocket\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(
            head.request,
            ClientRequest::Static {
                method: "GET".into(),
                path: "/".into()
            }
        );
    }

    #[tokio::test]
    async fn head_without_terminator_within_the_cap_is_too_large() {
        let mut raw = b"GET / HTTP/1.1\r\nX-Pad: ".to_vec();
        raw.resize(MAX_HTTP_HEAD_BYTES + 512, b'a');
        assert!(matches!(head_of(&raw).await, Err(HeadError::TooLarge)));
    }

    #[tokio::test]
    async fn garbage_head_is_malformed_and_eof_is_closed() {
        assert!(matches!(
            head_of(b"\x00\x01 nonsense\r\n\r\n").await,
            Err(HeadError::Malformed)
        ));
        assert!(matches!(
            head_of(b"GET / HTTP/1.1\r\n").await,
            Err(HeadError::Closed)
        ));
    }

    #[test]
    fn static_answers_cover_page_404_and_405() {
        let page = Some("<html></html>");
        let ok = String::from_utf8(static_answer("GET", "/?token=secret", page, None)).unwrap();
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"), "{ok}");
        assert!(ok.contains("Content-Length: 13\r\n"), "{ok}");
        assert!(ok.ends_with("\r\n\r\n<html></html>"), "{ok}");
        assert!(ok.contains("X-Frame-Options: DENY\r\n"), "{ok}");
        assert!(
            ok.contains("Content-Security-Policy: frame-ancestors 'none'\r\n"),
            "{ok}"
        );
        assert!(!ok.contains("secret"));

        let missing = String::from_utf8(static_answer("GET", "/", None, None)).unwrap();
        assert!(
            missing.starts_with("HTTP/1.1 404 Not Found\r\n"),
            "{missing}"
        );
        assert!(missing.ends_with("\r\n\r\nnot found\n"), "{missing}");

        let post = String::from_utf8(static_answer("POST", "/", page, None)).unwrap();
        assert!(post.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
        assert!(post.contains("Allow: GET, HEAD\r\n"));
    }

    #[test]
    fn health_answers_ok_and_degraded_as_uncached_json() {
        let ok =
            String::from_utf8(static_answer("GET", "/health", None, Some(Health::Ok))).unwrap();
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"), "{ok}");
        assert!(
            ok.contains("\r\nContent-Type: application/json\r\n"),
            "{ok}"
        );
        assert!(ok.contains("\r\nCache-Control: no-store\r\n"), "{ok}");
        assert!(ok.contains("\r\nContent-Length: 16\r\n"), "{ok}");
        assert!(ok.ends_with("\r\n\r\n{\"status\":\"ok\"}\n"), "{ok}");
        assert!(!ok.contains("Access-Control-Allow-Origin"), "{ok}");

        let degraded = String::from_utf8(static_answer(
            "GET",
            "/health",
            None,
            Some(Health::Degraded),
        ))
        .unwrap();
        assert!(degraded.starts_with("HTTP/1.1 200 OK\r\n"), "{degraded}");
        assert!(
            degraded.contains("\r\nContent-Length: 22\r\n"),
            "{degraded}"
        );
        assert!(
            degraded.ends_with("\r\n\r\n{\"status\":\"degraded\"}\n"),
            "{degraded}"
        );
    }

    #[test]
    fn health_without_a_value_configured_is_404() {
        let missing =
            String::from_utf8(static_answer("GET", "/health", Some("<html></html>"), None))
                .unwrap();
        assert!(
            missing.starts_with("HTTP/1.1 404 Not Found\r\n"),
            "{missing}"
        );
        assert!(!missing.contains("Cache-Control"), "{missing}");
    }

    #[test]
    fn health_ignores_the_query_and_never_echoes_it() {
        let ok = String::from_utf8(static_answer(
            "GET",
            "/health?token=secret",
            None,
            Some(Health::Ok),
        ))
        .unwrap();
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"), "{ok}");
        assert!(ok.ends_with("{\"status\":\"ok\"}\n"), "{ok}");
        assert!(!ok.contains("secret"), "{ok}");
        assert!(!ok.contains("token"), "{ok}");
    }

    #[test]
    fn head_is_the_get_response_without_its_body() {
        let page = Some("<html></html>");
        for path in ["/", "/index.html", "/health", "/nope"] {
            let get = static_answer("GET", path, page, Some(Health::Ok));
            let head = static_answer("HEAD", path, page, Some(Health::Ok));
            let split = get
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .expect("a response head")
                + 4;
            assert!(split < get.len(), "GET {path} has a body");
            assert_eq!(head, get[..split], "HEAD {path}");
            let text = String::from_utf8(head).unwrap();
            let length = format!("\r\nContent-Length: {}\r\n", get.len() - split);
            assert!(text.contains(&length), "HEAD {path}: {text}");
        }
    }

    #[test]
    fn other_methods_are_405_allowing_get_and_head() {
        for method in ["POST", "PUT", "OPTIONS", "DELETE"] {
            let response = String::from_utf8(static_answer(
                method,
                "/health",
                Some("<html></html>"),
                Some(Health::Ok),
            ))
            .unwrap();
            assert!(
                response.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"),
                "{method}: {response}"
            );
            assert!(
                response.contains("\r\nAllow: GET, HEAD\r\n"),
                "{method}: {response}"
            );
            assert!(!response.contains("status"), "{method}: {response}");
        }
    }

    #[tokio::test]
    async fn prefixed_stream_replays_the_prefix_then_reads_the_inner_stream() {
        let (mut client, server) = tokio::io::duplex(1024);
        client.write_all(b" world").await.unwrap();
        drop(client);
        let mut stream = PrefixedStream::new(b"hello".to_vec(), server);
        let mut out = String::new();
        stream.read_to_string(&mut out).await.unwrap();
        assert_eq!(out, "hello world");
    }
}
