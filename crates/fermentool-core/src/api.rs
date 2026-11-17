//! The daemon's HTTP API (axum), bound to `127.0.0.1` only.
//!
//! Every route that touches the run or the pump forwards a [`Command`] to the
//! control thread and awaits its reply. Reading the journal (history, ticks,
//! events, tracking, calibrations) and the calibration records go through the
//! API's own SQLite connection instead ([`ApiDb`]): the control thread also
//! talks to the pump and the balance, and a page of history must never wait
//! behind a device that is slow to answer.
//! Milestone 7 is REST + polling; the WebSocket push and static-UI serving land
//! with the UI (milestone 8).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{broadcast, Notify, RwLock};

use fermentool_curves::CurveSpec;
use fermentool_modbus::serial::available_ports;

use crate::config::{Config, PumpModelCfg, SERIAL_BAUDS};
use crate::control::{Command, ControlHandle, DaemonStatus};
use crate::engine::{ErrDetail, RunConfig};
use crate::store::{NewCalibration, RunKind, RunStatus, Store, StoreError, CALIBRATION_DRAFT_KEY};

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
    /// Alarms ringing until acknowledged, shared with the notifier.
    pub pages: crate::notify::Pages,
    /// The API's own connection to the journal.
    pub db: Arc<ApiDb>,
    /// Bumped by every config write: `GET /api/config` hands it out as
    /// `_rev`, and a `PUT` carrying an older one is refused, so a page that
    /// read the settings before a Connect cannot write the old port back.
    pub config_rev: Arc<AtomicU64>,
    /// Where this daemon keeps its files and when it started, for Settings > About.
    pub about: Arc<About>,
    /// The camera supervisor. Its handlers never touch the control thread.
    pub camera: crate::camera::Camera,
    /// The `timelapse` thread (its own connection and folder).
    pub timelapses: crate::timelapse::Timelapses,
    /// `fermentool-camera.exe` was not found next to the daemon.
    pub helper_missing: bool,
    /// Set when the daemon stops: open live views end, or graceful shutdown
    /// would wait for them forever.
    pub stopping: Arc<tokio::sync::watch::Sender<bool>>,
}

/// What `GET /api/about` reports besides the build's own constants.
#[derive(Debug, Clone)]
pub struct About {
    pub data_dir: PathBuf,
    pub db: PathBuf,
    pub log_dir: PathBuf,
    pub started_at: std::time::SystemTime,
}

impl Default for About {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::new(),
            db: PathBuf::new(),
            log_dir: PathBuf::new(),
            started_at: std::time::SystemTime::now(),
        }
    }
}

/// The API's connection to the journal, separate from the control thread's.
/// SQLite in WAL mode lets it read while the control thread writes ticks.
/// Calls run on tokio's blocking pool, one at a time on this connection.
pub struct ApiDb(std::sync::Mutex<Store>);

impl ApiDb {
    pub fn new(store: Store) -> Self {
        Self(std::sync::Mutex::new(store))
    }

    async fn call<R, F>(self: &Arc<Self>, f: F) -> ApiResult<R>
    where
        R: Send + 'static,
        F: FnOnce(&Store) -> R + Send + 'static,
    {
        let db = Arc::clone(self);
        tokio::task::spawn_blocking(move || f(&db.0.lock().unwrap_or_else(|p| p.into_inner())))
            .await
            .map_err(|e| ApiError::Conflict(format!("journal access failed: {e}")))
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/about", get(about))
        .route("/api/about/open", post(open_location))
        .route("/api/status", get(status))
        .route("/api/config", get(get_config).put(put_config))
        .route("/api/preview", post(preview))
        .route("/api/runs", get(list_runs).post(create_run))
        .route("/api/history", delete(clear_history))
        .route("/api/runs/active/rate", post(set_active_rate))
        .route("/api/runs/{id}", get(get_run).delete(delete_run))
        .route("/api/runs/{id}/ticks", get(get_ticks))
        .route("/api/runs/{id}/events", get(get_events))
        .route("/api/runs/{id}/tracking", get(get_tracking))
        .route("/api/runs/{id}/stop", post(stop_run))
        .route("/api/runs/{id}/abort", post(abort_run))
        .route("/api/recovery", get(get_recovery))
        .route("/api/recovery/resume", post(resume))
        .route("/api/recovery/discard", post(discard))
        .route("/api/serial/ports", get(serial_ports))
        .route("/api/serial/reconnect", post(serial_reconnect))
        .route("/api/serial/disconnect", post(serial_disconnect))
        .route("/api/scale/connect", post(scale_connect))
        .route("/api/scale/disconnect", post(scale_disconnect))
        .route("/api/pump/stop", post(pump_stop))
        .route("/api/pump/prime", post(pump_prime))
        .route("/api/pump/prime/stop", post(pump_prime_stop))
        .route("/api/scale/refill_mode", post(scale_refill_mode))
        .route("/api/scale/refill_done", post(scale_refill_done))
        .route("/api/refill/test", post(refill_test))
        .route("/api/refill/ping", post(refill_ping))
        .route("/api/refill/now", post(refill_now))
        .route("/api/refill/stop", post(refill_pause))
        .route("/api/calibrations", get(list_calibrations).post(create_calibration))
        .route(
            "/api/calibrations/draft",
            get(get_calibration_draft)
                .post(save_calibration_draft)
                .delete(delete_calibration_draft),
        )
        .route("/api/calibrations/{id}/archive", post(archive_calibration))
        .route("/api/calibrations/{id}/restore", post(restore_calibration))
        .route("/api/camera", get(camera_status))
        .route("/api/camera/devices", get(camera_devices))
        .route("/api/camera/connect", post(camera_connect))
        .route("/api/camera/disconnect", post(camera_disconnect))
        .route("/api/camera/controls", get(camera_controls))
        .route("/api/camera/control", post(camera_control))
        .route("/api/camera/controls/reset", post(camera_controls_reset))
        .route("/api/camera/fps", post(camera_fps))
        .route("/api/camera/picture", post(camera_picture))
        .route("/api/camera/captures", get(captures_info).post(captures_set))
        .route("/api/camera/interval", post(camera_interval))
        .route("/api/timelapses", post(timelapse_start))
        .route("/api/timelapses/current", get(timelapse_current))
        .route("/api/timelapses/current/stop", post(timelapse_stop))
        .route("/api/timelapses/current/pause", post(timelapse_pause))
        .route("/api/timelapses/current/resume", post(timelapse_resume))
        .route("/api/timelapses/current/interval", post(timelapse_interval))
        .route("/api/timelapses/{id}/images", get(timelapse_images))
        .route("/api/timelapses/images/{id}/{which}", get(timelapse_image_file))
        .route("/api/timelapses/images/{id}/show", post(timelapse_image_show))
        .route("/api/camera/frame.jpg", get(camera_frame))
        .route("/api/camera/live", get(camera_live))
        .route("/api/notify/test", post(notify_test))
        .route("/api/notify/pages", get(notify_pages))
        .route("/api/notify/ack", post(notify_ack))
        .route("/api/shutdown", post(shutdown))
        .route("/api/ws", get(ws_upgrade))
        .fallback(static_handler)
        .layer(cors_layer())
        .layer(middleware::from_fn(local_callers_only))
        .with_state(state)
}

/// The Tauri window's origins, the only cross-origin pages allowed in.
const TAURI_ORIGINS: [&str; 3] = ["http://tauri.localhost", "https://tauri.localhost", "tauri://localhost"];

/// Who may call the API. It listens on 127.0.0.1 only, yet any web page open
/// in this PC's browser can still send it requests: CORS keeps that page from
/// reading the answers, not from sending a body-less POST (a form, a
/// `no-cors` fetch) that stops or aborts a run or shuts the daemon down, nor
/// from opening the WebSocket. A DNS-rebinding page can even pass for the
/// same origin. So:
/// * `Host` must name this machine (`127.0.0.1` or `localhost`, any port: the
///   Vite dev proxy forwards its own),
/// * an `Origin`, which browsers send with every POST, every cross-origin
///   request and every WebSocket, must be this same host (the daemon's own
///   page, or the dev server behind its proxy) or the Tauri window.
///
/// Callers without an `Origin` (curl, the installer's PowerShell, the tray)
/// are not web pages and go through.
async fn local_callers_only(req: Request, next: Next) -> Response {
    match refusal(req.headers()) {
        None => next.run(req).await,
        Some(why) => {
            tracing::warn!("refused a request to {}: {why}", req.uri().path());
            (StatusCode::FORBIDDEN, Json(json!({ "error": why }))).into_response()
        }
    }
}

/// Why `local_callers_only` turns a request away, `None` to let it in.
fn refusal(headers: &HeaderMap) -> Option<&'static str> {
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    if let Some(host) = host {
        if !is_local_host(host) {
            return Some("this API answers only to 127.0.0.1 or localhost");
        }
    }
    let origin = headers.get(header::ORIGIN)?.to_str().ok();
    let allowed = origin.is_some_and(|o| {
        TAURI_ORIGINS.contains(&o)
            || host.is_some_and(|h| {
                o.strip_prefix("http://").or_else(|| o.strip_prefix("https://")) == Some(h)
            })
    });
    (!allowed).then_some("requests from other web pages are not accepted")
}

/// `127.0.0.1` or `localhost`, with or without a port.
fn is_local_host(host: &str) -> bool {
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => host,
    };
    name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1"
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
    /// A structured engine refusal: one-line `message`, a `code`, and an
    /// optional longer `hint` the UI reveals on demand.
    Detail(ErrDetail),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if let ApiError::Detail(d) = self {
            // Invalid input is 400; a hardware / lifecycle refusal is 409.
            let status = if d.code == "invalid_config" {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::CONFLICT
            };
            let mut body = json!({ "error": d.message, "code": d.code });
            if let Some(h) = d.hint {
                body["hint"] = json!(h);
            }
            return (status, Json(body)).into_response();
        }
        let (code, msg) = match self {
            ApiError::Down => (
                StatusCode::SERVICE_UNAVAILABLE,
                "control thread is not running".to_string(),
            ),
            ApiError::Bad(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            ApiError::Detail(_) => unreachable!("handled above"),
        };
        (code, Json(json!({ "error": msg }))).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

// ---------------------------------------------------------------------------
// handlers
// ---------------------------------------------------------------------------

/// "The daemon is up": answered by the HTTP server itself, never queued
/// behind the control thread (the desktop window polls this to decide
/// whether a daemon must be started; `/api/status` could be slow to answer
/// while the control thread waits on a device).
async fn health() -> Response {
    Json(json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") })).into_response()
}

/// Version, author and file locations, for Settings > About.
async fn about(State(s): State<AppState>) -> Response {
    let a = &s.about;
    let started_ms = a
        .started_at
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "authors": env!("CARGO_PKG_AUTHORS").split(':').collect::<Vec<_>>(),
        "license": env!("CARGO_PKG_LICENSE"),
        "homepage": env!("CARGO_PKG_HOMEPAGE"),
        "started_at_ms": started_ms,
        "data_dir": a.data_dir.display().to_string(),
        "config": s.config_path.display().to_string(),
        "db": a.db.display().to_string(),
        "log_dir": a.log_dir.display().to_string(),
    }))
    .into_response()
}

/// One of the daemon's own locations, as Settings > About names it, never a
/// path from the request: `(path, select)`, `select` for a file (its folder
/// opens with it picked), a folder opening as itself.
fn about_location(which: &str, a: &About, config: &std::path::Path) -> Option<(PathBuf, bool)> {
    match which {
        "data" => Some((a.data_dir.clone(), false)),
        "logs" => Some((a.log_dir.clone(), false)),
        "db" => Some((a.db.clone(), true)),
        "config" => Some((config.to_path_buf(), true)),
        _ => None,
    }
}

#[derive(Deserialize)]
struct OpenReq {
    which: String,
}

/// Opens one of those locations in the PC's file manager (Explorer), on the
/// desktop of the user the daemon runs as: Settings > About's "Open" buttons.
async fn open_location(State(s): State<AppState>, Json(req): Json<OpenReq>) -> ApiResult<Response> {
    let (path, select) = about_location(&req.which, &s.about, &s.config_path)
        .ok_or_else(|| ApiError::Bad(format!("unknown location `{}`", req.which)))?;
    if !path.exists() {
        return Err(ApiError::Conflict(format!("{} does not exist yet", path.display())));
    }
    open_in_file_manager(&path, select)
        .map_err(|e| ApiError::Conflict(format!("could not open the file manager: {e}")))?;
    Ok(Json(json!({ "opened": path.display().to_string() })).into_response())
}

/// Explorer, not waited on (it exits with 1 even when it opened the window).
/// `/select,` takes the path quoted as it is: Rust's own quoting of the whole
/// argument made Explorer open the Documents folder instead.
#[cfg(windows)]
fn open_in_file_manager(path: &std::path::Path, select: bool) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new("explorer.exe");
    if select {
        c.raw_arg(format!("/select,\"{}\"", path.display()));
    } else {
        c.raw_arg(format!("\"{}\"", path.display()));
    }
    c.spawn().map(|_| ())
}

#[cfg(not(windows))]
fn open_in_file_manager(path: &std::path::Path, select: bool) -> std::io::Result<()> {
    let dir = if select { path.parent().unwrap_or(path) } else { path };
    std::process::Command::new("xdg-open").arg(dir).spawn().map(|_| ())
}

async fn status(State(s): State<AppState>) -> ApiResult<Response> {
    let st = s
        .control
        .call(Command::Status)
        .await
        .map_err(|_| ApiError::Down)?;
    Ok(Json(st).into_response())
}

async fn get_config(State(s): State<AppState>) -> ApiResult<Response> {
    let cfg = s.config.read().await;
    let mut v = serde_json::to_value(&*cfg).map_err(|e| ApiError::Conflict(e.to_string()))?;
    v["_rev"] = json!(s.config_rev.load(Ordering::SeqCst));
    Ok(Json(v).into_response())
}

async fn put_config(State(s): State<AppState>, Json(raw): Json<serde_json::Value>) -> ApiResult<Response> {
    let rev = raw.get("_rev").and_then(|r| r.as_u64());
    let new: Config = serde_json::from_value(raw).map_err(|e| ApiError::Bad(e.to_string()))?;
    new.validate().map_err(ApiError::Bad)?;
    // One write at a time, checked against what the caller read.
    let mut current = s.config.write().await;
    if rev.is_some_and(|r| r != s.config_rev.load(Ordering::SeqCst)) {
        return Err(ApiError::Conflict(
            "the settings were changed meanwhile (another page, or Connect): reload them and save again".into(),
        ));
    }
    // `[refill]`: a model or head changed while a run drives it (or one that
    // did waits to be resumed) is refused (409). Only checked here: it is
    // applied once the file is saved, so neither a refused balance below nor
    // a failed save leaves the engine ahead of config.toml.
    s.control
        .call(|reply| Command::CheckRefill(new.refill.clone(), reply))
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    // A changed `[scale]` is applied live (Settings, Balance section), before
    // saving: a refusal (trimmed run active) must not leave the file ahead of
    // the engine.
    let old_scale = current.scale.clone();
    let scale_changed = old_scale.path.trim() != new.scale.path.trim()
        || old_scale.baud != new.scale.baud
        || old_scale.density_g_per_ml != new.scale.density_g_per_ml
        || old_scale.position != new.scale.position
        || old_scale.trim_limit_pct != new.scale.trim_limit_pct;
    let scale_connected = if scale_changed {
        Some(
            s.control
                .call(|reply| Command::SetScale(new.scale.clone(), reply))
                .await
                .map_err(|_| ApiError::Down)?
                .map_err(ApiError::Conflict)?,
        )
    } else {
        None
    };
    new.save(&s.config_path)
        .map_err(|e| ApiError::Bad(e.to_string()))?;
    *current = new.clone();
    s.config_rev.fetch_add(1, Ordering::SeqCst);
    drop(current);
    // Saved: now `[refill]` reaches the engine (a Stop goes to a moved
    // refill pump at once). Checked above; only a run started in between
    // can still refuse it, and then the engine keeps the pump it drives.
    s.control
        .call(|reply| Command::SetRefill(new.refill.clone(), reply))
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    // `serial.allow_simulator` gates Start/Resume in the live engine, so a save
    // that flips it must reach the control thread, otherwise the Settings
    // checkbox looks broken until the next reconnect.
    let _ = s
        .control
        .call(|reply| Command::SetAllowSimulator(new.serial.allow_simulator, reply))
        .await;
    // Same for `[resume] prompt` (Settings, Crash resume).
    let _ = s
        .control
        .call(|reply| Command::SetAutoResume(!new.resume.prompt, reply))
        .await;
    Ok(Json(json!({
        "saved": true,
        "scale_connected": scale_connected,
        "note": "pump port, baud and address changes apply on the next Connect; balance changes apply now; log level on restart"
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

async fn preview(Json(req): Json<PreviewReq>) -> ApiResult<Response> {
    if let Err(e) = req.curve.validate() {
        return Err(ApiError::Bad(e));
    }
    let samples = req.samples.clamp(2, 5000);
    let series: Vec<[f64; 2]> = tokio::task::spawn_blocking(move || {
        req.curve.preview(samples).into_iter().map(|(t, v)| [t, v]).collect()
    })
    .await
    .map_err(|e| ApiError::Conflict(format!("preview failed: {e}")))?;
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
    let limit = q.limit.clamp(1, 100_000);
    let runs = s
        .db
        .call(move |db| db.list_runs(limit))
        .await?
        .map_err(|e| ApiError::Conflict(e.to_string()))?;
    Ok(Json(runs).into_response())
}

#[derive(Deserialize)]
struct NotifyTest {
    #[serde(default)]
    name: String,
    /// The channels as typed in Settings (before a save); when both are
    /// absent, the saved ones of `name`.
    #[serde(default)]
    ntfy_topic: Option<String>,
    #[serde(default)]
    webhook: Option<String>,
    /// A test alarm instead: it rings every minute until acknowledged.
    #[serde(default)]
    alarm: bool,
}

/// Send a test message on every channel of a person, so they can check it
/// reaches their phone / chat from Settings. Off the async workers (a
/// blocking client, up to its own timeout).
async fn notify_test(State(s): State<AppState>, Json(t): Json<NotifyTest>) -> ApiResult<Response> {
    let notify = s.config.read().await.notify.clone();
    let typed = crate::config::Person {
        name: t.name.clone(),
        ntfy_topic: t.ntfy_topic.unwrap_or_default(),
        webhook: t.webhook.unwrap_or_default(),
    };
    let person = if typed.ntfy_topic.trim().is_empty() && typed.webhook.trim().is_empty() {
        notify
            .person(&t.name)
            .cloned()
            .ok_or_else(|| ApiError::Bad(format!("\"{}\" is not in Settings, Notifications", t.name)))?
    } else {
        typed
    };
    let check = crate::config::NotifyConfig {
        people: vec![crate::config::Person { name: "x".into(), ..person.clone() }],
        ntfy_server: notify.ntfy_server.clone(),
    };
    check.validate().map_err(ApiError::Bad)?;
    let targets = crate::notify::targets(&person, &notify.ntfy_server);
    let name = if t.name.trim().is_empty() { "You".to_string() } else { t.name.trim().to_string() };
    if t.alarm {
        let ntfy = targets
            .into_iter()
            .find(|t| matches!(t, crate::notify::Target::Ntfy { .. }))
            .ok_or_else(|| ApiError::Bad("a test alarm needs an ntfy topic".into()))?;
        let mut pages = s.pages.lock().unwrap_or_else(|p| p.into_inner());
        pages.retain(|p| p.run_id != crate::notify::TEST_RUN);
        pages.push(crate::notify::test_page(&name, ntfy));
        return Ok(Json(json!({ "sent": true })).into_response());
    }
    let errors = tokio::task::spawn_blocking(move || {
        let note = crate::notify::test_note(&name);
        targets
            .iter()
            .filter_map(|t| crate::notify::deliver(t, &note).err())
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|_| ApiError::Down)?;
    if !errors.is_empty() {
        return Err(ApiError::Conflict(format!("test not delivered: {}", errors.join("; "))));
    }
    Ok(Json(json!({ "sent": true })).into_response())
}

/// The alarms still ringing, for the UI's banner.
async fn notify_pages(State(s): State<AppState>) -> Json<Vec<crate::notify::Page>> {
    let pages = s.pages.lock().unwrap_or_else(|p| p.into_inner());
    Json(pages.iter().filter(|p| p.acked_by.is_none()).cloned().collect())
}

#[derive(Deserialize)]
struct NotifyAck {
    run_id: i64,
}

/// Acknowledge a run's ringing alarms from the UI. The notifier journals it
/// and stops the reminders on its next pass (within seconds).
async fn notify_ack(State(s): State<AppState>, Json(a): Json<NotifyAck>) -> ApiResult<Response> {
    let at = jiff::Timestamp::now().as_second();
    let mut pages = s.pages.lock().unwrap_or_else(|p| p.into_inner());
    if !crate::notify::apply_ack(&mut pages, a.run_id, at, "the Fermentool UI") {
        return Err(ApiError::NotFound);
    }
    Ok(Json(json!({ "acknowledged": true })).into_response())
}

#[derive(Deserialize)]
struct RunTimelapseReq {
    interval_min: u32,
}

/// `POST /api/runs`: the run config, plus `timelapse: {interval_min}` to
/// record one with it. A timelapse that cannot start never fails the run:
/// the answer carries `timelapse_error`.
async fn create_run(State(s): State<AppState>, Json(mut body): Json<serde_json::Value>) -> ApiResult<Response> {
    let timelapse = body.as_object_mut().and_then(|o| o.remove("timelapse")).filter(|t| !t.is_null());
    let timelapse: Option<RunTimelapseReq> = timelapse
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| ApiError::Bad(format!("timelapse: {e}")))?;
    if let Some(t) = &timelapse {
        if !crate::config::TIMELAPSE_MINUTES.contains(&t.interval_min) {
            return Err(ApiError::Bad("timelapse interval must be 1 to 1440 minutes".into()));
        }
    }
    let cfg: RunConfig = serde_json::from_value(body).map_err(|e| ApiError::Bad(e.to_string()))?;
    let (kind, name) = (cfg.kind, cfg.name.clone());
    let (status, mut answer) = start_run(&s, cfg).await?;
    // calibration runs never have one
    if let (Some(t), RunKind::Dosing) = (timelapse, kind) {
        let id = answer["run_id"].as_i64().unwrap_or_default();
        let tl = s.timelapses.clone();
        let started = tokio::task::spawn_blocking(move || tl.start_with_run(id, name, i64::from(t.interval_min) * 60))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        if let Err(e) = started {
            tracing::warn!(run = id, "the run started without its timelapse: {e}");
            answer["timelapse_error"] = json!(e);
        }
    }
    Ok((status, Json(answer)).into_response())
}

async fn start_run(s: &AppState, mut cfg: RunConfig) -> ApiResult<(StatusCode, serde_json::Value)> {
    // A dosing run names who it belongs to, so its alerts reach them and no
    // one else: required once anyone is set up for notifications. The name
    // is stored as configured (its case), a calibration burst goes without.
    if cfg.kind == RunKind::Dosing {
        let notify = s.config.read().await.notify.clone();
        if !notify.people.is_empty() {
            let given = cfg.responsible.as_deref().unwrap_or("").trim().to_string();
            let Some(p) = notify.person(&given) else {
                return Err(ApiError::Bad(if given.is_empty() {
                    "choose who is responsible for this run (their alerts go to them)".into()
                } else {
                    format!("\"{given}\" is not in Settings, Notifications")
                }));
            };
            cfg.responsible = Some(p.name.trim().to_string());
        }
    } else {
        cfg.responsible = None;
    }
    // The journal records the address the pump is really driven at.
    cfg.pump_addr = s.config.read().await.pump.address;
    let id = s
        .control
        .call(|reply| Command::StartRun(cfg, reply))
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Detail)?;
    Ok((StatusCode::CREATED, json!({ "run_id": id })))
}

#[derive(serde::Deserialize)]
struct RateBody {
    value: f64,
}

/// Change the rate of the active Constant run. Refusals come back as for a
/// start (`{ error, code, hint }`).
async fn set_active_rate(State(s): State<AppState>, Json(b): Json<RateBody>) -> ApiResult<Response> {
    let (from, to) = s
        .control
        .call(|reply| Command::SetConstantRate(b.value, reply))
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Detail)?;
    Ok(Json(json!({ "from": from, "to": to })).into_response())
}

async fn clear_history(State(s): State<AppState>) -> ApiResult<Response> {
    s.control
        .call(Command::ClearHistory)
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    // the captures go with the history (the one recording stays)
    let tl = s.timelapses.clone();
    if let Ok(Err(e)) = tokio::task::spawn_blocking(move || tl.delete_all()).await {
        tracing::warn!("captures not all deleted: {e}");
    }
    Ok(Json(json!({ "cleared": true })).into_response())
}

async fn delete_run(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Response> {
    let found = s
        .control
        .call(|reply| Command::DeleteRun(id, reply))
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    if !found {
        return Err(ApiError::NotFound);
    }
    // its timelapse's images go with it
    let tl = s.timelapses.clone();
    if let Ok(Err(e)) = tokio::task::spawn_blocking(move || tl.delete_for_run(id)).await {
        tracing::warn!(run = id, "the run's timelapse not deleted: {e}");
    }
    Ok(Json(json!({ "deleted": id })).into_response())
}

/// Delivered vs requested feed for a run: chart points, R², fitted µ. 404 when
/// the run has no balance data.
async fn get_tracking(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Response> {
    let rho = s.config.read().await.scale.density_g_per_ml;
    let report = s
        .db
        .call(move |db| crate::engine::tracking_report(db, id, rho))
        .await?
        .map_err(|e| ApiError::Conflict(e.to_string()))?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(report).into_response())
}

async fn get_run(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Response> {
    let run = s
        .db
        .call(move |db| db.run(id))
        .await?
        .map_err(|e| ApiError::Conflict(e.to_string()))?
        .ok_or(ApiError::NotFound)?;
    // The curve the run really followed, rate changes included.
    let effective = run.effective_curve();
    let mut body = serde_json::to_value(&run).map_err(|e| ApiError::Conflict(e.to_string()))?;
    body["effective_curve"] = serde_json::to_value(effective).map_err(|e| ApiError::Conflict(e.to_string()))?;
    // its timelapse, for Run again and the run detail
    let tl = s.db.call(move |db| db.timelapse_of_run(id)).await?.map_err(|e| ApiError::Conflict(e.to_string()))?;
    body["timelapse"] = json!(tl.map(|t| json!({ "id": t.id, "interval_s": t.interval_s, "images": t.images })));
    Ok(Json(body).into_response())
}

#[derive(Deserialize)]
struct TickQuery {
    #[serde(default)]
    from: i64,
    #[serde(default = "i64_max")]
    to: i64,
    /// About this many ticks spread over the whole run instead of a range
    /// (`from`/`to` ignored), for a chart.
    #[serde(default)]
    sample: Option<i64>,
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
    let sample = q.sample.map(|n| n.clamp(2, MAX_TICKS_SPAN));
    let ticks = s
        .db
        .call(move |db| match sample {
            Some(n) => db.ticks_sampled(id, n),
            None => db.ticks(id, from, to),
        })
        .await?
        .map_err(|e| ApiError::Conflict(e.to_string()))?;
    Ok(Json(ticks).into_response())
}

async fn get_events(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<LimitQuery>,
) -> ApiResult<Response> {
    let limit = q.limit.clamp(1, 5000);
    let events = s
        .db
        .call(move |db| db.events(Some(id), limit))
        .await?
        .map_err(|e| ApiError::Conflict(e.to_string()))?;
    Ok(Json(events).into_response())
}

async fn stop_run(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Response> {
    s.control
        .call(|reply| Command::StopRun(Some(id), reply))
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(json!({ "stopped": true })).into_response())
}

async fn abort_run(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Response> {
    s.control
        .call(|reply| Command::AbortRun(Some(id), reply))
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

#[derive(Deserialize)]
struct PrimeBody {
    direction: crate::config::Direction,
}

/// Calibration: prime the feed line at the pump's top speed, `direction`
/// (asked again the other way while priming, the pump turns around), until
/// `/api/pump/prime/stop` or 5 min. Refused during a run.
async fn pump_prime(State(s): State<AppState>, Json(b): Json<PrimeBody>) -> ApiResult<Response> {
    s.control
        .call(|reply| Command::PrimeStart(b.direction, reply))
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Detail)?;
    Ok(Json(json!({ "priming": true, "max_s": crate::engine::PRIME_MAX.as_secs() })).into_response())
}

async fn pump_prime_stop(State(s): State<AppState>) -> ApiResult<Response> {
    s.control
        .call(Command::PrimeStop)
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Detail)?;
    Ok(Json(json!({ "priming": false })).into_response())
}

#[derive(Deserialize)]
struct CalibrationQuery {
    lot_id: Option<String>,
}

/// Tubing calibrations, newest first; `?lot_id=` narrows it (blank = any).
async fn list_calibrations(
    State(s): State<AppState>,
    Query(q): Query<CalibrationQuery>,
) -> ApiResult<Response> {
    let nonblank = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let lot_id = nonblank(q.lot_id);
    let rows = s
        .db
        .call(move |db| db.list_calibrations(lot_id.as_deref()))
        .await?
        .map_err(|e| ApiError::Conflict(e.to_string()))?;
    Ok(Json(rows).into_response())
}

/// Record a tubing calibration from three finished bursts. Every derived
/// number is recomputed server-side from the runs' own clocks.
async fn create_calibration(
    State(s): State<AppState>,
    Json(mut cal): Json<NewCalibration>,
) -> ApiResult<Response> {
    let row = s
        .db
        .call(move |db| {
            // A calibration belongs to the pump its three measurements ran
            // on, as each run recorded it: never the body, nor the feed pump
            // configured by the time it is recorded.
            cal.pump_model = db.measurements_pump_model(cal.run_ids)?;
            db.insert_calibration(&cal).and_then(|id| {
                // Only one session is ever in progress: once it is recorded
                // the draft has served its purpose.
                let _ = db.set_state(CALIBRATION_DRAFT_KEY, "");
                db.calibration(id)?
                    .ok_or_else(|| StoreError::Invalid(format!("calibration {id} vanished")))
            })
        })
        .await?
        .map_err(|e| match e {
            StoreError::Invalid(m) => ApiError::Bad(m),
            other => ApiError::Conflict(other.to_string()),
        })?;
    Ok((StatusCode::CREATED, Json(row)).into_response())
}

/// Retire a tube: hidden from New run, kept for the runs that used it.
async fn archive_calibration(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Response> {
    set_calibration_archived(&s, id, true).await
}

async fn restore_calibration(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Response> {
    set_calibration_archived(&s, id, false).await
}

async fn set_calibration_archived(s: &AppState, id: i64, archived: bool) -> ApiResult<Response> {
    let row = s
        .db
        .call(move |db| db.set_calibration_archived(id, archived))
        .await?
        .map_err(|e| ApiError::Conflict(e.to_string()))?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(row).into_response())
}

/// The in-progress calibration session, or `null` when there is none.
async fn get_calibration_draft(State(s): State<AppState>) -> ApiResult<Response> {
    let draft = s
        .db
        .call(|db| db.get_state(CALIBRATION_DRAFT_KEY))
        .await?
        .map_err(|e| ApiError::Conflict(e.to_string()))?
        .filter(|d| !d.is_empty());
    let value = draft
        .and_then(|d| serde_json::from_str::<serde_json::Value>(&d).ok())
        .unwrap_or(serde_json::Value::Null);
    Ok(Json(value).into_response())
}

async fn save_calibration_draft(
    State(s): State<AppState>,
    Json(draft): Json<serde_json::Value>,
) -> ApiResult<Response> {
    set_calibration_draft(&s, (!draft.is_null()).then(|| draft.to_string())).await
}

async fn delete_calibration_draft(State(s): State<AppState>) -> ApiResult<Response> {
    set_calibration_draft(&s, None).await
}

async fn set_calibration_draft(s: &AppState, draft: Option<String>) -> ApiResult<Response> {
    s.db
        .call(move |db| db.set_state(CALIBRATION_DRAFT_KEY, draft.as_deref().unwrap_or("")))
        .await?
        .map_err(|e| ApiError::Conflict(e.to_string()))?;
    Ok(Json(json!({ "ok": true })).into_response())
}

async fn scale_refill_mode(State(s): State<AppState>) -> ApiResult<Response> {
    s.control
        .call(Command::TriggerRefillMode)
        .await
        .map_err(|_| ApiError::Down)?;
    Ok(Json(json!({ "ok": true })).into_response())
}

async fn scale_refill_done(State(s): State<AppState>) -> ApiResult<Response> {
    s.control
        .call(Command::TriggerRefillDone)
        .await
        .map_err(|_| ApiError::Down)?;
    Ok(Json(json!({ "ok": true })).into_response())
}

/// Settings' Test of the refill pump: 3 s at its speed and direction, then a
/// Stop. The reply comes once it started (or did not answer).
async fn refill_test(State(s): State<AppState>) -> ApiResult<Response> {
    s.control
        .call(Command::RefillTest)
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(json!({ "ok": true, "seconds": crate::engine::REFILL_TEST_S })).into_response())
}

/// Settings > Pumps: whether the refill pump answers at its address.
async fn refill_ping(State(s): State<AppState>) -> ApiResult<Response> {
    let responding = s
        .control
        .call(Command::RefillPing)
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(json!({ "responding": responding })).into_response())
}

async fn refill_now(State(s): State<AppState>) -> ApiResult<Response> {
    s.control
        .call(Command::RefillNow)
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(json!({ "ok": true })).into_response())
}

async fn refill_pause(State(s): State<AppState>) -> ApiResult<Response> {
    s.control
        .call(Command::RefillPause)
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(json!({ "ok": true })).into_response())
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
        .map_err(ApiError::Detail)?;
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
    // The webcam helper goes first, off the async workers (`shutdown` waits
    // up to ~1.5 s for it); the daemon's exit path does it again, harmlessly.
    let camera = s.camera.clone();
    let _ = tokio::task::spawn_blocking(move || camera.shutdown()).await;
    let timelapses = s.timelapses.clone();
    let _ = tokio::task::spawn_blocking(move || timelapses.shutdown()).await;
    s.shutdown.notify_one();
    s.stopping.send_replace(true);
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
            "UI not built, run `npm run build` in ui/",
        )
            .into_response(),
    }
}

async fn serial_ports() -> ApiResult<Response> {
    // Enumerating can take a few hundred ms on Windows: off the async workers.
    let ports = tokio::task::spawn_blocking(available_ports)
        .await
        .map_err(|e| ApiError::Conflict(e.to_string()))?
        .map_err(|e| ApiError::Conflict(e.to_string()))?;
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
    /// The feed pump's model, `"labq"`, `"labv"` or `"pds2"`; omitted = keep.
    #[serde(default)]
    pump_model: Option<String>,
    /// PDS II head, 1..=8; omitted = keep.
    #[serde(default)]
    pump_head: Option<u8>,
    // No max rpm: it is the model's. A `pump_max_rpm` sent by an older page
    // is ignored.
}

/// Persist `serial.*` to `config.toml` and rebuild the live pump transport
/// without a daemon restart. Refused (409) while a run is active.
async fn serial_reconnect(
    State(s): State<AppState>,
    Json(req): Json<ReconnectReq>,
) -> ApiResult<Response> {
    // Held to the end: a Settings save cannot interleave with this one.
    let mut current = s.config.write().await;
    let mut cfg = current.clone();
    if let Some(p) = req.path {
        cfg.serial.path = p;
    }
    if let Some(b) = req.baud {
        cfg.serial.baud = b;
    }
    if let Some(a) = req.pump_addr {
        cfg.pump.address = a;
    }
    if let Some(m) = req.pump_model {
        cfg.pump.model = match m.as_str() {
            "labq" => PumpModelCfg::Labq,
            "labv" => PumpModelCfg::Labv,
            "pds2" => PumpModelCfg::Pds2,
            _ => return Err(ApiError::Bad(format!("\"{m}\" is not a pump model (labq, labv, pds2)"))),
        };
    }
    if let Some(h) = req.pump_head {
        cfg.pump.head = h;
    }
    // Only what a reconnect changes (the baud, the feed pump's address per
    // model and head), refused before anything changes. The rest
    // of the config is PUT /api/config's business: a bad [notify] must not
    // stop the pump from connecting.
    if !SERIAL_BAUDS.contains(&cfg.serial.baud) {
        return Err(ApiError::Bad(format!("the pump baud must be one of {SERIAL_BAUDS:?}")));
    }
    if let Some(p) = cfg.pump.problem() {
        return Err(ApiError::Bad(p));
    }

    // Swap first, a refused reconnect (e.g. a run is active) must not touch
    // `config.toml`, so the file always matches the live transport.
    let serial = cfg.serial.clone();
    let feed = cfg.pump.target();
    let msg = s
        .control
        .call(move |reply| Command::Reconnect {
            serial,
            feed,
            reply,
        })
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;

    cfg.save(&s.config_path)
        .map_err(|e| ApiError::Bad(e.to_string()))?;
    *current = cfg;
    s.config_rev.fetch_add(1, Ordering::SeqCst);

    Ok(Json(json!({ "connected": msg })).into_response())
}

/// Settings > Pumps' Disconnect: closes the cable's COM port (frees it for
/// the LabQ's own software) by switching the live link to the simulator,
/// which auto-reconnect leaves alone. `config.toml` keeps the port, so
/// Connect, or a restart of the daemon, opens it again. Refused during a run.
async fn serial_disconnect(State(s): State<AppState>) -> ApiResult<Response> {
    let cfg = s.config.read().await.clone();
    let mut serial = cfg.serial.clone();
    serial.path = "sim".into();
    let feed = cfg.pump.target();
    s.control
        .call(move |reply| Command::Reconnect { serial, feed, reply })
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(json!({ "disconnected": cfg.serial.path })).into_response())
}

/// Settings > Balance's Connect: opens the saved balance port again (after a
/// Disconnect, or a balance that was off). `connected`: it answers already.
async fn scale_connect(State(s): State<AppState>) -> ApiResult<Response> {
    let scale = s.config.read().await.scale.clone();
    if scale.path.trim().is_empty() {
        return Err(ApiError::Conflict("no balance port saved: pick its port and Save".into()));
    }
    let connected = s
        .control
        .call(move |reply| Command::SetScale(scale, reply))
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(json!({ "connected": connected })).into_response())
}

/// Settings > Balance's Disconnect: closes the balance's COM port, the saved
/// config keeping it (Connect, or a restart of the daemon, opens it again).
/// Refused during a run corrected by the balance.
async fn scale_disconnect(State(s): State<AppState>) -> ApiResult<Response> {
    let saved = s.config.read().await.scale.clone();
    let mut scale = saved.clone();
    scale.path = String::new();
    s.control
        .call(move |reply| Command::SetScale(scale, reply))
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;
    Ok(Json(json!({ "disconnected": saved.path })).into_response())
}

// ---------------------------------------------------------------------------
// camera
// ---------------------------------------------------------------------------

/// `GET /api/camera`: the supervisor's state, whether a camera is saved, the
/// live viewers counted and the latest picture's time (`frame_at`, null
/// before any): with them the page tells a frozen picture from a live one.
async fn camera_status(State(s): State<AppState>) -> ApiResult<Response> {
    let mut v = serde_json::to_value(s.camera.state()).map_err(|e| ApiError::Conflict(e.to_string()))?;
    if s.helper_missing {
        // Even with no want yet: Settings must say why Connect cannot work.
        v = json!({ "state": "no_helper" });
    }
    let configured = s.config.read().await.camera.as_ref().is_some_and(|c| !c.device.trim().is_empty());
    v["configured"] = json!(configured);
    v["viewers"] = json!(s.camera.viewers());
    v["frame_at"] = json!(s.camera.latest().map(|f| f.at.to_string()));
    Ok(Json(v).into_response())
}

#[derive(Deserialize)]
struct DevicesQuery {
    #[serde(default)]
    refresh: Option<String>,
}

/// `GET /api/camera/devices?refresh=1`: the PC's cameras, listed by a helper
/// of their own, on the blocking pool.
async fn camera_devices(State(s): State<AppState>, Query(q): Query<DevicesQuery>) -> ApiResult<Response> {
    let refresh = q.refresh.as_deref().is_some_and(|r| !matches!(r, "" | "0" | "false"));
    let camera = s.camera.clone();
    let list = tokio::task::spawn_blocking(move || camera.devices(refresh))
        .await
        .map_err(|e| ApiError::Conflict(format!("camera listing failed: {e}")))?
        .map_err(ApiError::Conflict)?;
    let list: Vec<_> = list
        .into_iter()
        .map(|d| {
            let sizes: Vec<[u32; 2]> = d.sizes.iter().map(|&(w, h)| [w, h]).collect();
            json!({ "name": d.name, "index": d.index, "sizes": sizes })
        })
        .collect();
    Ok(Json(list).into_response())
}

#[derive(Deserialize)]
struct CameraConnectReq {
    device: String,
    #[serde(default)]
    index: u32,
    width: u32,
    height: u32,
    #[serde(rename = "_rev", default)]
    rev: Option<u64>,
}

/// `POST /api/camera/connect`: saves `[camera]` (same revision check and
/// atomic write as `PUT /api/config`), then asks the supervisor to connect.
/// `set_want` runs only here and in Disconnect, never on a config save.
async fn camera_connect(State(s): State<AppState>, Json(req): Json<CameraConnectReq>) -> ApiResult<Response> {
    if req.device.trim().is_empty() {
        return Err(ApiError::Bad("pick a camera".into()));
    }
    let mut current = s.config.write().await;
    // the rest of `[camera]` (test pattern, live view, saved controls) stays
    let cam = crate::config::CameraConfig {
        device: req.device.clone(),
        device_index: req.index,
        width: req.width,
        height: req.height,
        enabled: true,
        ..current.camera.clone().unwrap_or_default()
    };
    if let Some(p) = cam.problem() {
        return Err(ApiError::Bad(p));
    }
    if req.rev.is_some_and(|r| r != s.config_rev.load(Ordering::SeqCst)) {
        return Err(ApiError::Conflict(
            "the settings were changed meanwhile (another page): reload them and connect again".into(),
        ));
    }
    let controls = cam.controls_for(&cam.device);
    let mut new = current.clone();
    new.camera = Some(cam);
    new.save(&s.config_path).map_err(|e| ApiError::Bad(e.to_string()))?;
    *current = new;
    s.config_rev.fetch_add(1, Ordering::SeqCst);
    // Still under the write lock (a channel send): two Connects at once
    // cannot leave the file naming one camera and the supervisor another.
    s.camera.set_want(Some(crate::camera::CameraWant {
        device: req.device,
        index: req.index,
        width: req.width,
        height: req.height,
        controls,
    }));
    drop(current);
    Ok(Json(json!({ "connecting": true })).into_response())
}

/// `POST /api/camera/disconnect`: frees the camera and keeps the device in
/// `config.toml` with `enabled = false`, so the daemon does not connect it at
/// the next start.
async fn camera_disconnect(State(s): State<AppState>) -> ApiResult<Response> {
    let mut current = s.config.write().await;
    if current.camera.as_ref().is_some_and(|c| c.enabled) {
        let mut new = current.clone();
        if let Some(c) = new.camera.as_mut() {
            c.enabled = false;
        }
        new.save(&s.config_path).map_err(|e| ApiError::Bad(e.to_string()))?;
        *current = new;
        s.config_rev.fetch_add(1, Ordering::SeqCst);
    }
    // Saved first: a failed save must not leave the camera off with the file
    // still enabled.
    s.camera.set_want(None);
    drop(current);
    Ok(Json(json!({ "disconnected": true })).into_response())
}

/// A camera request on the blocking pool (it waits on the supervisor), its
/// refusal a 409 with the camera's own words.
async fn camera_blocking<T: Send + 'static>(
    s: &AppState,
    f: impl FnOnce(crate::camera::Camera) -> Result<T, String> + Send + 'static,
) -> ApiResult<T> {
    let camera = s.camera.clone();
    tokio::task::spawn_blocking(move || f(camera))
        .await
        .map_err(|e| ApiError::Conflict(format!("camera request failed: {e}")))?
        .map_err(ApiError::Conflict)
}

/// `GET /api/camera/controls`: the connected camera's picture controls, as
/// it has them now (only those it has).
async fn camera_controls(State(s): State<AppState>) -> ApiResult<Response> {
    let controls = camera_blocking(&s, |c| c.controls()).await?;
    Ok(Json(json!({ "controls": controls })).into_response())
}

#[derive(Deserialize)]
struct CameraControlReq {
    #[serde(flatten)]
    value: fermentool_camera::protocol::ControlValue,
    /// false while a slider moves: set on the camera, saved at its release
    #[serde(default = "yes")]
    save: bool,
}

fn yes() -> bool {
    true
}

/// `POST /api/camera/control {control, value, auto, save}`: set on the
/// camera first, then (with `save`) kept in `[camera.controls]` for this
/// camera: a value the camera refuses is never saved.
async fn camera_control(State(s): State<AppState>, Json(req): Json<CameraControlReq>) -> ApiResult<Response> {
    let v = req.value;
    let controls = camera_blocking(&s, move |c| c.set_control(v)).await?;
    if req.save {
        save_camera_controls(&s, |saved| {
            saved.insert(
                v.control.name().to_string(),
                crate::config::SavedControl { value: v.value, auto: v.auto },
            );
        })
        .await?;
    }
    Ok(Json(json!({ "controls": controls })).into_response())
}

/// `POST /api/camera/controls/reset`: every control back to the camera's
/// default (auto where it has one), and none saved for this camera.
async fn camera_controls_reset(State(s): State<AppState>) -> ApiResult<Response> {
    let controls = camera_blocking(&s, |c| {
        let mut now = c.controls()?;
        for i in now.clone() {
            let v = fermentool_camera::protocol::ControlValue { control: i.control, value: i.default, auto: i.can_auto };
            // a refused one leaves the others reset
            match c.set_control(v) {
                Ok(after) => now = after,
                Err(e) => tracing::warn!(control = i.control.name(), "camera control not reset: {e}"),
            }
        }
        Ok(now)
    })
    .await?;
    save_camera_controls(&s, |saved| saved.clear()).await?;
    Ok(Json(json!({ "controls": controls })).into_response())
}

/// Changes the saved controls of the camera in `[camera]` and writes the
/// file (same atomic write as `PUT /api/config`).
async fn save_camera_controls(
    s: &AppState,
    change: impl FnOnce(&mut std::collections::BTreeMap<String, crate::config::SavedControl>),
) -> ApiResult<()> {
    let mut current = s.config.write().await;
    let mut new = current.clone();
    let Some(cam) = new.camera.as_mut() else {
        return Err(ApiError::Conflict("no camera is saved".into()));
    };
    let device = cam.device.clone();
    let saved = cam.controls.entry(device.clone()).or_default();
    change(saved);
    if saved.is_empty() {
        cam.controls.remove(&device);
    }
    new.save(&s.config_path).map_err(|e| ApiError::Bad(e.to_string()))?;
    *current = new;
    s.config_rev.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

#[derive(Deserialize)]
struct CameraFpsReq {
    fps: u32,
}

/// `POST /api/camera/fps {fps}`: live view pictures a second, saved and
/// used from the next picture.
async fn camera_fps(State(s): State<AppState>, Json(req): Json<CameraFpsReq>) -> ApiResult<Response> {
    if !crate::config::CAMERA_FPS.contains(&req.fps) {
        let (lo, hi) = (crate::config::CAMERA_FPS.start(), crate::config::CAMERA_FPS.end());
        return Err(ApiError::Bad(format!("live view must be {lo} to {hi} pictures a second")));
    }
    let mut current = s.config.write().await;
    let mut new = current.clone();
    new.camera.get_or_insert_with(Default::default).live_fps = req.fps;
    new.save(&s.config_path).map_err(|e| ApiError::Bad(e.to_string()))?;
    *current = new;
    s.config_rev.fetch_add(1, Ordering::SeqCst);
    s.camera.set_live_fps(req.fps);
    Ok(Json(json!({ "fps": req.fps })).into_response())
}

// ---------------------------------------------------------------------------
// timelapses
// ---------------------------------------------------------------------------

/// A timelapse request on the blocking pool, its refusal a 409.
async fn timelapse_blocking<T: Send + 'static>(
    s: &AppState,
    f: impl FnOnce(crate::timelapse::Timelapses) -> Result<T, String> + Send + 'static,
) -> ApiResult<T> {
    let tl = s.timelapses.clone();
    tokio::task::spawn_blocking(move || f(tl))
        .await
        .map_err(|e| ApiError::Conflict(format!("timelapse request failed: {e}")))?
        .map_err(ApiError::Conflict)
}

fn interval_s(minutes: u32) -> ApiResult<i64> {
    if crate::config::TIMELAPSE_MINUTES.contains(&minutes) {
        Ok(i64::from(minutes) * 60)
    } else {
        Err(ApiError::Bad("timelapse interval must be 1 to 1440 minutes".into()))
    }
}

/// `GET /api/timelapses/current`: `{timelapse, warn_free}`, the timelapse
/// recording or paused (`null` without one) and whether its disk is under
/// 1 GB free.
async fn timelapse_current(State(s): State<AppState>) -> Response {
    let cur = s.timelapses.current();
    let warn_free = cur
        .as_ref()
        .and_then(|c| c.free_bytes)
        .is_some_and(|f| f < crate::timelapse::files::WARN_FREE_BYTES);
    Json(json!({ "timelapse": cur, "warn_free": warn_free })).into_response()
}

#[derive(Deserialize)]
struct TimelapseStartReq {
    name: String,
    interval_min: u32,
    #[serde(default)]
    stop_after_h: Option<f64>,
}

/// `POST /api/timelapses {name, interval_min, stop_after_h?}`: one on its
/// own (no run going).
async fn timelapse_start(State(s): State<AppState>, Json(req): Json<TimelapseStartReq>) -> ApiResult<Response> {
    let every = interval_s(req.interval_min)?;
    if req.name.trim().is_empty() {
        return Err(ApiError::Bad("give the timelapse a name".into()));
    }
    let stop_after_s = match req.stop_after_h {
        None => None,
        Some(h) if h.is_finite() && (0.1..=1000.0).contains(&h) => Some((h * 3600.0).round() as i64),
        Some(_) => return Err(ApiError::Bad("stop after must be 0.1 to 1000 h".into())),
    };
    let name = req.name.trim().to_string();
    let id = timelapse_blocking(&s, move |tl| tl.start_standalone(name, every, stop_after_s)).await?;
    Ok(Json(json!({ "id": id })).into_response())
}

async fn timelapse_stop(State(s): State<AppState>) -> ApiResult<Response> {
    timelapse_blocking(&s, |tl| tl.stop()).await?;
    Ok(Json(json!({ "stopped": true })).into_response())
}

async fn timelapse_pause(State(s): State<AppState>) -> ApiResult<Response> {
    timelapse_blocking(&s, |tl| tl.pause()).await?;
    Ok(Json(json!({ "paused": true })).into_response())
}

async fn timelapse_resume(State(s): State<AppState>) -> ApiResult<Response> {
    timelapse_blocking(&s, |tl| tl.resume()).await?;
    Ok(Json(json!({ "resumed": true })).into_response())
}

#[derive(Deserialize)]
struct IntervalReq {
    interval_min: u32,
}

/// `POST /api/timelapses/current/interval {interval_min}`: from the next
/// image on.
async fn timelapse_interval(State(s): State<AppState>, Json(req): Json<IntervalReq>) -> ApiResult<Response> {
    let every = interval_s(req.interval_min)?;
    timelapse_blocking(&s, move |tl| tl.set_interval(every)).await?;
    Ok(Json(json!({ "interval_s": every })).into_response())
}

/// `POST /api/camera/picture`: one image now, into the timelapse recording
/// or today's pictures.
async fn camera_picture(State(s): State<AppState>) -> ApiResult<Response> {
    let id = timelapse_blocking(&s, |tl| tl.take_picture()).await?;
    // the folder's name (a timelapse's, or "Pictures <day>"), never its path
    let folder = s
        .db
        .call(move |db| -> Result<Option<String>, crate::store::StoreError> {
            let Some(img) = db.image(id)? else { return Ok(None) };
            Ok(db.timelapse(img.timelapse_id)?.map(|t| t.name))
        })
        .await?
        .ok()
        .flatten();
    Ok(Json(json!({ "image_id": id, "folder": folder })).into_response())
}

#[derive(Deserialize)]
struct ImagesQuery {
    #[serde(default)]
    before: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
}

/// `GET /api/timelapses/{id}/images?before=&limit=`: newest first, no file
/// paths (images are fetched by id).
async fn timelapse_images(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<ImagesQuery>,
) -> ApiResult<Response> {
    let limit = q.limit.unwrap_or(8).clamp(1, 200);
    let rows = s
        .db
        .call(move |db| db.images(id, q.before, limit))
        .await?
        .map_err(|e| ApiError::Conflict(e.to_string()))?;
    let list: Vec<_> = rows
        .iter()
        .map(|i| json!({ "id": i.id, "slot": i.slot, "taken_at": i.taken_at, "elapsed_s": i.elapsed_s, "bytes": i.bytes }))
        .collect();
    Ok(Json(list).into_response())
}

/// `GET /api/timelapses/images/{id}/full.jpg` or `/thumb.jpg`: the file,
/// found from the image's row only.
async fn timelapse_image_file(State(s): State<AppState>, Path((id, which)): Path<(i64, String)>) -> ApiResult<Response> {
    let thumb = match which.as_str() {
        "full.jpg" => false,
        "thumb.jpg" => true,
        _ => return Err(ApiError::NotFound),
    };
    let path = image_path(&s, id, thumb).await?;
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(path))
        .await
        .map_err(|e| ApiError::Conflict(e.to_string()))?
        .map_err(|_| ApiError::NotFound)?;
    Ok(([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "max-age=31536000, immutable")], bytes)
        .into_response())
}

/// An image's file, found from its row only (never a path from a request).
async fn image_path(s: &AppState, id: i64, thumb: bool) -> ApiResult<std::path::PathBuf> {
    s.db
        .call(move |db| -> Result<Option<std::path::PathBuf>, crate::store::StoreError> {
            let Some(img) = db.image(id)? else { return Ok(None) };
            let Some(t) = db.timelapse(img.timelapse_id)? else { return Ok(None) };
            let folder = std::path::PathBuf::from(t.folder);
            Ok(Some(if thumb { folder.join("thumbs").join(img.thumb) } else { folder.join(img.file) }))
        })
        .await?
        .map_err(|e| ApiError::Conflict(e.to_string()))?
        .ok_or(ApiError::NotFound)
}

/// `POST /api/timelapses/images/{id}/show`: Explorer opens the image's
/// folder with the image picked (the Camera page's "Show in folder").
async fn timelapse_image_show(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Response> {
    let path = image_path(&s, id, false).await?;
    if !path.exists() {
        return Err(ApiError::NotFound);
    }
    open_in_file_manager(&path, true)
        .map_err(|e| ApiError::Conflict(format!("could not open the file manager: {e}")))?;
    Ok(Json(json!({ "opened": true })).into_response())
}

/// The data dir new timelapses go under when `captures_dir` is empty.
fn captures_root(s: &AppState, dir: &str) -> std::path::PathBuf {
    crate::timelapse::files::captures_root(dir, &s.about.data_dir)
}

/// `GET /api/camera/captures`: `{folder, used_bytes, free_bytes}`, the
/// folder new timelapses go in, what the timelapses kept there take, and the
/// free space on its disk.
async fn captures_info(State(s): State<AppState>) -> ApiResult<Response> {
    let dir = s.config.read().await.camera.as_ref().map(|c| c.captures_dir.clone()).unwrap_or_default();
    let root = captures_root(&s, &dir);
    let under = root.clone();
    let used: i64 = s
        .db
        .call(move |db| db.all_timelapses())
        .await?
        .map_err(|e| ApiError::Conflict(e.to_string()))?
        .iter()
        .filter(|t| std::path::Path::new(&t.folder).starts_with(&under))
        .map(|t| t.bytes)
        .sum();
    let probe = root.clone();
    let free = tokio::task::spawn_blocking(move || crate::timelapse::files::free_bytes(&probe)).await.ok().flatten();
    Ok(Json(json!({ "folder": root.to_string_lossy(), "used_bytes": used, "free_bytes": free })).into_response())
}

#[derive(Deserialize)]
struct CapturesReq {
    folder: String,
}

/// `POST /api/camera/captures {folder}`: checked writable (created if
/// missing), then saved as `[camera] captures_dir`; empty: the default.
async fn captures_set(State(s): State<AppState>, Json(req): Json<CapturesReq>) -> ApiResult<Response> {
    let folder = req.folder.trim().to_string();
    let root = captures_root(&s, &folder);
    tokio::task::spawn_blocking(move || crate::timelapse::files::ensure_writable(&root))
        .await
        .map_err(|e| ApiError::Conflict(e.to_string()))?
        .map_err(ApiError::Conflict)?;
    let saved = folder.clone();
    save_camera(&s, move |c| c.captures_dir = saved).await?;
    Ok(Json(json!({ "folder": folder })).into_response())
}

/// `POST /api/camera/interval {interval_min}`: New run's default interval.
async fn camera_interval(State(s): State<AppState>, Json(req): Json<IntervalReq>) -> ApiResult<Response> {
    interval_s(req.interval_min)?;
    save_camera(&s, move |c| c.interval_min = req.interval_min).await?;
    Ok(Json(json!({ "interval_min": req.interval_min })).into_response())
}

/// Changes `[camera]` (created when absent) and writes the file.
async fn save_camera(s: &AppState, change: impl FnOnce(&mut crate::config::CameraConfig)) -> ApiResult<()> {
    let mut current = s.config.write().await;
    let mut new = current.clone();
    change(new.camera.get_or_insert_with(Default::default));
    new.save(&s.config_path).map_err(|e| ApiError::Bad(e.to_string()))?;
    *current = new;
    s.config_rev.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// `GET /api/camera/frame.jpg`: the latest picture.
async fn camera_frame(State(s): State<AppState>) -> Response {
    match s.camera.latest() {
        Some(f) => (
            [(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-store")],
            f.jpeg.as_ref().clone(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "no picture yet" }))).into_response(),
    }
}

/// How long the live view waits for a new picture before it repeats the last
/// one (keeps the connection and the browser's image alive).
const LIVE_KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(2);

/// A channel of body parts as a stream.
struct PartStream(tokio::sync::mpsc::Receiver<bytes::Bytes>);

impl futures_core::Stream for PartStream {
    type Item = Result<bytes::Bytes, std::convert::Infallible>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.0.poll_recv(cx).map(|o| o.map(Ok))
    }
}

fn mjpeg_part(jpeg: &[u8]) -> bytes::Bytes {
    let mut part = Vec::with_capacity(jpeg.len() + 80);
    part.extend_from_slice(
        format!("--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n", jpeg.len()).as_bytes(),
    );
    part.extend_from_slice(jpeg);
    part.extend_from_slice(b"\r\n");
    part.into()
}

/// `GET /api/camera/live`: `multipart/x-mixed-replace`, one part per new
/// picture. A task feeds the body: each wait for a picture is a
/// `spawn_blocking` (never an async worker), and the task, which owns the
/// `ViewerGuard`, ends the moment the body is dropped, so the viewer count
/// (and the supervisor's grabbing) stops with the browser tab.
async fn camera_live(State(s): State<AppState>) -> Response {
    let camera = s.camera.clone();
    let guard = camera.viewer();
    let (tx, rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(1);
    let mut stop = s.stopping.subscribe();
    tokio::spawn(async move {
        let _guard = guard;
        let mut last: Option<crate::camera::Frame> = None;
        let mut seq = 0u64;
        loop {
            let cam = camera.clone();
            let wait = tokio::task::spawn_blocking(move || cam.wait_frame_after(seq, LIVE_KEEPALIVE));
            let frame = tokio::select! {
                _ = tx.closed() => break,
                Ok(_) = stop.wait_for(|stopped| *stopped) => break,
                r = wait => match r {
                    Ok(f) => f,
                    Err(_) => break,
                },
            };
            let part = match frame {
                Some(f) => {
                    seq = f.seq;
                    let p = mjpeg_part(&f.jpeg);
                    last = Some(f);
                    p
                }
                // Nothing new within the bound: repeat the last picture, or
                // keep waiting if there has been none yet.
                None => match &last {
                    Some(f) => mjpeg_part(&f.jpeg),
                    None => continue,
                },
            };
            tokio::select! {
                _ = tx.closed() => break,
                Ok(_) = stop.wait_for(|stopped| *stopped) => break,
                sent = tx.send(part) => if sent.is_err() { break },
            }
        }
    });
    (
        [
            (header::CONTENT_TYPE, "multipart/x-mixed-replace; boundary=frame"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        axum::body::Body::from_stream(PartStream(rx)),
    )
        .into_response()
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

    /// A fresh journal file: the control thread and the API each open their
    /// own connection to it, as in the daemon (an in-memory database cannot
    /// be shared). Files of earlier test processes are swept on first use;
    /// this process's are still open, so they wait for the next run.
    fn test_db() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static CTR: AtomicU64 = AtomicU64::new(0);
        static SWEEP: std::sync::Once = std::sync::Once::new();
        let dir = std::env::temp_dir().join("fermentool-api-tests");
        let _ = std::fs::create_dir_all(&dir);
        let pid = std::process::id();
        SWEEP.call_once(|| {
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                if !e.file_name().to_string_lossy().starts_with(&format!("{pid}-")) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        });
        dir.join(format!("{pid}-{}.sqlite", CTR.fetch_add(1, Ordering::Relaxed)))
    }

    fn test_state() -> AppState {
        test_state_at(&test_db())
    }

    #[test]
    fn about_opens_its_own_four_locations_and_nothing_else() {
        let a = About {
            data_dir: "D:/data".into(),
            db: "D:/data/fermentool.sqlite".into(),
            log_dir: "D:/data/logs".into(),
            ..About::default()
        };
        let cfg = std::path::Path::new("D:/data/config.toml");
        assert_eq!(about_location("data", &a, cfg), Some(("D:/data".into(), false)));
        assert_eq!(about_location("logs", &a, cfg), Some(("D:/data/logs".into(), false)));
        assert_eq!(about_location("db", &a, cfg), Some(("D:/data/fermentool.sqlite".into(), true)));
        assert_eq!(about_location("config", &a, cfg), Some(("D:/data/config.toml".into(), true)));
        assert_eq!(about_location("C:/Windows", &a, cfg), None);
        assert_eq!(about_location("../data", &a, cfg), None);
    }

    #[tokio::test]
    async fn opening_an_unknown_location_is_refused() {
        let app = router(test_state());
        let res = app
            .oneshot(post_json("/api/about/open", json!({ "which": "C:/Windows/System32" })))
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    fn test_state_at(db: &std::path::Path) -> AppState {
        let engine = Engine::new(Pump::new(SimPump::new(1), 1), Store::open(db).unwrap(), "test");
        state_for(engine, db)
    }

    fn state_for(engine: Engine<SimPump>, db: &std::path::Path) -> AppState {
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
            pages: Default::default(),
            db: Arc::new(ApiDb::new(Store::open(db).unwrap())),
            config_rev: Default::default(),
            about: Default::default(),
            camera: crate::camera::Camera::spawn(crate::camera::fake::FakeLaunch::healthy().0, None),
            timelapses: test_timelapses(),
            helper_missing: false,
            stopping: Arc::new(tokio::sync::watch::channel(false).0),
        }
    }

    /// Like [`test_state`] but with `serial.allow_simulator = false` applied to
    /// the live engine, so `POST /api/runs` is refused with a hinted error.
    fn test_state_no_sim_runs() -> AppState {
        let db = test_db();
        let mut engine = Engine::new(Pump::new(SimPump::new(1), 1), Store::open(&db).unwrap(), "test");
        engine.set_serial(
            crate::config::SerialConfig {
                path: "sim".into(),
                baud: 9600,
                allow_simulator: false,
            },
            fermentool_modbus::PumpTarget::labq(1),
        );
        state_for(engine, &db)
    }

    #[tokio::test]
    async fn a_dosing_run_must_name_a_known_responsible_once_people_exist() {
        let state = test_state();
        state.config.write().await.notify.people.push(crate::config::Person {
            name: "Drew".into(),
            ntfy_topic: "fermentool-drew-x7k2".into(),
            ..Default::default()
        });
        let app = router(state);
        let body = |who: Option<&str>| {
            let mut b = json!({ "name": "t", "control_var": "rpm", "direction": "cw",
                                "pump_addr": 1, "curve": linear_curve() });
            if let Some(w) = who {
                b["responsible"] = json!(w);
            }
            b
        };
        for missing in [None, Some("  "), Some("Nobody")] {
            let res = app.clone().oneshot(post_json("/api/runs", body(missing))).await.unwrap();
            assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{missing:?}");
        }
        // Any case is accepted, the configured spelling is stored.
        let res = app.clone().oneshot(post_json("/api/runs", body(Some("drew")))).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let id = body_json(res).await["run_id"].as_i64().unwrap();
        let run = body_json(app.oneshot(get(&format!("/api/runs/{id}"))).await.unwrap()).await;
        assert_eq!(run["responsible"], json!("Drew"));
    }

    #[tokio::test]
    async fn without_anyone_configured_a_run_needs_no_responsible() {
        let app = router(test_state());
        let res = app
            .oneshot(post_json(
                "/api/runs",
                json!({ "name": "t", "control_var": "rpm", "direction": "cw",
                        "pump_addr": 1, "curve": linear_curve() }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn refused_start_carries_a_code_and_a_hint() {
        let app = router(test_state_no_sim_runs());
        let req = post_json(
            "/api/runs",
            json!({
                "name": "t", "control_var": "rpm", "direction": "cw",
                "pump_addr": 1,
                "curve": linear_curve()
            }),
        );
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let body = body_json(res).await;
        assert_eq!(body["code"], "simulator_not_allowed");
        assert!(body["error"].as_str().unwrap().to_lowercase().contains("simulator"));
        assert!(
            body["hint"].as_str().is_some_and(|h| h.contains("Settings")),
            "hint should be present and actionable: {body}"
        );
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
    async fn about_reports_version_author_and_paths() {
        let app = router(test_state());
        let res = app.oneshot(get("/api/about")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
        assert!(v["authors"][0].as_str().unwrap().contains("Andrew Zicler"));
        assert_eq!(v["license"], "MIT");
        assert_eq!(v["homepage"], "https://www.delvigne-lab.com");
        assert!(v["config"].as_str().unwrap().ends_with("ft-api-test.toml"));
        assert!(v["started_at_ms"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn status_reports_no_scale_fields_when_unconfigured() {
        let app = router(test_state());
        let res = app.oneshot(get("/api/status")).await.unwrap();
        let body = body_json(res).await;
        assert!(body["scale_ok"].as_bool().unwrap());
        assert!(body["scale_state"].is_null());
        assert!(body["trim_c"].is_null());
    }

    #[tokio::test]
    async fn refill_endpoints_toggle_the_manual_flags() {
        let app = router(test_state());
        let res = app
            .clone()
            .oneshot(post_json("/api/scale/refill_mode", json!({})))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        // The SPA fallback also answers an unmapped path with 200 (it serves
        // index.html for client-side routing), so the status code alone
        // can't tell a missing route from a real one, check the JSON body.
        assert_eq!(body_json(res).await["ok"], true);
        let res = app
            .oneshot(post_json("/api/scale/refill_done", json!({})))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_json(res).await["ok"], true);
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
        // "a run is already active" is a conflict, and carries a structured code.
        assert_eq!(res.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(res).await["code"], "busy");

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
    async fn a_settings_save_from_a_stale_page_is_refused() {
        let mut st = test_state();
        st.config_path = Arc::new(std::env::temp_dir().join(format!("ft-rev-{}.toml", std::process::id())));
        let app = router(st);
        let cfg = body_json(app.clone().oneshot(get("/api/config")).await.unwrap()).await;
        assert_eq!(cfg["_rev"], 0);
        let put = |body: serde_json::Value| {
            Request::builder()
                .method("PUT")
                .uri("/api/config")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        assert_eq!(app.clone().oneshot(put(cfg.clone())).await.unwrap().status(), StatusCode::OK);
        // The same, older read once more: someone saved in between.
        assert_eq!(app.clone().oneshot(put(cfg.clone())).await.unwrap().status(), StatusCode::CONFLICT);
        // An address out of range is refused, whatever the revision.
        let mut bad = body_json(app.clone().oneshot(get("/api/config")).await.unwrap()).await;
        bad["pump"]["address"] = json!(0);
        assert_eq!(app.oneshot(put(bad)).await.unwrap().status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_saved_refill_pump_is_applied_live_and_its_test_says_when_it_is_silent() {
        let mut st = test_state();
        st.config_path = Arc::new(std::env::temp_dir().join(format!("ft-refill-{}.toml", std::process::id())));
        let app = router(st);
        let put = |body: serde_json::Value| {
            Request::builder()
                .method("PUT")
                .uri("/api/config")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let mut cfg = body_json(app.clone().oneshot(get("/api/config")).await.unwrap()).await;
        cfg["refill"]["pump_address"] = json!(0);
        assert_eq!(app.clone().oneshot(put(cfg.clone())).await.unwrap().status(), StatusCode::BAD_REQUEST);
        cfg["refill"]["pump_address"] = json!(3);
        assert_eq!(app.clone().oneshot(put(cfg)).await.unwrap().status(), StatusCode::OK);
        let st = body_json(app.clone().oneshot(get("/api/status")).await.unwrap()).await;
        assert!(st["refill"].is_null(), "nothing drives the refill pump outside a run");

        // This engine's line has only the feed pump (address 1) on it.
        let res = app.oneshot(post_json("/api/refill/test", json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let body = body_json(res).await;
        assert!(body["error"].as_str().is_some_and(|e| e.contains("address 3")), "{body}");
    }

    #[tokio::test]
    async fn a_settings_save_that_fails_leaves_the_refill_pump_as_it_was() {
        // config.toml cannot be written: its folder is a file.
        let blocker = std::env::temp_dir().join(format!("ft-blocker-{}", std::process::id()));
        std::fs::write(&blocker, "not a folder").unwrap();
        let mut st = test_state();
        st.config_path = Arc::new(blocker.join("config.toml"));
        let app = router(st);
        let put = |body: serde_json::Value| {
            Request::builder()
                .method("PUT")
                .uri("/api/config")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let mut cfg = body_json(app.clone().oneshot(get("/api/config")).await.unwrap()).await;
        // The feed pump's address: a ping there is refused, so it shows
        // whether the engine took it.
        cfg["refill"]["pump_address"] = json!(1);
        assert_eq!(app.clone().oneshot(put(cfg)).await.unwrap().status(), StatusCode::BAD_REQUEST);
        let saved = body_json(app.clone().oneshot(get("/api/config")).await.unwrap()).await;
        assert_eq!(saved["refill"]["pump_address"], 2, "nothing saved");
        let res = app.oneshot(post_json("/api/refill/ping", json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "the engine still has the refill pump at 2");
        assert_eq!(body_json(res).await["responding"], false);
        let _ = std::fs::remove_file(&blocker);
    }

    #[tokio::test]
    async fn a_stop_for_another_run_is_refused() {
        let app = router(test_state());
        let start = post_json(
            "/api/runs",
            json!({ "name": "t", "control_var": "rpm", "direction": "cw", "pump_addr": 1, "curve": linear_curve() }),
        );
        let id = body_json(app.clone().oneshot(start).await.unwrap()).await["run_id"].as_i64().unwrap();
        let res = app.clone().oneshot(post_json(&format!("/api/runs/{}/stop", id + 41), json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let st = body_json(app.oneshot(get("/api/status")).await.unwrap()).await;
        assert_eq!(st["active"]["run_id"], id, "the active run goes on");
    }

    #[tokio::test]
    async fn missing_run_is_404() {
        let app = router(test_state());
        let res = app.oneshot(get("/api/runs/999")).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_balance_disconnects_and_connects_keeping_its_port() {
        let st = test_state();
        st.config.write().await.scale.path = "COM7".into();
        let app = router(st.clone());
        let res = app.clone().oneshot(post_json("/api/scale/disconnect", json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_json(res).await["disconnected"], "COM7");
        assert_eq!(st.config.read().await.scale.path, "COM7", "the port stays saved");
        // no balance on a test machine: Connect is accepted, the link not up
        let res = app.oneshot(post_json("/api/scale/connect", json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn connecting_with_no_balance_port_saved_is_refused() {
        let app = router(test_state());
        let res = app.oneshot(post_json("/api/scale/connect", json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn disconnect_frees_the_port_and_keeps_it_in_the_config() {
        let dir = std::env::temp_dir().join(format!("ft-disconnect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfgp = dir.join("config.toml");
        let mut st = test_state();
        st.config_path = Arc::new(cfgp.clone());
        st.config.write().await.serial.path = "COM8".into();
        let app = router(st.clone());
        let res = app.oneshot(post_json("/api/serial/disconnect", json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_json(res).await["disconnected"], "COM8");
        // the port stays configured: Connect (or a restart) opens it again
        assert_eq!(st.config.read().await.serial.path, "COM8");
        assert!(!cfgp.exists(), "nothing written to config.toml");
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
    async fn the_rate_of_a_constant_run_changes_through_the_api() {
        let app = router(test_state());
        let res = app
            .clone()
            .oneshot(post_json(
                "/api/runs",
                json!({
                    "name": "c", "control_var": "rpm", "direction": "cw", "pump_addr": 1,
                    "curve": {
                        "mode": "endpoints", "start": 5, "end": 5, "duration": 3600,
                        "clamp_min": 0, "clamp_max": 350,
                        "params": { "kind": "constant" }
                    }
                }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let id = body_json(res).await["run_id"].as_i64().unwrap();

        let res = app
            .clone()
            .oneshot(post_json("/api/runs/active/rate", json!({ "value": 3.0 })))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let b = body_json(res).await;
        assert_eq!((b["from"].as_f64(), b["to"].as_f64()), (Some(5.0), Some(3.0)), "{b}");

        let run = body_json(app.clone().oneshot(get(&format!("/api/runs/{id}"))).await.unwrap()).await;
        assert_eq!(run["setpoint_changes"].as_array().unwrap().len(), 1, "{run}");
        assert_eq!(run["effective_curve"]["params"]["kind"], json!("step"), "{run}");
        assert_eq!(run["curve"]["params"]["kind"], json!("constant"), "{run}");

        // A rate the pump cannot take is refused with the structured body.
        let res = app
            .oneshot(post_json("/api/runs/active/rate", json!({ "value": -1.0 })))
            .await
            .unwrap();
        assert!(res.status().is_client_error());
        assert!(body_json(res).await["error"].is_string());
    }

    #[tokio::test]
    async fn a_rate_change_without_a_run_is_refused() {
        let app = router(test_state());
        let res = app
            .oneshot(post_json("/api/runs/active/rate", json!({ "value": 3.0 })))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(res).await["code"], json!("idle"));
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
    async fn priming_shows_in_the_status_until_stopped() {
        let app = router(test_state());
        let res = app.clone().oneshot(post_json("/api/pump/prime", json!({ "direction": "ccw" }))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let st = body_json(app.clone().oneshot(get("/api/status")).await.unwrap()).await;
        assert_eq!(st["priming"]["direction"], "ccw");
        assert_eq!(st["priming"]["max_s"], 300);
        let res = app.clone().oneshot(post_json("/api/pump/prime/stop", json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let st = body_json(app.clone().oneshot(get("/api/status")).await.unwrap()).await;
        assert!(st["priming"].is_null());
        // nothing left to stop
        let res = app.oneshot(post_json("/api/pump/prime/stop", json!({}))).await.unwrap();
        assert!(res.status().is_client_error());
    }

    #[tokio::test]
    async fn a_finished_curve_stays_active_and_stop_completes_it() {
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
        let res = app.clone().oneshot(start).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let id = body_json(res).await["run_id"].as_i64().unwrap();

        tokio::time::sleep(Duration::from_millis(2500)).await;
        let st = body_json(app.clone().oneshot(get("/api/status")).await.unwrap()).await;
        assert_eq!(st["active"]["curve_done"], json!(true), "still active, holding: {st}");
        assert!(st["holding"].is_null(), "{st}");

        let res = app
            .clone()
            .oneshot(post_json(&format!("/api/runs/{id}/stop"), json!({})))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let run = body_json(app.oneshot(get(&format!("/api/runs/{id}"))).await.unwrap()).await;
        assert_eq!(run["status"], json!("completed"), "{run}");
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
    async fn reconnect_sets_the_feed_pump_model() {
        let dir = std::env::temp_dir().join(format!("ft-reconnect-model-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfgp = dir.join("config.toml");
        let mut st = test_state();
        st.config_path = Arc::new(cfgp.clone());
        let app = router(st.clone());

        let res = app
            .clone()
            .oneshot(post_json(
                "/api/serial/reconnect",
                json!({ "pump_model": "labv", "pump_head": 1 }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let written = std::fs::read_to_string(&cfgp).unwrap();
        assert!(written.contains("model = \"labv\""), "{written}");
        let status = body_json(app.clone().oneshot(get("/api/status")).await.unwrap()).await;
        assert_eq!(status["pump_model"], "labv", "{status}");
        assert_eq!(status["pump_max_rpm"], 150.0, "{status}");
        assert_eq!(status["pump_rpm_step"], 0.01, "{status}");
        assert_eq!(status["pump_flow_step"], 0.01, "{status}");
        assert!(status["pump_head"].is_null(), "{status}");

        // A LabV answers at 1..=32 only: refused, nothing changes.
        let res = app
            .clone()
            .oneshot(post_json("/api/serial/reconnect", json!({ "pump_model": "labv", "pump_addr": 40 })))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let msg = body_json(res).await.to_string();
        assert!(msg.contains("feed pump") && msg.contains("LabV"), "{msg}");
        assert_eq!(st.config.read().await.pump.address, 1);

        // Omitted fields keep their value; a max rpm sent by an older page
        // is ignored: the max is the model's.
        let res = app
            .clone()
            .oneshot(post_json("/api/serial/reconnect", json!({ "pump_max_rpm": 120 })))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let cfg = st.config.read().await.clone();
        assert_eq!((cfg.pump.model, cfg.pump.max_rpm()), (PumpModelCfg::Labv, 150.0));

        let res = app
            .oneshot(post_json("/api/serial/reconnect", json!({ "pump_model": "masterflex" })))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reconnect_ignores_a_notify_section_that_would_not_save() {
        let dir = std::env::temp_dir().join(format!("ft-reconnect-notify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut st = test_state();
        st.config_path = Arc::new(dir.join("config.toml"));
        st.config.write().await.notify.ntfy_server = "http://not-https".into();
        assert!(st.config.read().await.validate().is_err(), "precondition");
        let res = router(st)
            .oneshot(post_json("/api/serial/reconnect", json!({ "path": "sim" })))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let _ = std::fs::remove_dir_all(&dir);
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
    // `http://tauri.localhost` origin, cross-origin, so it needs CORS.

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
    async fn another_web_page_is_turned_away() {
        let app = router(test_state());
        let req = Request::builder()
            .uri("/api/status")
            .header("host", "127.0.0.1:8730")
            .header("origin", "http://evil.example")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        assert!(res.headers().get("access-control-allow-origin").is_none());
    }

    /// The CSRF: a page anywhere on the web posts a body-less form to the
    /// daemon. It used to be executed, CORS only hiding the answer.
    #[tokio::test]
    async fn a_cross_site_post_cannot_shut_the_daemon_down() {
        let state = test_state();
        let shutdown = Arc::clone(&state.shutdown);
        let app = router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/api/shutdown")
            .header("host", "127.0.0.1:8730")
            .header("origin", "https://some-site.example")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::empty())
            .unwrap();
        assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::FORBIDDEN);
        let notified = tokio::time::timeout(Duration::from_millis(50), shutdown.notified()).await;
        assert!(notified.is_err(), "the shutdown must not have been asked for");
    }

    /// DNS rebinding: the attacker's own name resolves to 127.0.0.1, so the
    /// page is "same origin", but the Host it sends is that name.
    #[tokio::test]
    async fn a_rebound_host_name_is_turned_away() {
        let app = router(test_state());
        let req = Request::builder()
            .uri("/api/config")
            .header("host", "attacker.example:8730")
            .body(Body::empty())
            .unwrap();
        assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn the_daemon_page_the_dev_server_and_the_window_get_in() {
        let app = router(test_state());
        for (host, origin) in [
            ("127.0.0.1:8730", Some("http://127.0.0.1:8730")),
            ("localhost:8730", Some("http://localhost:8730")),
            ("localhost:5173", Some("http://localhost:5173")),
            ("127.0.0.1:8730", Some("http://tauri.localhost")),
            ("127.0.0.1:8730", None),
        ] {
            let mut req = Request::builder().method("POST").uri("/api/scale/refill_done").header("host", host);
            if let Some(o) = origin {
                req = req.header("origin", o);
            }
            let res = app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK, "{host} {origin:?}");
        }
        let res = app.oneshot(get("/api/health")).await.unwrap();
        assert_eq!(body_json(res).await["ok"], true);
    }

    // ---- tubing calibrations (Task 13) ----

    /// An engine over a store already holding three finished 5-minute
    /// `kind = calibration` bursts at 10 ml/min; returns their ids too. The
    /// bursts are seeded directly because their durations must be real
    /// minutes, which an API-driven start/stop in a test cannot give.
    fn state_with_three_bursts() -> (AppState, [i64; 3]) {
        state_with_bursts_on(["labq"; 3])
    }

    /// [`state_with_three_bursts`], each burst run on the pump model given.
    fn state_with_bursts_on(models: [&str; 3]) -> (AppState, [i64; 3]) {
        use crate::store::{ControlVar, Direction, NewRun, RunKind};
        let db = test_db();
        let store = Store::open(&db).unwrap();
        let mut ids = [0; 3];
        for (i, id) in ids.iter_mut().enumerate() {
            let started: jiff::Timestamp = "2026-09-01T09:00:00Z".parse().unwrap();
            let started = started + jiff::SignedDuration::from_mins(10 * i as i64);
            *id = store
                .insert_run(&NewRun {
                    name: format!("cal {}", i + 1),
                    started_at: started,
                    control_var: ControlVar::MlMin,
                    direction: Direction::Cw,
                    tick_interval_s: 1,
                    pump_addr: 1,
                    app_version: "test".into(),
                    curve: CurveSpec::linear(10.0, 10.0, Duration::from_secs(600)),
                    gravimetric_trim: false,
                    kind: RunKind::Calibration,
                    tubing_calibration_id: None,
                    responsible: None,
                    density_g_per_ml: None,
                    auto_refill: false,
                    refill_draw_g: None,
                    pump_model: models[i].into(),
                    pump_head: None,
                })
                .unwrap();
            store
                .finish_run(*id, RunStatus::Stopped, started + jiff::SignedDuration::from_mins(5))
                .unwrap();
        }
        drop(store);
        (test_state_at(&db), ids)
    }

    /// The bug: with the balance off, every read on the control thread waited
    /// out its timeout, and the History and Calibration pages queued behind
    /// it. Here the control thread is held up 1.5 s by every balance read;
    /// the journal must still answer at once.
    #[tokio::test]
    async fn the_journal_answers_while_the_control_thread_waits_on_a_device() {
        struct SlowSilentScale;
        impl fermentool_modbus::Transport for SlowSilentScale {
            fn transaction(&mut self, _: &[u8]) -> Result<Vec<u8>, fermentool_modbus::TransportError> {
                std::thread::sleep(Duration::from_millis(1500));
                Err(fermentool_modbus::TransportError::Timeout)
            }
        }
        let db = test_db();
        let mut engine = Engine::new(Pump::new(SimPump::new(1), 1), Store::open(&db).unwrap(), "test");
        engine.attach_scale(
            Some(Box::new(SlowSilentScale)),
            crate::config::ScaleConfig { path: "COM-slow".into(), ..Default::default() },
        );
        let app = router(state_for(engine, &db));
        // Let the idle balance probe start holding the control thread.
        tokio::time::sleep(Duration::from_millis(1200)).await;
        for uri in [
            "/api/runs?limit=1000",
            "/api/calibrations",
            "/api/calibrations/draft",
            "/api/runs/1/events?limit=100",
        ] {
            let t = std::time::Instant::now();
            let res = app.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
            assert!(res.status().is_success() || res.status() == StatusCode::NOT_FOUND, "{uri}: {}", res.status());
            assert!(t.elapsed() < Duration::from_millis(300), "{uri} took {:?}", t.elapsed());
        }
    }

    fn delete_req(uri: &str) -> Request<Body> {
        Request::builder().method("DELETE").uri(uri).body(Body::empty()).unwrap()
    }

    fn calibration_body(run_ids: [i64; 3], weights: [f64; 3]) -> serde_json::Value {
        json!({
            "tubing_lot_id": "LOT-42",
            "product_ref": "06409-14",
            "brand": "Masterflex",
            "inner_diameter_mm": 1.6, "outer_diameter_mm": 4.8,
            "control_var": "ml_min", "setpoint": 10.0, "density_g_per_ml": 1.0,
            "run_ids": run_ids, "weights_g": weights,
        })
    }

    #[tokio::test]
    async fn calibration_round_trip() {
        let (state, runs) = state_with_three_bursts();
        let app = router(state);
        let res = app
            .clone()
            .oneshot(post_json("/api/calibrations", calibration_body(runs, [50.0; 3])))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = body_json(res).await;
        assert!((body["c0"].as_f64().unwrap() - 1.0).abs() < 1e-6);
        assert!(body["cv_pct"].as_f64().unwrap().abs() < 1e-6);
        let id = body["id"].as_i64().unwrap();

        let res = app
            .oneshot(get("/api/calibrations?lot_id=LOT-42"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let list = body_json(res).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["id"].as_i64(), Some(id));
    }

    #[tokio::test]
    async fn a_calibration_takes_the_model_its_measurements_ran_on() {
        // Measured on a LabV; by Record time the feed pump is a LabQ again.
        let (state, runs) = state_with_bursts_on(["labv"; 3]);
        state.config.write().await.pump.model = PumpModelCfg::Labq;
        let app = router(state);
        // The body claims another model: ignored.
        let mut body = calibration_body(runs, [50.0; 3]);
        body["pump_model"] = json!("pds2");
        let res = app.clone().oneshot(post_json("/api/calibrations", body)).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        assert_eq!(body_json(res).await["pump_model"], "labv");
        let list = body_json(app.oneshot(get("/api/calibrations")).await.unwrap()).await;
        assert_eq!(list[0]["pump_model"], "labv");
    }

    #[tokio::test]
    async fn measurements_on_two_models_are_no_calibration() {
        let (state, runs) = state_with_bursts_on(["labq", "labv", "labq"]);
        let app = router(state);
        let res = app.clone().oneshot(post_json("/api/calibrations", calibration_body(runs, [50.0; 3]))).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let msg = body_json(res).await.to_string();
        assert!(msg.contains("LabQ") && msg.contains("LabV"), "{msg}");
        // A measurement that does not exist: refused the same way.
        let res = app
            .clone()
            .oneshot(post_json("/api/calibrations", calibration_body([runs[0], runs[2], 999], [50.0; 3])))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let list = body_json(app.oneshot(get("/api/calibrations")).await.unwrap()).await;
        assert!(list.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn archive_and_restore_a_calibration_over_http() {
        let (state, runs) = state_with_three_bursts();
        let app = router(state);
        let res = app
            .clone()
            .oneshot(post_json("/api/calibrations", calibration_body(runs, [50.0; 3])))
            .await
            .unwrap();
        let id = body_json(res).await["id"].as_i64().unwrap();

        let res = app
            .clone()
            .oneshot(post_json(&format!("/api/calibrations/{id}/archive"), json!({})))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(body_json(res).await["archived_at"].is_string());

        let res = app
            .clone()
            .oneshot(post_json(&format!("/api/calibrations/{id}/restore"), json!({})))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(body_json(res).await["archived_at"].is_null());

        let res = app
            .oneshot(post_json(&format!("/api/calibrations/{}/archive", id + 1), json!({})))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn list_calibrations_filters_and_defaults_to_all() {
        let (state, runs) = state_with_three_bursts();
        let app = router(state);
        app.clone()
            .oneshot(post_json("/api/calibrations", calibration_body(runs, [50.0; 3])))
            .await
            .unwrap();
        let other = app.clone().oneshot(get("/api/calibrations?lot_id=OTHER")).await.unwrap();
        assert_eq!(body_json(other).await.as_array().unwrap().len(), 0);
        let all = app.oneshot(get("/api/calibrations")).await.unwrap();
        assert_eq!(body_json(all).await.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_calibration_over_bursts_that_are_not_finished_calibrations_is_a_400() {
        let (state, runs) = state_with_three_bursts();
        let app = router(state);
        let res = app
            .oneshot(post_json(
                "/api/calibrations",
                calibration_body([runs[0], runs[1], 9999], [50.0; 3]),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn draft_persists_across_a_reload() {
        let app = router(test_state());
        let res = app
            .clone()
            .oneshot(post_json(
                "/api/calibrations/draft",
                json!({"tubing_lot_id": "LOT-1", "inner_diameter_mm": 1.6}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["ok"], true);
        let res = app.oneshot(get("/api/calibrations/draft")).await.unwrap();
        let body = body_json(res).await;
        assert_eq!(body["tubing_lot_id"], "LOT-1");
    }

    #[tokio::test]
    async fn no_draft_reads_as_null_and_delete_clears_it() {
        let app = router(test_state());
        let res = app.clone().oneshot(get("/api/calibrations/draft")).await.unwrap();
        assert!(body_json(res).await.is_null());
        app.clone()
            .oneshot(post_json("/api/calibrations/draft", json!({"tubing_lot_id": "X"})))
            .await
            .unwrap();
        let res = app.clone().oneshot(delete_req("/api/calibrations/draft")).await.unwrap();
        assert_eq!(body_json(res).await["ok"], true);
        let res = app.oneshot(get("/api/calibrations/draft")).await.unwrap();
        assert!(body_json(res).await.is_null());
    }

    #[tokio::test]
    async fn recording_a_calibration_clears_the_draft() {
        let (state, runs) = state_with_three_bursts();
        let app = router(state);
        app.clone()
            .oneshot(post_json("/api/calibrations/draft", json!({"tubing_lot_id": "LOT-42"})))
            .await
            .unwrap();
        app.clone()
            .oneshot(post_json("/api/calibrations", calibration_body(runs, [50.0; 3])))
            .await
            .unwrap();
        let res = app.oneshot(get("/api/calibrations/draft")).await.unwrap();
        assert!(body_json(res).await.is_null());
    }

    #[tokio::test]
    async fn a_calibration_run_can_be_started_through_the_runs_endpoint() {
        let app = router(test_state());
        let res = app
            .clone()
            .oneshot(post_json(
                "/api/runs",
                json!({
                    "name": "cal 1", "control_var": "ml_min", "direction": "cw", "pump_addr": 1,
                    "curve": serde_json::to_value(CurveSpec::linear(10.0, 10.0, Duration::from_secs(300))).unwrap(),
                    "kind": "calibration",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let id = body_json(res).await["run_id"].as_i64().unwrap();
        let res = app.oneshot(get(&format!("/api/runs/{id}"))).await.unwrap();
        assert_eq!(body_json(res).await["kind"], "calibration");
    }

    #[tokio::test]
    async fn tracking_is_404_for_a_run_without_balance_data() {
        let app = router(test_state());
        let res = app.oneshot(get("/api/runs/1/tracking")).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    // ----- camera -----

    /// A timelapse thread on a database and a folder of its own, its camera
    /// never connected.
    fn test_timelapses() -> crate::timelapse::Timelapses {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("ft-api-tl-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = crate::store::Store::open(&dir.join("db.sqlite")).unwrap();
        let camera = crate::camera::Camera::spawn(crate::camera::fake::FakeLaunch::healthy().0, None);
        crate::timelapse::Timelapses::spawn(store, camera, move || dir.join("captures"))
    }

    fn camera_want() -> crate::camera::CameraWant {
        crate::camera::CameraWant { device: "Test pattern".into(), index: 0, width: 640, height: 480, controls: Vec::new() }
    }

    fn wait_for(max: Duration, mut f: impl FnMut() -> bool) -> bool {
        let end = std::time::Instant::now() + max;
        loop {
            if f() {
                return true;
            }
            if std::time::Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// For single-thread `#[tokio::test]`s: polls without blocking the runtime.
    async fn wait_async(max: Duration, mut f: impl FnMut() -> bool) -> bool {
        let end = std::time::Instant::now() + max;
        while !f() {
            if std::time::Instant::now() >= end {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        true
    }

    #[tokio::test]
    async fn no_helper_is_reported_even_without_a_want() {
        let mut st = test_state();
        st.helper_missing = true;
        let v = body_json(router(st).oneshot(get("/api/camera")).await.unwrap()).await;
        assert_eq!(v["state"], "no_helper");
        assert_eq!(v["configured"], false);
    }

    #[tokio::test]
    async fn shutdown_ends_an_open_live_view() {
        use http_body_util::BodyExt;
        let mut st = test_state();
        st.camera = crate::camera::Camera::spawn(crate::camera::fake::FakeLaunch::healthy().0, Some(camera_want()));
        let cam = st.camera.clone();
        let app = router(st);
        let res = app.clone().oneshot(get("/api/camera/live")).await.unwrap();
        let mut body = res.into_body();
        tokio::time::timeout(Duration::from_secs(5), body.frame()).await.unwrap().unwrap().unwrap();
        let res = app.oneshot(post_json("/api/shutdown", json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        // The body must end (None) within ~2 s, not repeat frames forever.
        let ended = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(f) = body.frame().await {
                f.unwrap();
            }
        })
        .await;
        assert!(ended.is_ok(), "the live view outlived the shutdown");
        assert!(wait_async(Duration::from_secs(1), || cam.viewers() == 0).await);
    }

    #[tokio::test]
    async fn camera_off_reports_off_and_not_configured() {
        let app = router(test_state());
        let res = app.oneshot(get("/api/camera")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["state"], "off");
        assert_eq!(v["configured"], false);
    }

    #[tokio::test]
    async fn camera_status_reports_viewers_and_the_latest_frame_time() {
        use http_body_util::BodyExt;
        let v = body_json(router(test_state()).oneshot(get("/api/camera")).await.unwrap()).await;
        assert_eq!(v["viewers"], 0);
        assert!(v["frame_at"].is_null(), "{v}");

        let mut st = test_state();
        st.camera = crate::camera::Camera::spawn(crate::camera::fake::FakeLaunch::healthy().0, Some(camera_want()));
        let app = router(st);
        let res = app.clone().oneshot(get("/api/camera/live")).await.unwrap();
        let mut body = res.into_body();
        tokio::time::timeout(Duration::from_secs(5), body.frame()).await.unwrap().unwrap().unwrap();
        let v = body_json(app.clone().oneshot(get("/api/camera")).await.unwrap()).await;
        assert_eq!(v["viewers"], 1);
        let at: jiff::Timestamp = v["frame_at"].as_str().expect("frame_at").parse().unwrap();
        assert!(jiff::Timestamp::now().duration_since(at).as_secs() < 5);
        drop(body);
    }

    #[tokio::test]
    async fn frame_404_before_any_picture() {
        let app = router(test_state());
        let res = app.oneshot(get("/api/camera/frame.jpg")).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(res).await["error"], "no picture yet");
    }

    #[tokio::test]
    async fn closing_the_stream_releases_the_viewer() {
        use http_body_util::BodyExt;
        let mut st = test_state();
        st.camera = crate::camera::Camera::spawn(crate::camera::fake::FakeLaunch::healthy().0, Some(camera_want()));
        let cam = st.camera.clone();
        let app = router(st);
        let res = app.oneshot(get("/api/camera/live")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "multipart/x-mixed-replace; boundary=frame");
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(cam.viewers(), 1);
        let mut body = res.into_body();
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame()).await.unwrap().unwrap().unwrap();
        let part = frame.into_data().unwrap();
        assert!(part.starts_with(b"--frame\r\nContent-Type: image/jpeg\r\nContent-Length: "), "{part:?}");
        assert!(part.ends_with(b"\r\n"));
        drop(body);
        // Async polling: the feeding task shares this test's single thread.
        let end = std::time::Instant::now() + Duration::from_secs(1);
        while cam.viewers() != 0 && std::time::Instant::now() < end {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(cam.viewers(), 0, "viewer not released");
        cam.shutdown();
    }

    /// A state whose timelapses share its journal, follow its run statuses
    /// and use a connected fake camera; its captures go under `root`.
    async fn timelapse_state(root: std::path::PathBuf) -> AppState {
        let db = test_db();
        let mut st = test_state_at(&db);
        st.camera = crate::camera::Camera::spawn(crate::camera::fake::FakeLaunch::healthy().0, Some(camera_want()));
        let cam = st.camera.clone();
        assert!(
            wait_async(Duration::from_secs(5), || matches!(cam.state(), crate::camera::CameraState::Connected { .. })).await
        );
        st.timelapses =
            crate::timelapse::Timelapses::spawn(crate::store::Store::open(&db).unwrap(), st.camera.clone(), move || root.clone());
        crate::timelapse::follow_status(&st.events, &st.timelapses);
        st
    }

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ft-api-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn timelapse_routes_start_stop_and_list_images() {
        let root = scratch_dir("tlroutes");
        let app = router(timelapse_state(root.join("captures")).await);
        let res = app.clone().oneshot(get("/api/timelapses/current")).await.unwrap();
        assert!(body_json(res).await["timelapse"].is_null());
        let bad = json!({ "name": "Night", "interval_min": 0 });
        assert_eq!(app.clone().oneshot(post_json("/api/timelapses", bad)).await.unwrap().status(), StatusCode::BAD_REQUEST);
        let ok = json!({ "name": "Night", "interval_min": 5, "stop_after_h": 2 });
        let res = app.clone().oneshot(post_json("/api/timelapses", ok)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let id = body_json(res).await["id"].as_i64().unwrap();
        let mut cur = json!(null);
        for _ in 0..50 {
            cur = body_json(app.clone().oneshot(get("/api/timelapses/current")).await.unwrap()).await;
            if cur["timelapse"]["images"].as_i64() == Some(1) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(cur["timelapse"]["images"], 1, "{cur}");
        assert_eq!(cur["timelapse"]["interval_s"], 300);
        assert!(cur["timelapse"]["ends_at"].is_string());
        assert!(cur["warn_free"].is_boolean());
        let res = app.clone().oneshot(get(&format!("/api/timelapses/{id}/images"))).await.unwrap();
        let list = body_json(res).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert!(list[0].get("file").is_none(), "no path leaves the daemon: {list}");
        let image = list[0]["id"].as_i64().unwrap();
        let full = app.clone().oneshot(get(&format!("/api/timelapses/images/{image}/full.jpg"))).await.unwrap();
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.headers()[header::CONTENT_TYPE], "image/jpeg");
        use http_body_util::BodyExt;
        assert_eq!(full.into_body().collect().await.unwrap().to_bytes().as_ref(), [0xFF, 0xD8, 95]);
        let thumb = app.clone().oneshot(get(&format!("/api/timelapses/images/{image}/thumb.jpg"))).await.unwrap();
        assert_eq!(thumb.into_body().collect().await.unwrap().to_bytes().as_ref(), [0xFF, 0xD8, 1]);
        let res = app.clone().oneshot(post_json("/api/timelapses/current/interval", json!({ "interval_min": 10 }))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let res = app.clone().oneshot(post_json("/api/timelapses/current/pause", json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let res = app.clone().oneshot(post_json("/api/camera/picture", json!({}))).await.unwrap();
        let pic = body_json(res).await;
        assert!(pic["image_id"].is_i64());
        // the page says where it went: the folder's name, never a path
        assert_eq!(pic["folder"], "Night", "{pic}");
        let res = app.clone().oneshot(post_json("/api/timelapses/current/stop", json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let cur = body_json(app.clone().oneshot(get("/api/timelapses/current")).await.unwrap()).await;
        assert!(cur["timelapse"].is_null());
        let res = app.clone().oneshot(post_json("/api/timelapses/current/stop", json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn image_files_are_served_by_id_only() {
        let app = router(test_state());
        for path in ["/api/timelapses/images/999/full.jpg", "/api/timelapses/images/1/other.jpg"] {
            assert_eq!(app.clone().oneshot(get(path)).await.unwrap().status(), StatusCode::NOT_FOUND, "{path}");
        }
        let res = app.clone().oneshot(post_json("/api/timelapses/images/999/show", json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "show: an unknown image opens nothing");
        let res = app.oneshot(get("/api/timelapses/images/..%2F..%2Fconfig.toml/full.jpg")).await.unwrap();
        assert!(matches!(res.status(), StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND), "{}", res.status());
    }

    fn timelapse_run(interval_min: u32) -> serde_json::Value {
        json!({ "name": "with images", "control_var": "rpm", "direction": "cw", "pump_addr": 1,
                "curve": linear_curve(), "timelapse": { "interval_min": interval_min } })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_start_with_a_timelapse_links_it_and_deleting_the_run_deletes_it() {
        let root = scratch_dir("tlrun");
        let st = timelapse_state(root.join("captures")).await;
        let tl = st.timelapses.clone();
        let app = router(st);
        let res = app.clone().oneshot(post_json("/api/runs", timelapse_run(5))).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = body_json(res).await;
        assert!(body["timelapse_error"].is_null(), "{body}");
        let id = body["run_id"].as_i64().unwrap();
        let cur = tl.current().unwrap();
        assert_eq!((cur.run_id, cur.interval_s), (Some(id), 300));
        let folder = std::path::PathBuf::from(&cur.folder);
        assert!(folder.is_dir());
        let run = body_json(app.clone().oneshot(get(&format!("/api/runs/{id}"))).await.unwrap()).await;
        assert_eq!(run["timelapse"]["interval_s"], 300, "{run}");
        app.clone().oneshot(post_json(&format!("/api/runs/{id}/stop"), json!({}))).await.unwrap();
        assert!(wait_async(Duration::from_secs(8), || tl.current().is_none()).await, "{:?}", tl.current());
        let res = app.clone().oneshot(Request::delete(format!("/api/runs/{id}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(!folder.exists(), "the run's images go with it");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_timelapse_failure_never_fails_the_run_start() {
        let root = scratch_dir("tlfail");
        // the captures folder is a file: the timelapse cannot start
        std::fs::write(root.join("captures"), b"x").unwrap();
        let app = router(timelapse_state(root.join("captures")).await);
        let res = app.clone().oneshot(post_json("/api/runs", timelapse_run(5))).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = body_json(res).await;
        assert!(body["timelapse_error"].as_str().unwrap().contains("cannot be written"), "{body}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn captures_folder_refuses_an_unwritable_path() {
        let dir = scratch_dir("tlcaptures");
        let mut st = test_state();
        st.config_path = Arc::new(dir.join("config.toml"));
        let cfg = Arc::clone(&st.config);
        let app = router(st);
        std::fs::write(dir.join("afile"), b"x").unwrap();
        let bad = json!({ "folder": dir.join("afile").to_string_lossy() });
        let res = app.clone().oneshot(post_json("/api/camera/captures", bad)).await.unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        assert!(body_json(res).await["error"].as_str().unwrap().starts_with("This folder cannot be written to"));
        let good = dir.join("imgs").to_string_lossy().into_owned();
        let res = app.clone().oneshot(post_json("/api/camera/captures", json!({ "folder": good }))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(cfg.read().await.camera.as_ref().unwrap().captures_dir, good);
        let info = body_json(app.clone().oneshot(get("/api/camera/captures")).await.unwrap()).await;
        assert_eq!(info["folder"], json!(good));
        assert_eq!(info["used_bytes"], 0);
        let res = app.clone().oneshot(post_json("/api/camera/interval", json!({ "interval_min": 2000 }))).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let res = app.clone().oneshot(post_json("/api/camera/interval", json!({ "interval_min": 10 }))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(cfg.read().await.camera.as_ref().unwrap().interval_min, 10);
    }

    #[tokio::test]
    async fn controls_are_set_then_saved_per_camera_and_reset() {
        let mut st = test_state();
        let path = std::env::temp_dir().join(format!("ft-camctl-{}.toml", std::process::id()));
        st.config_path = Arc::new(path.clone());
        let cfg = Arc::clone(&st.config);
        let cam = st.camera.clone();
        let app = router(st);
        let res = app.clone().oneshot(get("/api/camera/controls")).await.unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT, "no camera connected");
        let ok = json!({ "device": "Test pattern", "index": 0, "width": 640, "height": 480 });
        app.clone().oneshot(post_json("/api/camera/connect", ok.clone())).await.unwrap();
        assert!(
            wait_async(Duration::from_secs(5), || matches!(cam.state(), crate::camera::CameraState::Connected { .. })).await
        );
        let saved = |c: &crate::config::Config| {
            c.camera.as_ref().unwrap().controls.get("Test pattern").and_then(|m| m.get("brightness")).copied()
        };

        // dragging: on the camera, not in the file
        let drag = json!({ "control": "brightness", "value": 7, "auto": false, "save": false });
        let res = app.clone().oneshot(post_json("/api/camera/control", drag)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_json(res).await["controls"][0]["value"], 7);
        assert_eq!(saved(&*cfg.read().await), None);

        let set = json!({ "control": "brightness", "value": 12, "auto": false });
        let res = app.clone().oneshot(post_json("/api/camera/control", set)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(saved(&*cfg.read().await), Some(crate::config::SavedControl { value: 12, auto: false }));

        // refused by the camera: never saved
        let bad = json!({ "control": "brightness", "value": 999, "auto": false });
        let res = app.clone().oneshot(post_json("/api/camera/control", bad)).await.unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        assert_eq!(saved(&*cfg.read().await).unwrap().value, 12);

        // a new Connect (another size) keeps them
        let big = json!({ "device": "Test pattern", "index": 0, "width": 1280, "height": 720 });
        app.clone().oneshot(post_json("/api/camera/connect", big)).await.unwrap();
        assert_eq!(saved(&*cfg.read().await).unwrap().value, 12);

        assert!(
            wait_async(Duration::from_secs(5), || matches!(cam.state(), crate::camera::CameraState::Connected { width: 1280, .. })).await
        );
        let res = app.clone().oneshot(post_json("/api/camera/controls/reset", json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_json(res).await["controls"][0]["value"], 0);
        assert!(cfg.read().await.camera.as_ref().unwrap().controls.is_empty());

        let res = app.clone().oneshot(post_json("/api/camera/fps", json!({ "fps": 60 }))).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let res = app.clone().oneshot(post_json("/api/camera/fps", json!({ "fps": 5 }))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(cfg.read().await.camera.as_ref().unwrap().live_fps, 5);
        cam.shutdown();
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn connect_saves_the_section_and_refuses_an_odd_size() {
        let mut st = test_state();
        let path = std::env::temp_dir().join(format!("ft-cam-{}.toml", std::process::id()));
        st.config_path = Arc::new(path.clone());
        let cfg = Arc::clone(&st.config);
        let cam = st.camera.clone();
        let app = router(st);
        let bad = json!({ "device": "Test pattern", "index": 0, "width": 800, "height": 600 });
        let res = app.clone().oneshot(post_json("/api/camera/connect", bad)).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        assert!(cfg.read().await.camera.is_none());
        let ok = json!({ "device": "Test pattern", "index": 0, "width": 640, "height": 480 });
        let res = app.clone().oneshot(post_json("/api/camera/connect", ok)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(cfg.read().await.camera.as_ref().is_some_and(|c| c.enabled && c.width == 640));
        assert!(
            wait_async(Duration::from_secs(5), || matches!(cam.state(), crate::camera::CameraState::Connected { .. })).await
        );
        let res = app.clone().oneshot(post_json("/api/camera/disconnect", json!({}))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let c = cfg.read().await.camera.clone().unwrap();
        assert!(!c.enabled);
        assert_eq!(c.device, "Test pattern", "the device stays in config.toml");
        assert!(wait_async(Duration::from_secs(5), || cam.state() == crate::camera::CameraState::Off).await);
        cam.shutdown();
        let _ = std::fs::remove_file(&path);
    }
}
