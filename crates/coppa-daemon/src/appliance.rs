//! Debian appliance setup, isolated from the ordinary daemon invocation.
use anyhow::Result;

use crate::cli::LogLevel;

#[cfg(all(feature = "cpal-backend", feature = "websocket"))]
mod server {
    use super::*;
    use std::future::IntoFuture;
    use std::path::{Path, PathBuf};

    use crate::cli::ConfigSource;
    use anyhow::Context;
    use axum::{
        body::{to_bytes, Body, Bytes},
        extract::Request,
        extract::{DefaultBodyLimit, State},
        http::{header, HeaderMap, Method, StatusCode, Uri},
        middleware::{self, Next},
        response::{Html, IntoResponse, Response},
        routing::get,
        Json, Router,
    };
    use serde::{Deserialize, Serialize};
    use serde_json::json;
    use std::{fs, io::Write, net::SocketAddr, os::unix::fs::OpenOptionsExt, sync::Arc};
    use tokio::sync::{oneshot, Mutex, Semaphore};

    const MAX_BODY: usize = 8192;

    #[derive(Clone)]
    struct App {
        token: Arc<str>,
        config: PathBuf,
        host: String,
        origin: String,
        saved: Arc<Mutex<Option<oneshot::Sender<()>>>>,
        save_lock: Arc<Mutex<()>>,
        requests: Arc<Semaphore>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Settings {
        callsign: String,
        grid: String,
        input_device_id: String,
        output_device_id: String,
        ptt: Ptt,
        beacon_interval_secs: u64,
    }

    #[derive(Deserialize)]
    #[serde(tag = "method", rename_all = "lowercase", deny_unknown_fields)]
    enum Ptt {
        None,
        Vox,
        Rigctld {
            address: String,
        },
        Serial {
            path: String,
            line: String,
        },
        Gpio {
            chip: String,
            line: u32,
            active_low: bool,
        },
        Cm108 {
            path: String,
            pin: Option<u8>,
            active_low: bool,
        },
    }

    #[derive(Serialize)]
    struct Device {
        id: String,
        name: String,
    }

    fn reject(status: StatusCode, error: &str) -> Response {
        (status, Json(json!({"error": error}))).into_response()
    }

    fn secure(mut response: Response) -> Response {
        let h = response.headers_mut();
        h.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
        h.insert("x-frame-options", "DENY".parse().unwrap());
        h.insert("content-security-policy", "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'".parse().unwrap());
        response
    }

    async fn bounded(State(app): State<App>, request: Request, next: Next) -> Response {
        let Ok(_permit) = app.requests.clone().try_acquire_owned() else {
            return secure(reject(StatusCode::SERVICE_UNAVAILABLE, "busy"));
        };
        if request.method() == Method::PUT {
            if let Err(response) = authorize(&app, request.headers(), Method::PUT, request.uri()) {
                return secure(*response);
            }
            let (parts, body) = request.into_parts();
            let bytes = match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                to_bytes(body, MAX_BODY),
            )
            .await
            {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(_)) => {
                    return secure(reject(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large"))
                }
                Err(_) => return secure(reject(StatusCode::REQUEST_TIMEOUT, "request_timeout")),
            };
            return next
                .run(Request::from_parts(parts, Body::from(bytes)))
                .await;
        }
        match tokio::time::timeout(std::time::Duration::from_secs(10), next.run(request)).await {
            Ok(response) => response,
            Err(_) => secure(reject(StatusCode::REQUEST_TIMEOUT, "request_timeout")),
        }
    }

    fn constant_eq(a: &[u8], b: &[u8]) -> bool {
        let mut diff = a.len() ^ b.len();
        for i in 0..a.len().max(b.len()) {
            diff |= usize::from(*a.get(i).unwrap_or(&0) ^ *b.get(i).unwrap_or(&0));
        }
        diff == 0
    }

    fn authorize(
        app: &App,
        headers: &HeaderMap,
        method: Method,
        uri: &Uri,
    ) -> Result<(), Box<Response>> {
        if uri.query().is_some()
            || headers.get(header::HOST).and_then(|v| v.to_str().ok()) != Some(&app.host)
        {
            return Err(Box::new(reject(StatusCode::FORBIDDEN, "forbidden")));
        }
        if let Some(origin) = headers.get(header::ORIGIN) {
            if origin.to_str().ok() != Some(&app.origin) {
                return Err(Box::new(reject(StatusCode::FORBIDDEN, "forbidden")));
            }
        } else if method == Method::PUT {
            return Err(Box::new(reject(StatusCode::FORBIDDEN, "forbidden")));
        }
        let supplied = headers
            .get(header::AUTHORIZATION)
            .map(|v| v.as_bytes())
            .unwrap_or_default();
        let expected = format!("Bearer {}", app.token);
        if !constant_eq(supplied, expected.as_bytes()) {
            return Err(Box::new(reject(StatusCode::UNAUTHORIZED, "unauthorized")));
        }
        Ok(())
    }

    async fn page(State(app): State<App>, headers: HeaderMap, uri: Uri) -> Response {
        if headers.get(header::HOST).and_then(|v| v.to_str().ok()) != Some(&app.host)
            || uri.query().is_some()
        {
            return secure(reject(StatusCode::FORBIDDEN, "forbidden"));
        }
        secure(Html(include_str!("web/setup.html")).into_response())
    }

    async fn script(State(app): State<App>, headers: HeaderMap, uri: Uri) -> Response {
        if headers.get(header::HOST).and_then(|v| v.to_str().ok()) != Some(&app.host)
            || uri.query().is_some()
        {
            return secure(reject(StatusCode::FORBIDDEN, "forbidden"));
        }
        let mut response = include_str!("web/setup.js").into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            "text/javascript; charset=utf-8".parse().unwrap(),
        );
        secure(response)
    }

    async fn read_config(State(app): State<App>, headers: HeaderMap, uri: Uri) -> Response {
        if let Err(r) = authorize(&app, &headers, Method::GET, &uri) {
            return secure(*r);
        }
        if path_present(&app.config) {
            return secure(reject(StatusCode::CONFLICT, "already_configured"));
        }
        secure(Json(json!({"callsign":"","grid":"","input_device_id":"","output_device_id":"","ptt":{"method":"none"},"beacon_interval_secs":0})).into_response())
    }

    async fn devices(State(app): State<App>, headers: HeaderMap, uri: Uri) -> Response {
        if let Err(r) = authorize(&app, &headers, Method::GET, &uri) {
            return secure(*r);
        }
        let result = tokio::task::spawn_blocking(coppa_audio::list_devices).await;
        let Ok(Ok(items)) = result else {
            return secure(reject(
                StatusCode::SERVICE_UNAVAILABLE,
                "devices_unavailable",
            ));
        };
        let inputs: Vec<Device> = items
            .iter()
            .filter(|d| d.input_channels > 0)
            .filter_map(|d| {
                d.id.as_ref().map(|id| Device {
                    id: id.clone(),
                    name: d.name.clone(),
                })
            })
            .collect();
        let outputs: Vec<Device> = items
            .iter()
            .filter(|d| d.output_channels > 0)
            .filter_map(|d| {
                d.id.as_ref().map(|id| Device {
                    id: id.clone(),
                    name: d.name.clone(),
                })
            })
            .collect();
        secure(Json(json!({"inputs":inputs,"outputs":outputs})).into_response())
    }

    fn invalid(field: &str) -> Response {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error":"invalid_settings","field":field})),
        )
            .into_response()
    }

    fn parse_settings(body: &[u8]) -> std::result::Result<Settings, Box<Response>> {
        let mut decoder = serde_json::Deserializer::from_slice(body);
        let settings = serde_path_to_error::deserialize(&mut decoder).map_err(|error| {
            if error.inner().is_syntax() || error.inner().is_eof() {
                return Box::new(invalid("request"));
            }
            let path = error.path().to_string();
            let detail = error.inner().to_string();
            for prefix in ["unknown field `", "missing field `"] {
                if let Some(name) = detail
                    .strip_prefix(prefix)
                    .and_then(|rest| rest.split('`').next())
                {
                    if !name.is_empty() {
                        return Box::new(invalid(&if path == "ptt" {
                            format!("ptt.{name}")
                        } else {
                            name.to_string()
                        }));
                    }
                }
            }
            Box::new(invalid(if path.is_empty() || path == "." {
                "request"
            } else {
                &path
            }))
        })?;
        decoder.end().map_err(|_| Box::new(invalid("request")))?;
        Ok(settings)
    }

    fn device_path(path: &str) -> bool {
        let mut parts = Path::new(path).components();
        matches!(parts.next(), Some(std::path::Component::RootDir))
            && matches!(parts.next(), Some(std::path::Component::Normal(dev)) if dev == "dev")
            && parts.clone().next().is_some()
            && parts.all(|part| matches!(part, std::path::Component::Normal(_)))
            && !path.contains(':')
    }

    fn rigctld_address(address: &str) -> bool {
        if address.parse::<std::net::SocketAddr>().is_ok() {
            return true;
        }
        let Some((host, port)) = address.rsplit_once(':') else {
            return false;
        };
        !host.is_empty()
            && !host.starts_with('-')
            && !host.ends_with(['.', '-'])
            && host
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
            && port.parse::<u16>().is_ok_and(|p| p != 0)
    }

    fn prepare(s: Settings, token: &str) -> Result<crate::config::DaemonConfig, Box<Response>> {
        use coppa_protocol::mac::Callsign;
        let mut config = crate::config::DaemonConfig::default();
        let callsign = s.callsign.trim().to_ascii_uppercase();
        if Callsign::new(&callsign).is_err() || callsign.is_empty() {
            return Err(Box::new(invalid("callsign")));
        }
        if s.grid.len() > u8::MAX as usize || !s.grid.is_ascii() {
            return Err(Box::new(invalid("grid")));
        }
        if s.input_device_id.is_empty() {
            return Err(Box::new(invalid("input_device_id")));
        }
        if s.output_device_id.is_empty() {
            return Err(Box::new(invalid("output_device_id")));
        }
        let (method, address, invert) = match s.ptt {
            Ptt::None => ("none".to_string(), None, false),
            Ptt::Vox => ("vox".to_string(), None, false),
            Ptt::Rigctld { address } => {
                if !rigctld_address(&address) {
                    return Err(Box::new(invalid("ptt.address")));
                }
                ("rigctld".to_string(), Some(address), false)
            }
            Ptt::Serial { path, line } => {
                if !device_path(&path) || !["rts", "dtr"].contains(&line.as_str()) {
                    return Err(Box::new(invalid("ptt.path")));
                }
                (format!("serial:{path}:{line}"), None, false)
            }
            Ptt::Gpio {
                chip,
                line,
                active_low,
            } => {
                if chip.is_empty() || chip.contains(':') {
                    return Err(Box::new(invalid("ptt.chip")));
                }
                (format!("gpio:{chip}:{line}"), None, active_low)
            }
            Ptt::Cm108 {
                path,
                pin,
                active_low,
            } => {
                if !device_path(&path) {
                    return Err(Box::new(invalid("ptt.path")));
                }
                let method = match pin {
                    Some(pin) => format!("cm108:{path}:{pin}"),
                    None => format!("cm108:{path}"),
                };
                (method, None, active_low)
            }
        };
        config.engine.callsign = callsign;
        config.engine.grid = (!s.grid.is_empty()).then_some(s.grid);
        config.audio.input_device_id = s.input_device_id;
        config.audio.output_device_id = s.output_device_id;
        config.radio.ptt_method = method;
        config.radio.ptt_invert = invert;
        if let Some(address) = address {
            config.radio.rigctld_address = address;
        }
        config.station_id.beacon_interval_secs = s.beacon_interval_secs;
        config.host.bind_address = "127.0.0.1".into();
        config.host.vara_enabled = false;
        config.host.websocket_enabled = true;
        config.host.websocket_port = std::env::var("COPPA_APPLIANCE_DASHBOARD_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8400);
        config.host.websocket_token =
            Some(serde_json::from_value(json!(token)).map_err(|_| Box::new(invalid("request")))?);
        config.host.websocket_allowed_origins =
            vec![format!("http://127.0.0.1:{}", config.host.websocket_port)];
        config
            .check_for_reload()
            .map_err(|_| Box::new(invalid("ptt")))?;
        if config.radio.ptt_invert_error().is_some() {
            return Err(Box::new(invalid("ptt.active_low")));
        }
        match config
            .radio
            .ptt_config()
            .map_err(|_| Box::new(invalid("ptt.method")))?
        {
            crate::config::PttConfig::Serial { .. } if !cfg!(feature = "serial-ptt") => {
                return Err(Box::new(invalid("ptt.method")));
            }
            crate::config::PttConfig::Gpio { .. } | crate::config::PttConfig::Cm108 { .. }
                if !cfg!(feature = "gpio-ptt") =>
            {
                return Err(Box::new(invalid("ptt.method")));
            }
            _ => {}
        }
        Ok(config)
    }

    fn publish(path: &Path, contents: &str) -> Result<bool> {
        publish_with_sync(path, contents, |parent| fs::File::open(parent)?.sync_all())
    }

    fn publish_with_sync(
        path: &Path,
        contents: &str,
        sync_directory: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<bool> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let candidate = path.with_extension(format!("tmp.{}.{nonce}", std::process::id()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&candidate)?;
        let result = (|| {
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
            match fs::hard_link(&candidate, path) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
                Err(e) => Err(e),
            }
        })();
        let _ = fs::remove_file(&candidate);
        if result.as_ref().is_ok_and(|saved| *saved) {
            if let Some(parent) = path.parent() {
                if let Err(error) = sync_directory(parent) {
                    tracing::warn!(%error, "managed configuration committed; directory sync failed");
                }
            }
        }
        Ok(result?)
    }

    #[cfg(test)]
    #[test]
    fn committed_config_survives_directory_sync_failure() {
        let root = std::env::temp_dir().join(format!("coppa-publish-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let target = root.join("coppad.toml");
        assert!(publish_with_sync(&target, "station = 1", |_| {
            Err(std::io::Error::other("injected directory sync failure"))
        })
        .unwrap());
        assert_eq!(fs::read_to_string(&target).unwrap(), "station = 1");
        assert!(!publish(&target, "station = 2").unwrap());
        assert_eq!(fs::read_to_string(&target).unwrap(), "station = 1");
        fs::remove_dir_all(root).unwrap();
    }

    fn path_present(path: &Path) -> bool {
        match fs::symlink_metadata(path) {
            Ok(_) => true,
            Err(error) => error.kind() != std::io::ErrorKind::NotFound,
        }
    }

    async fn save(
        State(app): State<App>,
        headers: HeaderMap,
        uri: Uri,
        body: std::result::Result<Bytes, axum::extract::rejection::BytesRejection>,
    ) -> Response {
        if let Err(r) = authorize(&app, &headers, Method::PUT, &uri) {
            return secure(*r);
        }
        if headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            != Some("application/json")
        {
            return secure(reject(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
            ));
        }
        let body = match body {
            Ok(body) => body,
            Err(_) => return secure(reject(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large")),
        };
        if body.len() > MAX_BODY {
            return secure(reject(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large"));
        }
        let _lock = app.save_lock.lock().await;
        if path_present(&app.config) {
            return secure(reject(StatusCode::CONFLICT, "already_configured"));
        }
        let settings: Settings = match parse_settings(&body) {
            Ok(s) => s,
            Err(response) => return secure(*response),
        };
        let config = match prepare(settings, &app.token) {
            Ok(c) => c,
            Err(r) => return secure(*r),
        };
        let path = app.config.clone();
        let dashboard_port = config.host.websocket_port;
        let check = config.clone();
        let result = tokio::task::spawn_blocking(
            move || -> std::result::Result<bool, (StatusCode, &'static str, anyhow::Error)> {
                let audio_error = |e| (StatusCode::UNPROCESSABLE_ENTITY, "audio_unavailable", e);
                let save_error = |e| (StatusCode::INTERNAL_SERVER_ERROR, "save_failed", e);
                let devices = coppa_audio::list_devices().map_err(audio_error)?;
                if !devices.iter().any(|d| {
                    d.input_channels > 0 && d.id.as_deref() == Some(&check.audio.input_device_id)
                }) || !devices.iter().any(|d| {
                    d.output_channels > 0 && d.id.as_deref() == Some(&check.audio.output_device_id)
                }) {
                    return Err(audio_error(anyhow::anyhow!(
                        "selected audio device unavailable"
                    )));
                }
                let input = crate::open_input(
                    "",
                    &check.audio.input_device_id,
                    48000,
                    check.audio.sample_rate,
                    check.audio.buffer_size,
                    0,
                )
                .map_err(audio_error)?;
                let output = crate::open_output(
                    "",
                    &check.audio.output_device_id,
                    48000,
                    check.audio.sample_rate,
                    check.audio.buffer_size,
                    0,
                )
                .map_err(audio_error)?;
                drop((input, output));
                let text = toml::to_string_pretty(&check).map_err(|e| save_error(e.into()))?;
                publish(&path, &text).map_err(save_error)
            },
        )
        .await;
        match result {
            Ok(Ok(true)) => {
                if let Some(tx) = app.saved.lock().await.take() {
                    let _ = tx.send(());
                }
                secure(
                    Json(json!({"status":"saved","dashboard_url":format!("http://127.0.0.1:{dashboard_port}/")})).into_response(),
                )
            }
            Ok(Ok(false)) => secure(reject(StatusCode::CONFLICT, "already_configured")),
            Ok(Err((status, error, e))) => {
                tracing::warn!(error = %e, "appliance save failed");
                secure(reject(status, error))
            }
            Err(_) => secure(reject(StatusCode::INTERNAL_SERVER_ERROR, "save_failed")),
        }
    }

    pub async fn run(level: Option<LogLevel>) -> Result<()> {
        let state =
            PathBuf::from(std::env::var("STATE_DIRECTORY").context("STATE_DIRECTORY is required")?);
        let credentials = PathBuf::from(
            std::env::var("CREDENTIALS_DIRECTORY").context("CREDENTIALS_DIRECTORY is required")?,
        );
        let token = fs::read_to_string(credentials.join("setup-token"))
            .context("reading setup credential")?;
        let token = token.trim();
        if token.len() != 32 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
            anyhow::bail!("invalid setup credential");
        }
        let admin = std::env::var_os("COPPA_APPLIANCE_ADMIN_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/etc/coppa/coppad.toml"));
        let managed = state.join("coppad.toml");
        if path_present(&admin) || path_present(&managed) {
            let source = ConfigSource::Explicit(if path_present(&admin) { admin } else { managed });
            let (config, _) = crate::load_config(&source, None)?;
            crate::init_tracing(level, None);
            return crate::run_daemon(config, source, None).await;
        }
        crate::init_tracing(level, None);
        let port = std::env::var("COPPA_APPLIANCE_SETUP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8401);
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
        let addr: SocketAddr = listener.local_addr()?;
        tracing::info!(%addr, "Appliance setup listening");
        let (tx, rx) = oneshot::channel();
        let app = App {
            token: Arc::from(token),
            config: managed.clone(),
            host: format!("127.0.0.1:{}", addr.port()),
            origin: format!("http://127.0.0.1:{}", addr.port()),
            saved: Arc::new(Mutex::new(Some(tx))),
            save_lock: Arc::new(Mutex::new(())),
            requests: Arc::new(Semaphore::new(8)),
        };
        let router = Router::new()
            .route("/setup", get(page))
            .route("/setup.js", get(script))
            .route("/api/config", get(read_config).put(save))
            .route("/api/devices", get(devices))
            .layer(DefaultBodyLimit::max(MAX_BODY))
            .layer(middleware::from_fn_with_state(app.clone(), bounded))
            .with_state(app);
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .context("registering setup stop signal")?;
        let (drain_tx, drain_rx) = oneshot::channel();
        let server = axum::serve(listener, router).with_graceful_shutdown(async move {
            tokio::select! { _ = rx => {}, _ = terminate.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
            let _ = drain_tx.send(());
        }).into_future();
        tokio::pin!(server);
        tokio::select! {
            result = &mut server => result?,
            _ = drain_rx => {
                match tokio::time::timeout(std::time::Duration::from_secs(5), &mut server).await {
                    Ok(result) => result?,
                    Err(_) => tracing::warn!("setup listener drain timed out"),
                }
            }
        }
        if !path_present(&managed) {
            return Ok(());
        }
        let source = ConfigSource::Explicit(managed);
        let (config, _) = crate::load_config(&source, None)?;
        crate::run_daemon(config, source, None).await
    }
}

pub async fn run(level: Option<LogLevel>) -> Result<()> {
    #[cfg(all(feature = "cpal-backend", feature = "websocket"))]
    {
        server::run(level).await
    }
    #[cfg(not(all(feature = "cpal-backend", feature = "websocket")))]
    {
        let _ = level;
        anyhow::bail!("--appliance requires cpal-backend and websocket")
    }
}
