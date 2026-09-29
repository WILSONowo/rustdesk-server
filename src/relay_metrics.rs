//! Private, read-only relay telemetry. Disabled unless explicitly configured.
pub use crate::relay_telemetry::Snapshot;
use axum::{
    extract::Extension,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use hbb_common::{bail, tokio, ResultType};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};

static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static FORWARDED: AtomicU64 = AtomicU64::new(0);

pub struct Session;
impl Session {
    pub fn start() -> Self {
        ACTIVE.fetch_add(1, Ordering::Relaxed);
        Self
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::Relaxed);
    }
}
pub fn forwarded(bytes: usize) {
    FORWARDED.fetch_add(bytes as u64, Ordering::Relaxed);
}

struct State {
    id: String,
    token: String,
    boot: String,
    started: Instant,
}

async fn metrics(headers: HeaderMap, Extension(state): Extension<Arc<State>>) -> Response {
    let supplied = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !sodiumoxide::utils::memcmp(supplied.as_bytes(), state.token.as_bytes()) {
        return (
            StatusCode::UNAUTHORIZED,
            [("cache-control", "no-store")],
            "Unauthorized",
        )
            .into_response();
    }
    let snapshot = Snapshot {
        schema: 1,
        node_id: state.id.clone(),
        boot_id: state.boot.clone(),
        uptime_ms: state.started.elapsed().as_millis() as u64,
        active_sessions: ACTIVE.load(Ordering::Relaxed),
        forwarded_bytes: FORWARDED.load(Ordering::Relaxed),
    };
    ([("cache-control", "no-store")], Json(snapshot)).into_response()
}

// Binding happens before returning, so bad credentials or occupied ports stop startup.
pub fn start_from_env() -> ResultType<Option<tokio::task::JoinHandle<Result<(), String>>>> {
    let bind = match std::env::var("RELAY_METRICS_BIND") {
        Ok(value) => value.parse::<SocketAddr>()?,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let id = std::env::var("RELAY_NODE_ID")?;
    let token = std::env::var("RELAY_METRICS_TOKEN")?;
    if id.trim().is_empty() || token.len() < 32 || !token.bytes().all(|b| b.is_ascii_graphic()) {
        bail!("Metrics require RELAY_NODE_ID and a RELAY_METRICS_TOKEN of at least 32 printable ASCII characters");
    }
    let listener = std::net::TcpListener::bind(bind)?;
    listener.set_nonblocking(true)?;
    let state = Arc::new(State {
        id,
        token,
        boot: uuid::Uuid::new_v4().to_string(),
        started: Instant::now(),
    });
    let app = Router::new()
        .route("/metrics", get(metrics))
        .layer(Extension(state));
    let server = axum::Server::from_tcp(listener)?.serve(app.into_make_service());
    hbb_common::log::info!("Relay metrics listening on {bind}");
    Ok(Some(tokio::spawn(async move {
        server.await.map_err(|err| err.to_string())
    })))
}
