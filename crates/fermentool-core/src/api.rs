//! The daemon's HTTP API (axum), bound to `127.0.0.1` only.
//!
//! Every route that touches the run forwards a [`Command`] to the control thread
//! and awaits its reply; nothing here touches the pump or the database directly.
//! Milestone 7 is REST + polling; the WebSocket push and static-UI serving land
//! with the UI (milestone 8).

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{broadcast, Notify, RwLock};

use fermentool_curves::CurveSpec;
use fermentool_modbus::serial::available_ports;

use crate::config::Config;
use crate::control::{Command, ControlHandle, DaemonStatus};
use crate::engine::RunConfig;
use crate::store::RunStatus;

/// The built Svelte UI (`ui/dist/`), baked into the binary. The folder path is
/// resolved relative to this crate's `Cargo.toml`.
#[derive(rust_embed::RustEmbed)]
#[folder = "../../ui/dist"]
struct Assets;

#[derive(Clone)]
pub struct AppState {
    pub control: Arc<ControlHandle>,
    pub config: Arc<RwLock<Config>>,
    pub config_path: Arc<PathBuf>,
    /// Notified by `POST /api/shutdown` to stop the server gracefully.
    pub shutdown: Arc<Notify>,
    /// Status fan-out to `/api/ws` subscribers.
    pub events: broadcast::Sender<DaemonStatus>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/config", get(get_config).put(put_config))
        .route("/api/preview", post(preview))
        .route("/api/runs", get(list_runs).post(create_run))
        .route("/api/history", delete(clear_history))
        .route("/api/runs/{id}", get(get_run))
        .route("/api/runs/{id}/ticks", get(get_ticks))
        .route("/api/runs/{id}/events", get(get_events))
        .route("/api/runs/{id}/stop", post(stop_run))
        .route("/api/runs/{id}/abort", post(abort_run))
        .route("/api/recovery", get(get_recovery))
        .route("/api/recovery/resume", post(resume))
        .route("/api/recovery/discard", post(discard))
        .route("/api/serial/ports", get(serial_ports))
        .route("/api/serial/reconnect", post(serial_reconnect))
        .route("/api/pump/stop", post(pump_stop))
        .route("/api/shutdown", post(shutdown))
        .route("/api/ws", get(ws_upgrade))
        .fallback(static_handler)
        .layer(cors_layer())
        .with_state(state)
}

/// The Svelte UI bundled into the Tauri desktop shell runs on the
/// `http://tauri.localhost` origin and calls this API cross-origin. The
/// listener is already `127.0.0.1`-only, so an explicit allow-list of the
/// Tauri origins is the whole exposure. Browser / curl / same-origin callers
/// are unaffected (CORS headers only matter to a browser enforcing them).
fn cors_layer() -> tower_http::cors::CorsLayer {
    use tower_http::cors::{Any, CorsLayer};
    CorsLayer::new()
        .allow_origin([
            "http://tauri.localhost".parse().unwrap(),
            "https://tauri.localhost".parse().unwrap(),
            "tauri://localhost".parse().unwrap(),
        ])
        .allow_methods(Any)
        .allow_headers(Any)
}

// ---------------------------------------------------------------------------
// errors
// ---------------------------------------------------------------------------

enum ApiError {
    /// The control thread is gone.
    Down,
    /// Bad request body / parameters.
    Bad(String),
    /// The engine refused (busy / idle / invalid config).
    Conflict(String),
    NotFound,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (code, msg) = match self {
            ApiError::Down => (
                StatusCode::SERVICE_UNAVAILABLE,
                "control thread is not running".to_string(),
            ),
            ApiError::Bad(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
        };
        (code, Json(json!({ "error": msg }))).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

// ---------------------------------------------------------------------------
// handlers
// ---------------------------------------------------------------------------

async fn status(State(s): State<AppState>) -> ApiResult<Response> {
    let st = s
        .control
        .call(Command::Status)
        .await
        .map_err(|_| ApiError::Down)?;
    Ok(Json(st).into_response())
}

async fn get_config(State(s): State<AppState>) -> Response {
    let cfg = s.config.read().await.clone();
    Json(cfg).into_response()
}

async fn put_config(State(s): State<AppState>, Json(new): Json<Config>) -> ApiResult<Response> {
    new.save(&s.config_path)
        .map_err(|e| ApiError::Bad(e.to_string()))?;
    *s.config.write().await = new.clone();
    // `serial.allow_simulator` gates Start/Resume in the live engine, so a save
    // that flips it must reach the control thread — otherwise the Settings
    // checkbox looks broken until the next reconnect.
    let _ = s
        .control
        .call(|reply| Command::SetAllowSimulator(new.serial.allow_simulator, reply))
        .await;
    Ok(Json(json!({
        "saved": true,
        "note": "serial port, baud and pump address changes apply on the next Connect; other changes take effect on restart"
    }))
    .into_response())
}

#[derive(Deserialize)]
struct PreviewReq {
    curve: CurveSpec,
    #[serde(default = "default_samples")]
    samples: usize,
}
fn default_samples() -> usize {
    240
}

async fn preview(State(s): State<AppState>, Json(req): Json<PreviewReq>) -> ApiResult<Response> {
    if let Err(e) = req.curve.validate() {
        return Err(ApiError::Bad(e));
    }
    let series = s
        .control
        .call(|reply| Command::Preview(req.curve, req.samples.clamp(2, 5000), reply))
        .await
        .map_err(|_| ApiError::Down)?;
    Ok(Json(json!({ "series": series })).into_response())
}

#[derive(Deserialize)]
struct LimitQuery {
    #[serde(default = "default_limit")]
    limit: i64,
}
fn default_limit() -> i64 {
    50
}

async fn list_runs(State(s): State<AppState>, Query(q): Query<LimitQuery>) -> ApiResult<Response> {
    let runs = s
        .control
        .call(|reply| Command::ListRuns(q.limit.clamp(1, 1000), reply))
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(runs).into_response())
}

async fn create_run(State(s): State<AppState>, Json(cfg): Json<RunConfig>) -> ApiResult<Response> {
    let id = s
        .control
        .call(|reply| Command::StartRun(cfg, reply))
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Bad)?;
    Ok((StatusCode::CREATED, Json(json!({ "run_id": id }))).into_response())
}

async fn clear_history(State(s): State<AppState>) -> ApiResult<Response> {
    s.control
        .call(Command::ClearHistory)
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(json!({ "cleared": true })).into_response())
}

async fn get_run(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Response> {
    let run = s
        .control
        .call(|reply| Command::GetRun(id, reply))
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(run).into_response())
}

#[derive(Deserialize)]
struct TickQuery {
    #[serde(default)]
    from: i64,
    #[serde(default = "i64_max")]
    to: i64,
}
fn i64_max() -> i64 {
    i64::MAX
}

/// Hard cap on one `/ticks` response: a 100 h run holds ~360 k rows, and the
/// chart only ever needs a downsampled window. Callers page with `from`/`to`.
const MAX_TICKS_SPAN: i64 = 50_000;

async fn get_ticks(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<TickQuery>,
) -> ApiResult<Response> {
    let from = q.from.max(0);
    let to = q.to.min(from.saturating_add(MAX_TICKS_SPAN));
    let ticks = s
        .control
        .call(|reply| Command::GetTicks {
            run_id: id,
            from,
            to,
            reply,
        })
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(ticks).into_response())
}

async fn get_events(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<LimitQuery>,
) -> ApiResult<Response> {
    let events = s
        .control
        .call(|reply| Command::GetEvents {
            run_id: id,
            limit: q.limit.clamp(1, 5000),
            reply,
        })
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(events).into_response())
}

async fn stop_run(State(s): State<AppState>, Path(_id): Path<i64>) -> ApiResult<Response> {
    s.control
        .call(Command::StopRun)
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(json!({ "stopped": true })).into_response())
}

async fn abort_run(State(s): State<AppState>, Path(_id): Path<i64>) -> ApiResult<Response> {
    s.control
        .call(Command::AbortRun)
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(json!({ "aborted": true })).into_response())
}

/// Stop a pump that is still holding a completed run's final setpoint.
async fn pump_stop(State(s): State<AppState>) -> ApiResult<Response> {
    s.control
        .call(Command::StopPump)
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(json!({ "stopped": true })).into_response())
}

async fn get_recovery(State(s): State<AppState>) -> ApiResult<Response> {
    let info = s
        .control
        .call(Command::Recovery)
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(info).into_response())
}

async fn resume(State(s): State<AppState>) -> ApiResult<Response> {
    let id = s
        .control
        .call(Command::Resume)
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(json!({ "run_id": id })).into_response())
}

#[derive(Deserialize)]
struct DiscardReq {
    status: RunStatus,
}

async fn discard(State(s): State<AppState>, Json(req): Json<DiscardReq>) -> ApiResult<Response> {
    if req.status == RunStatus::Running {
        return Err(ApiError::Bad("status must be terminal".into()));
    }
    s.control
        .call(|reply| Command::DiscardRecovery(req.status, reply))
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(json!({ "resolved": true })).into_response())
}

async fn shutdown(State(s): State<AppState>) -> Response {
    tracing::info!("shutdown requested via API");
    s.shutdown.notify_one();
    Json(json!({ "stopping": true })).into_response()
}

// ---------------------------------------------------------------------------
// WebSocket: push a DaemonStatus on connect and on every state change
// ---------------------------------------------------------------------------

async fn ws_upgrade(State(s): State<AppState>, up: WebSocketUpgrade) -> Response {
    up.on_upgrade(move |socket| ws_loop(socket, s))
}

async fn ws_loop(mut socket: WebSocket, s: AppState) {
    let mut rx = s.events.subscribe();

    // initial snapshot
    if let Ok(st) = s.control.call(Command::Status).await {
        if send_status(&mut socket, &st).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            update = rx.recv() => match update {
                Ok(st) => {
                    if send_status(&mut socket, &st).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
            },
        }
    }
}

async fn send_status(socket: &mut WebSocket, st: &DaemonStatus) -> Result<(), axum::Error> {
    let text = serde_json::to_string(st).unwrap_or_else(|_| "{}".into());
    socket.send(Message::Text(text.into())).await
}

// ---------------------------------------------------------------------------
// static UI (SPA)
// ---------------------------------------------------------------------------

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "png" => "image/png",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

/// `/assets/*` files carry a content hash in their name, so they can be cached
/// forever. Everything else (chiefly `index.html`, which points at the current
/// hashed bundle) must be revalidated every load, or a stale `index.html` pins
/// the browser to an old bundle.
fn cache_control(path: &str) -> &'static str {
    if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

async fn static_handler(uri: Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');
    let path = if raw.is_empty() { "index.html" } else { raw };

    if let Some(file) = Assets::get(path) {
        return (
            [
                (header::CONTENT_TYPE, mime_for(path)),
                (header::CACHE_CONTROL, cache_control(path)),
            ],
            file.data.into_owned(),
        )
            .into_response();
    }
    // SPA fallback: unknown non-asset path -> index.html
    match Assets::get("index.html") {
        Some(index) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            index.data.into_owned(),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "UI not built — run `npm run build` in ui/",
        )
            .into_response(),
    }
}

async fn serial_ports() -> ApiResult<Response> {
    let ports = available_ports().map_err(|e| ApiError::Conflict(e.to_string()))?;
    let out: Vec<_> = ports
        .into_iter()
        .map(|p| {
            json!({
                "name": p.name,
                "kind": p.kind,
                "manufacturer": p.manufacturer,
                "product": p.product,
                "usb_id": p.usb_id,
            })
        })
        .collect();
    Ok(Json(json!({ "ports": out })).into_response())
}

#[derive(Deserialize)]
struct ReconnectReq {
    /// New serial path; omitted = keep the current one.
    #[serde(default)]
    path: Option<String>,
    /// New baud; omitted = keep the current one.
    #[serde(default)]
    baud: Option<u32>,
    /// New MODBUS slave address; omitted = keep the current one.
    #[serde(default)]
    pump_addr: Option<u8>,
}

/// Persist `serial.*` to `config.toml` and rebuild the live pump transport
/// without a daemon restart. Refused (409) while a run is active.
async fn serial_reconnect(
    State(s): State<AppState>,
    Json(req): Json<ReconnectReq>,
) -> ApiResult<Response> {
    let mut cfg = s.config.read().await.clone();
    if let Some(p) = req.path {
        cfg.serial.path = p;
    }
    if let Some(b) = req.baud {
        cfg.serial.baud = b;
    }
    if let Some(a) = req.pump_addr {
        if !(1..=247).contains(&a) {
            return Err(ApiError::Bad("MODBUS address must be 1..=247".into()));
        }
        cfg.pump.address = a;
    }

    // Swap first — a refused reconnect (e.g. a run is active) must not touch
    // `config.toml`, so the file always matches the live transport.
    let serial = cfg.serial.clone();
    let pump_addr = cfg.pump.address;
    let msg = s
        .control
        .call(move |reply| Command::Reconnect {
            serial,
            pump_addr,
            reply,
        })
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;

    cfg.save(&s.config_path)
        .map_err(|e| ApiError::Bad(e.to_string()))?;
    *s.config.write().await = cfg;

    Ok(Json(json!({ "connected": msg })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    use crate::engine::Engine;
    use crate::store::Store;
    use fermentool_modbus::{Pump, SimPump};

    fn test_state() -> AppState {
        let engine = Engine::new(
            Pump::new(SimPump::new(1), 1),
            Store::open_in_memory().unwrap(),
            "test",
        );
        let (events, _) = broadcast::channel(16);
        AppState {
            control: Arc::new(crate::control::spawn(
                engine,
                Duration::from_secs(300),
                events.clone(),
            )),
            config: Arc::new(RwLock::new(Config::default())),
            config_path: Arc::new(std::env::temp_dir().join("ft-api-test.toml")),
            shutdown: Arc::new(Notify::new()),
            events,
        }
    }

    async fn body_json(res: Response) -> serde_json::Value {
        let bytes = to_bytes(res.into_body(), 128 * 1024).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    fn post_json(uri: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn linear_curve() -> serde_json::Value {
        json!({
            "mode": "endpoints", "start": 5, "end": 50, "duration": 3600,
            "clamp_min": 0, "clamp_max": 1000,
            "params": { "kind": "linear", "rate_per_hour": 0 }
        })
    }

    #[tokio::test]
    async fn status_endpoint_reports_idle() {
        let app = router(test_state());
        let res = app.oneshot(get("/api/status")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert!(v["active"].is_null());
        assert_eq!(v["has_pending_recovery"], false);
    }

    #[tokio::test]
    async fn preview_endpoint_returns_a_series() {
        let app = router(test_state());
        let res = app
            .oneshot(post_json(
                "/api/preview",
                json!({ "curve": linear_curve(), "samples": 4 }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        let series = v["series"].as_array().unwrap();
        assert_eq!(series.len(), 4);
        assert_eq!(series[0][1], 5.0);
        assert_eq!(series[3][1], 50.0);
    }

    #[tokio::test]
    async fn preview_rejects_an_invalid_curve() {
        let app = router(test_state());
        let mut bad = linear_curve();
        bad["duration"] = json!(0);
        let res = app
            .oneshot(post_json("/api/preview", json!({ "curve": bad })))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn run_lifecycle_over_http() {
        let app = router(test_state());

        let start = post_json(
            "/api/runs",
            json!({
                "name": "t", "control_var": "rpm", "direction": "cw",
                "pump_addr": 1,
                "curve": linear_curve()
            }),
        );
        let res = app.clone().oneshot(start).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        assert_eq!(body_json(res).await["run_id"], 1);

        // now active
        let res = app.clone().oneshot(get("/api/status")).await.unwrap();
        assert_eq!(body_json(res).await["active"]["run_id"], 1);

        // a second start is refused
        let dup = post_json(
            "/api/runs",
            json!({
                "name": "t2", "control_var": "rpm", "direction": "cw",
                "pump_addr": 1,
                "curve": linear_curve()
            }),
        );
        let res = app.clone().oneshot(dup).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        // stop
        let res = app
            .clone()
            .oneshot(post_json("/api/runs/1/stop", json!({})))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let res = app.oneshot(get("/api/runs/1")).await.unwrap();
        assert_eq!(body_json(res).await["status"], "stopped");
    }

    #[tokio::test]
    async fn missing_run_is_404() {
        let app = router(test_state());
        let res = app.oneshot(get("/api/runs/999")).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn reconnect_persists_config_and_reports_the_transport() {
        let dir = std::env::temp_dir().join(format!("ft-reconnect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfgp = dir.join("config.toml");

        let mut st = test_state();
        st.config_path = Arc::new(cfgp.clone());
        let app = router(st);

        let res = app
            .oneshot(post_json("/api/serial/reconnect", json!({ "path": "sim" })))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert!(
            v["connected"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("simulator"),
            "got: {v}"
        );

        let written = std::fs::read_to_string(&cfgp).unwrap();
        assert!(
            written.contains("path = \"sim\""),
            "config not written: {written}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reconnect_is_conflict_during_a_run_and_leaves_config_untouched() {
        let dir = std::env::temp_dir().join(format!("ft-reconnect-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfgp = dir.join("config.toml");

        let mut st = test_state();
        st.config_path = Arc::new(cfgp.clone());
        let app = router(st);

        let start = post_json(
            "/api/runs",
            json!({
                "name": "t", "control_var": "rpm", "direction": "cw",
                "pump_addr": 1, "curve": linear_curve()
            }),
        );
        assert_eq!(
            app.clone().oneshot(start).await.unwrap().status(),
            StatusCode::CREATED
        );

        let res = app
            .oneshot(post_json("/api/serial/reconnect", json!({ "path": "COM_NOPE" })))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);

        // A refused reconnect must not have written the config file.
        assert!(
            !cfgp.exists(),
            "config was rewritten despite the reconnect being refused"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn pump_stop_is_conflict_when_nothing_held() {
        let app = router(test_state());
        let res = app
            .oneshot(post_json("/api/pump/stop", json!({})))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn pump_stop_releases_a_completed_run_hold() {
        let app = router(test_state());

        let start = post_json(
            "/api/runs",
            json!({
                "name": "s", "control_var": "rpm", "direction": "cw", "pump_addr": 1,
                "curve": {
                    "mode": "endpoints", "start": 10, "end": 20, "duration": 1,
                    "clamp_min": 0, "clamp_max": 350,
                    "params": { "kind": "linear", "rate_per_hour": 0 }
                }
            }),
        );
        assert_eq!(
            app.clone().oneshot(start).await.unwrap().status(),
            StatusCode::CREATED
        );

        tokio::time::sleep(Duration::from_millis(2500)).await;
        let st = body_json(app.clone().oneshot(get("/api/status")).await.unwrap()).await;
        assert!(st["active"].is_null(), "run should have completed: {st}");
        assert!(!st["holding"].is_null(), "expected a hold: {st}");

        let res = app
            .oneshot(post_json("/api/pump/stop", json!({})))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn reconnect_with_empty_body_uses_current_config() {
        let app = router(test_state());
        let res = app
            .oneshot(post_json("/api/serial/reconnect", json!({})))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn reconnect_rejects_a_non_json_body() {
        let app = router(test_state());
        let req = Request::builder()
            .method("POST")
            .uri("/api/serial/reconnect")
            .header("content-type", "application/json")
            .body(Body::from("not json"))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert!(res.status().is_client_error(), "got: {}", res.status());
    }

    // The Svelte UI, bundled into the Tauri window, calls this API from the
    // `http://tauri.localhost` origin — cross-origin, so it needs CORS.

    #[tokio::test]
    async fn cors_echoes_the_tauri_origin() {
        let app = router(test_state());
        let req = Request::builder()
            .uri("/api/status")
            .header("origin", "http://tauri.localhost")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("http://tauri.localhost"),
        );
    }

    #[tokio::test]
    async fn cors_preflight_is_answered() {
        let app = router(test_state());
        let req = Request::builder()
            .method("OPTIONS")
            .uri("/api/config")
            .header("origin", "http://tauri.localhost")
            .header("access-control-request-method", "PUT")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert!(res.status().is_success(), "got: {}", res.status());
        assert!(res.headers().contains_key("access-control-allow-methods"));
    }

    #[tokio::test]
    async fn cors_does_not_echo_an_unknown_origin() {
        let app = router(test_state());
        let req = Request::builder()
            .uri("/api/status")
            .header("origin", "http://evil.example")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        // Still served — CORS is browser-enforced — but with no allow-origin echo.
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers().get("access-control-allow-origin").is_none());
    }
}
