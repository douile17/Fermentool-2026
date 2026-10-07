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
        .route("/api/status", get(status))
        .route("/api/config", get(get_config).put(put_config))
        .route("/api/preview", post(preview))
        .route("/api/runs", get(list_runs).post(create_run))
        .route("/api/history", delete(clear_history))
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
        .route("/api/pump/stop", post(pump_stop))
        .route("/api/scale/refill_mode", post(scale_refill_mode))
        .route("/api/scale/refill_done", post(scale_refill_done))
        .route("/api/calibrations", get(list_calibrations).post(create_calibration))
        .route(
            "/api/calibrations/draft",
            get(get_calibration_draft)
                .post(save_calibration_draft)
                .delete(delete_calibration_draft),
        )
        .route("/api/calibrations/{id}/archive", post(archive_calibration))
        .route("/api/calibrations/{id}/restore", post(restore_calibration))
        .route("/api/notify/test", post(notify_test))
        .route("/api/notify/pages", get(notify_pages))
        .route("/api/notify/ack", post(notify_ack))
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
    if !(new.scale.density_g_per_ml.is_finite() && new.scale.density_g_per_ml > 0.0) {
        return Err(ApiError::Bad("liquid density must be a positive number (g/mL)".into()));
    }
    new.notify.validate().map_err(ApiError::Bad)?;
    let lim = new.scale.trim_limit_pct;
    if !(lim.is_finite()
        && (crate::config::TRIM_LIMIT_PCT_MIN..=crate::config::TRIM_LIMIT_PCT_MAX).contains(&lim))
    {
        return Err(ApiError::Bad(format!(
            "correction limit must be between {} and {} %",
            crate::config::TRIM_LIMIT_PCT_MIN,
            crate::config::TRIM_LIMIT_PCT_MAX
        )));
    }
    // A changed `[scale]` is applied live (Settings, Balance section), before
    // saving: a refusal (trimmed run active) must not leave the file ahead of
    // the engine.
    let old_scale = s.config.read().await.scale.clone();
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
    *s.config.write().await = new.clone();
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
        "note": "pump port, baud and address changes apply on the next Connect; balance changes apply now; API port and log level on restart"
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
    let limit = q.limit.clamp(1, 1000);
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

async fn create_run(State(s): State<AppState>, Json(mut cfg): Json<RunConfig>) -> ApiResult<Response> {
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
    let id = s
        .control
        .call(|reply| Command::StartRun(cfg, reply))
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Detail)?;
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
        .db
        .call(move |db| db.ticks(id, from, to))
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
    Json(cal): Json<NewCalibration>,
) -> ApiResult<Response> {
    let row = s
        .db
        .call(move |db| {
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
            "UI not built, run `npm run build` in ui/",
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

    // Swap first, a refused reconnect (e.g. a run is active) must not touch
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
            1,
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
    async fn cors_does_not_echo_an_unknown_origin() {
        let app = router(test_state());
        let req = Request::builder()
            .uri("/api/status")
            .header("origin", "http://evil.example")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        // Still served, CORS is browser-enforced, but with no allow-origin echo.
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers().get("access-control-allow-origin").is_none());
    }

    // ---- tubing calibrations (Task 13) ----

    /// An engine over a store already holding three finished 5-minute
    /// `kind = calibration` bursts at 10 ml/min; returns their ids too. The
    /// bursts are seeded directly because their durations must be real
    /// minutes, which an API-driven start/stop in a test cannot give.
    fn state_with_three_bursts() -> (AppState, [i64; 3]) {
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
}
