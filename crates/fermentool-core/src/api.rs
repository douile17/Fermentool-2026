//! The daemon's HTTP API (axum), bound to `127.0.0.1` only.
//!
//! Every route that touches the run forwards a [`Command`] to the control thread
//! and awaits its reply; nothing here touches the pump or the database directly.
//! Milestone 7 is REST + polling; the WebSocket push and static-UI serving land
//! with the UI (milestone 8).

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{Notify, RwLock};

use fermentool_curves::CurveSpec;
use fermentool_modbus::serial::available_ports;

use crate::config::Config;
use crate::control::{Command, ControlHandle};
use crate::engine::RunConfig;
use crate::store::RunStatus;

#[derive(Clone)]
pub struct AppState {
    pub control: Arc<ControlHandle>,
    pub config: Arc<RwLock<Config>>,
    pub config_path: Arc<PathBuf>,
    /// Notified by `POST /api/shutdown` to stop the server gracefully.
    pub shutdown: Arc<Notify>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/config", get(get_config).put(put_config))
        .route("/api/preview", post(preview))
        .route("/api/runs", get(list_runs).post(create_run))
        .route("/api/runs/{id}", get(get_run))
        .route("/api/runs/{id}/ticks", get(get_ticks))
        .route("/api/runs/{id}/events", get(get_events))
        .route("/api/runs/{id}/stop", post(stop_run))
        .route("/api/runs/{id}/abort", post(abort_run))
        .route("/api/recovery", get(get_recovery))
        .route("/api/recovery/resume", post(resume))
        .route("/api/recovery/discard", post(discard))
        .route("/api/serial/ports", get(serial_ports))
        .route("/api/shutdown", post(shutdown))
        .with_state(state)
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
    Ok(Json(json!({
        "saved": true,
        "note": "port and serial changes take effect on restart"
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

async fn get_ticks(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<TickQuery>,
) -> ApiResult<Response> {
    let ticks = s
        .control
        .call(|reply| Command::GetTicks {
            run_id: id,
            from: q.from,
            to: q.to,
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
        AppState {
            control: Arc::new(crate::control::spawn(engine, Duration::from_secs(300))),
            config: Arc::new(RwLock::new(Config::default())),
            config_path: Arc::new(std::env::temp_dir().join("ft-api-test.toml")),
            shutdown: Arc::new(Notify::new()),
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
                "tick_interval_s": 1, "pump_addr": 1, "pump_head": null, "tubing": null,
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
                "tick_interval_s": 1, "pump_addr": 1, "pump_head": null, "tubing": null,
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
}
