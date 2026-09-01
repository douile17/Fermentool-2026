//! The control thread: the single owner of the [`Engine`].
//!
//! `Engine` is `!Sync` (it holds a SQLite connection and the serial port), so it
//! lives on one dedicated OS thread. The async API talks to it over a command
//! channel and gets replies through per-call `oneshot`s. The loop blocks on
//! `recv_timeout(next_tick)` so a command is served immediately and ticks still
//! fire on cadence. Each command / tick runs inside `catch_unwind` — a panic is
//! logged and the loop continues; it never takes the process down.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

use jiff::Timestamp;
use serde::Serialize;
use tokio::sync::oneshot;

use fermentool_curves::CurveSpec;
use fermentool_modbus::Transport;

use crate::engine::{ActiveStatus, Engine, RecoveryInfo, RunConfig, TickOutcome};
use crate::store::{EventRow, RunRow, RunStatus, TickRow};

const IDLE_POLL: Duration = Duration::from_millis(500);

/// One request to the control thread. Each carries a `oneshot` reply channel.
pub enum Command {
    Status(oneshot::Sender<DaemonStatus>),
    StartRun(RunConfig, oneshot::Sender<Result<i64, String>>),
    StopRun(oneshot::Sender<Result<(), String>>),
    AbortRun(oneshot::Sender<Result<(), String>>),
    Recovery(oneshot::Sender<Result<Option<RecoveryInfo>, String>>),
    Resume(oneshot::Sender<Result<i64, String>>),
    DiscardRecovery(RunStatus, oneshot::Sender<Result<(), String>>),
    GetRun(i64, oneshot::Sender<Result<Option<RunRow>, String>>),
    ListRuns(i64, oneshot::Sender<Result<Vec<RunRow>, String>>),
    GetTicks {
        run_id: i64,
        from: i64,
        to: i64,
        reply: oneshot::Sender<Result<Vec<TickRow>, String>>,
    },
    GetEvents {
        run_id: i64,
        limit: i64,
        reply: oneshot::Sender<Result<Vec<EventRow>, String>>,
    },
    Preview(CurveSpec, usize, oneshot::Sender<Vec<[f64; 2]>>),
    Shutdown,
}

#[derive(Debug, Clone, Serialize)]
pub struct DaemonStatus {
    pub app_version: String,
    pub active: Option<ActiveStatus>,
    pub has_pending_recovery: bool,
}

/// Sending / receiving on the control channel failed — the thread is gone.
#[derive(Debug)]
pub struct ControlDown;

impl std::fmt::Display for ControlDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "control thread is not running")
    }
}
impl std::error::Error for ControlDown {}

/// Handle held by the async side. `Send + Sync`.
pub struct ControlHandle {
    tx: Mutex<mpsc::Sender<Command>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl ControlHandle {
    /// Send a command built by `make` and await its reply.
    pub async fn call<R>(
        &self,
        make: impl FnOnce(oneshot::Sender<R>) -> Command,
    ) -> Result<R, ControlDown> {
        let (rtx, rrx) = oneshot::channel();
        {
            let tx = self.tx.lock().expect("control tx poisoned");
            tx.send(make(rtx)).map_err(|_| ControlDown)?;
        }
        rrx.await.map_err(|_| ControlDown)
    }

    /// Ask the loop to stop, then join the thread.
    pub fn shutdown(&self) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(Command::Shutdown);
        }
        if let Some(handle) = self.join.lock().expect("control join poisoned").take() {
            let _ = handle.join();
        }
    }
}

/// Move `engine` onto its own thread and return the handle.
pub fn spawn<T>(mut engine: Engine<T>, grace: Duration) -> ControlHandle
where
    T: Transport + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Command>();
    let join = std::thread::Builder::new()
        .name("fermentool-control".into())
        .spawn(move || control_loop(&mut engine, &rx, grace))
        .expect("spawn control thread");

    ControlHandle {
        tx: Mutex::new(tx),
        join: Mutex::new(Some(join)),
    }
}

fn control_loop<T: Transport>(
    engine: &mut Engine<T>,
    rx: &mpsc::Receiver<Command>,
    grace: Duration,
) {
    tracing::info!("control loop started");
    loop {
        let wait = engine.tick_interval().unwrap_or(IDLE_POLL);
        match rx.recv_timeout(wait) {
            Ok(Command::Shutdown) => break,
            Ok(cmd) => {
                let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    handle(engine, cmd, grace)
                }));
                if done.is_err() {
                    tracing::error!("panic while handling a command; loop continues");
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match engine
                    .tick(Timestamp::now())
                {
                    Ok(TickOutcome::Finished { seq, target }) => {
                        tracing::info!(seq, target, "run finished");
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("tick error: {e}"),
                }));
                if done.is_err() {
                    tracing::error!("panic in tick; loop continues");
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    tracing::info!("control loop stopped");
}

fn handle<T: Transport>(engine: &mut Engine<T>, cmd: Command, grace: Duration) {
    let now = Timestamp::now();
    match cmd {
        Command::Shutdown => {}
        Command::Status(reply) => {
            let has_pending_recovery = engine
                .pending_recovery(now, grace)
                .map(|r| r.is_some())
                .unwrap_or(false);
            let _ = reply.send(DaemonStatus {
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                active: engine.status().active,
                has_pending_recovery,
            });
        }
        Command::StartRun(cfg, reply) => {
            let _ = reply.send(engine.start_run(cfg, now).map_err(|e| e.to_string()));
        }
        Command::StopRun(reply) => {
            let _ = reply.send(engine.stop_run(now).map_err(|e| e.to_string()));
        }
        Command::AbortRun(reply) => {
            let _ = reply.send(engine.abort_run(now).map_err(|e| e.to_string()));
        }
        Command::Recovery(reply) => {
            let _ = reply.send(
                engine
                    .pending_recovery(now, grace)
                    .map_err(|e| e.to_string()),
            );
        }
        Command::Resume(reply) => {
            let _ = reply.send(engine.resume(now, grace).map_err(|e| e.to_string()));
        }
        Command::DiscardRecovery(status, reply) => {
            let _ = reply.send(
                engine
                    .discard_recovery(now, status)
                    .map_err(|e| e.to_string()),
            );
        }
        Command::GetRun(id, reply) => {
            let _ = reply.send(engine.store().run(id).map_err(|e| e.to_string()));
        }
        Command::ListRuns(limit, reply) => {
            let _ = reply.send(engine.store().list_runs(limit).map_err(|e| e.to_string()));
        }
        Command::GetTicks {
            run_id,
            from,
            to,
            reply,
        } => {
            let _ = reply.send(
                engine
                    .store()
                    .ticks(run_id, from, to)
                    .map_err(|e| e.to_string()),
            );
        }
        Command::GetEvents {
            run_id,
            limit,
            reply,
        } => {
            let _ = reply.send(
                engine
                    .store()
                    .events(Some(run_id), limit)
                    .map_err(|e| e.to_string()),
            );
        }
        Command::Preview(spec, samples, reply) => {
            let series = spec
                .preview(samples)
                .into_iter()
                .map(|(t, v)| [t, v])
                .collect();
            let _ = reply.send(series);
        }
    }
}
