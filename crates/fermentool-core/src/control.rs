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
use std::time::{Duration, Instant};

use jiff::Timestamp;
use serde::Serialize;
use tokio::sync::{broadcast, oneshot};

use fermentool_curves::CurveSpec;
use fermentool_modbus::Transport;

use crate::config::SerialConfig;
use crate::engine::{
    ActiveStatus, Engine, HoldingStatus, RecoveryInfo, RunConfig, TickOutcome, TICK_INTERVAL,
};
use crate::store::{EventRow, RunRow, RunStatus, TickRow};
use crate::transport::{SwapTransport, TransportKind};

const IDLE_POLL: Duration = Duration::from_millis(500);

/// Sub-second cadence for `Engine::apply_setpoint`. 150 ms is ~3x a MODBUS write
/// transaction at 9600 8E1 and well under the 200 ms serial timeout, so a steep
/// ramp steps through each pump grid value without loading the bus.
const WRITE_SETPOINT_INTERVAL: Duration = Duration::from_millis(150);

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
    /// Rebuild the pump transport from a serial config, without restarting.
    Reconnect {
        serial: SerialConfig,
        pump_addr: u8,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// Stop a pump still holding a completed run's final setpoint.
    StopPump(oneshot::Sender<Result<(), String>>),
    Shutdown,
}

#[derive(Debug, Clone, Serialize)]
pub struct DaemonStatus {
    pub app_version: String,
    pub active: Option<ActiveStatus>,
    pub has_pending_recovery: bool,
    /// `"sim"` or the open serial port name.
    pub transport: String,
    /// Set when a run completed and the pump is still holding its final speed.
    pub holding: Option<HoldingStatus>,
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

/// The current daemon status (cheap; called every tick and on demand).
pub fn current_status<T: Transport>(engine: &Engine<T>, grace: Duration) -> DaemonStatus {
    let st = engine.status();
    DaemonStatus {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        active: st.active,
        has_pending_recovery: engine
            .pending_recovery(Timestamp::now(), grace)
            .map(|r| r.is_some())
            .unwrap_or(false),
        transport: st.transport,
        holding: st.holding,
    }
}

/// Move `engine` onto its own thread and return the handle. `events` receives a
/// [`DaemonStatus`] after every applied tick and every state-changing command
/// (fan-out to the WebSocket).
pub fn spawn<T>(
    mut engine: Engine<T>,
    grace: Duration,
    events: broadcast::Sender<DaemonStatus>,
) -> ControlHandle
where
    T: Transport + Send + SwapTransport + 'static,
{
    let (tx, rx) = mpsc::channel::<Command>();
    let join = std::thread::Builder::new()
        .name("fermentool-control".into())
        .spawn(move || control_loop(&mut engine, &rx, grace, &events))
        .expect("spawn control thread");

    ControlHandle {
        tx: Mutex::new(tx),
        join: Mutex::new(Some(join)),
    }
}

fn control_loop<T: Transport + SwapTransport>(
    engine: &mut Engine<T>,
    rx: &mpsc::Receiver<Command>,
    grace: Duration,
    events: &broadcast::Sender<DaemonStatus>,
) {
    tracing::info!("control loop started");
    // Two absolute deadlines while a run is active, each advanced by whole steps
    // from its own previous value (never `now + interval`) so the phase stays
    // locked to the run start — command traffic between them can't shift the
    // cadence and no timing error accumulates over a long run:
    //
    //   * `next_journal` (+= TICK_INTERVAL, 1 s): `Engine::tick` — writes the
    //     pump, appends a journal row, owns run completion. Also the heartbeat
    //     that keeps writing when the curve is flat.
    //   * `next_write` (+= WRITE_SETPOINT_INTERVAL, 150 ms): `Engine::apply_setpoint`
    //     — writes the pump only when the setpoint has moved by a pump step,
    //     so a steep ramp steps through every grid value instead of jumping.
    let mut next_journal: Option<Instant> = None;
    let mut next_write: Option<Instant> = None;

    loop {
        if engine.tick_interval().is_none() {
            next_journal = None;
            next_write = None;
        } else {
            let journal_due = *next_journal.get_or_insert_with(|| Instant::now() + TICK_INTERVAL);
            let write_due =
                *next_write.get_or_insert_with(|| Instant::now() + WRITE_SETPOINT_INTERVAL);

            if Instant::now() >= journal_due {
                let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    match engine.tick(Timestamp::now()) {
                        Ok(TickOutcome::Finished { seq, target }) => {
                            tracing::info!(seq, target, "run finished");
                            true
                        }
                        Ok(TickOutcome::Applied { .. }) => true,
                        Ok(TickOutcome::Idle) => false,
                        Err(e) => {
                            tracing::warn!("tick error: {e}");
                            false
                        }
                    }
                }));
                match done {
                    Ok(true) => {
                        let _ = events.send(current_status(engine, grace));
                    }
                    Ok(false) => {}
                    Err(_) => tracing::error!("panic in tick; loop continues"),
                }
                next_journal = Some(advance_past(journal_due, TICK_INTERVAL));
                // The journal tick just wrote the pump — hold the next sub-second
                // write a full interval off so we don't write twice in a row.
                next_write = Some(Instant::now() + WRITE_SETPOINT_INTERVAL);
                continue;
            }

            if Instant::now() >= write_due {
                let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    engine.apply_setpoint(Timestamp::now())
                }));
                match done {
                    Ok(Ok(true)) => {
                        let _ = events.send(current_status(engine, grace));
                    }
                    Ok(Ok(false)) => {}
                    Ok(Err(e)) => tracing::warn!("apply_setpoint error: {e}"),
                    Err(_) => tracing::error!("panic in apply_setpoint; loop continues"),
                }
                next_write = Some(advance_past(write_due, WRITE_SETPOINT_INTERVAL));
                continue;
            }
        }

        let wait = match (next_journal, next_write) {
            (Some(j), Some(w)) => j.min(w).saturating_duration_since(Instant::now()),
            _ => IDLE_POLL,
        };
        match rx.recv_timeout(wait) {
            Ok(Command::Shutdown) => break,
            Ok(cmd) => {
                let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    handle(engine, cmd, grace)
                }));
                match done {
                    Ok(true) => {
                        let _ = events.send(current_status(engine, grace));
                    }
                    Ok(false) => {}
                    Err(_) => tracing::error!("panic while handling a command; loop continues"),
                }
            }
            // Woke on a deadline (or early): the due checks at the top of the
            // loop do the work.
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    tracing::info!("control loop stopped");
}

/// Advance `deadline` by whole `step`s until it is in the future — keeps the
/// cadence phase-locked while a slow tick / long stall doesn't trigger a burst
/// of makeup work.
fn advance_past(mut deadline: Instant, step: Duration) -> Instant {
    let now = Instant::now();
    loop {
        deadline += step;
        if deadline > now {
            return deadline;
        }
    }
}

/// Returns `true` when the run state may have changed and a status broadcast is warranted.
fn handle<T: Transport + SwapTransport>(engine: &mut Engine<T>, cmd: Command, grace: Duration) -> bool {
    let now = Timestamp::now();
    match cmd {
        Command::Shutdown => false,
        Command::Status(reply) => {
            let _ = reply.send(current_status(engine, grace));
            false
        }
        Command::StartRun(cfg, reply) => {
            let _ = reply.send(engine.start_run(cfg, now).map_err(|e| e.to_string()));
            true
        }
        Command::StopRun(reply) => {
            let _ = reply.send(engine.stop_run(now).map_err(|e| e.to_string()));
            true
        }
        Command::AbortRun(reply) => {
            let _ = reply.send(engine.abort_run(now).map_err(|e| e.to_string()));
            true
        }
        Command::Recovery(reply) => {
            let _ = reply.send(
                engine
                    .pending_recovery(now, grace)
                    .map_err(|e| e.to_string()),
            );
            false
        }
        Command::Resume(reply) => {
            let _ = reply.send(engine.resume(now, grace).map_err(|e| e.to_string()));
            true
        }
        Command::DiscardRecovery(status, reply) => {
            let _ = reply.send(
                engine
                    .discard_recovery(now, status)
                    .map_err(|e| e.to_string()),
            );
            true
        }
        Command::GetRun(id, reply) => {
            let _ = reply.send(engine.store().run(id).map_err(|e| e.to_string()));
            false
        }
        Command::ListRuns(limit, reply) => {
            let _ = reply.send(engine.store().list_runs(limit).map_err(|e| e.to_string()));
            false
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
            false
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
            false
        }
        Command::Preview(spec, samples, reply) => {
            let series = spec
                .preview(samples)
                .into_iter()
                .map(|(t, v)| [t, v])
                .collect();
            let _ = reply.send(series);
            false
        }
        Command::Reconnect {
            serial,
            pump_addr,
            reply,
        } => {
            let wanted_serial = !serial.use_simulator();
            let msg = engine
                .swap_transport(&serial, pump_addr)
                .map(|kind| match kind {
                    TransportKind::Serial(name) => format!("serial port open: {name}"),
                    TransportKind::Sim if wanted_serial => format!(
                        "could not open {} — running on the pump simulator",
                        serial.path
                    ),
                    TransportKind::Sim => "running on the pump simulator".to_string(),
                })
                .map_err(|e| e.to_string());
            let _ = reply.send(msg);
            true
        }
        Command::StopPump(reply) => {
            let _ = reply.send(engine.stop_pump(now).map_err(|e| e.to_string()));
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use fermentool_curves::CurveSpec;
    use fermentool_modbus::{Pump, SimPump};

    use crate::engine::{Engine, RunConfig};
    use crate::store::{ControlVar, Direction, Store};

    fn cadence_run() -> RunConfig {
        RunConfig {
            name: "cadence".into(),
            control_var: ControlVar::Rpm,
            direction: Direction::Cw,
            pump_addr: 1,
            curve: CurveSpec::linear(0.0, 100.0, Duration::from_secs(3600)),
        }
    }

    /// A burst of API commands between ticks must not shift the tick cadence.
    /// The UI refetches `/ticks` + `/events` after every WS push, i.e. two
    /// commands land ~immediately after each tick; with the deadline reset on
    /// every `recv_timeout` this steady traffic starves the tick loop. Against
    /// an absolute deadline the loop still ticks ~once per second.
    #[tokio::test]
    async fn tick_cadence_is_independent_of_command_traffic() {
        let engine = Engine::new(
            Pump::new(SimPump::new(1), 1),
            Store::open_in_memory().unwrap(),
            "test",
        );
        let (events, _keep_rx) = broadcast::channel(64);
        let handle = spawn(engine, Duration::from_secs(300), events);

        handle
            .call(|reply| Command::StartRun(cadence_run(), reply))
            .await
            .unwrap()
            .unwrap();

        let until = Instant::now() + Duration::from_millis(3000);
        while Instant::now() < until {
            let _ = handle.call(Command::Status).await;
            tokio::time::sleep(Duration::from_millis(40)).await;
        }

        let st = handle.call(Command::Status).await.unwrap();
        let last_seq = st.active.and_then(|a| a.last_seq);
        // ~3 ticks in 3 s. `last_seq` is `next_seq - 1`, so >= Some(1) means at
        // least two ticks fired despite ~75 Status calls; the upper bound guards
        // against a makeup-tick burst.
        assert!(
            matches!(last_seq, Some(s) if (1..=8).contains(&s)),
            "cadence not held under command flood: last_seq = {last_seq:?}"
        );

        handle.shutdown();
    }

    /// Between two 1 s journal ticks the sub-second write loop must still move
    /// the setpoint, so a steep ramp doesn't jump a whole tick's worth at once.
    #[tokio::test]
    async fn setpoint_advances_between_journal_ticks() {
        let engine = Engine::new(
            Pump::new(SimPump::new(1), 1),
            Store::open_in_memory().unwrap(),
            "test",
        );
        let (events, _keep_rx) = broadcast::channel(64);
        let handle = spawn(engine, Duration::from_secs(300), events);

        let cfg = RunConfig {
            name: "ramp".into(),
            control_var: ControlVar::Rpm,
            direction: Direction::Cw,
            pump_addr: 1,
            // 0 → 350 rpm in 60 s ⇒ ~5.8 rpm/s: a 0.1 grid step every ~17 ms.
            curve: CurveSpec::linear(0.0, 350.0, Duration::from_secs(60)),
        };
        handle
            .call(|reply| Command::StartRun(cfg, reply))
            .await
            .unwrap()
            .unwrap();

        let a = handle
            .call(Command::Status)
            .await
            .unwrap()
            .active
            .and_then(|s| s.last_target);
        tokio::time::sleep(Duration::from_millis(600)).await;
        let b = handle
            .call(Command::Status)
            .await
            .unwrap()
            .active
            .and_then(|s| s.last_target);

        // Well before the first journal tick (1 s), the setpoint has already
        // climbed via the 150 ms write loop.
        assert!(
            matches!((a, b), (Some(x), Some(y)) if y > x + 0.05),
            "setpoint did not advance between journal ticks: {a:?} -> {b:?}"
        );

        handle.shutdown();
    }

    #[tokio::test]
    async fn reconnect_when_idle_reports_the_transport() {
        let engine = Engine::new(
            Pump::new(SimPump::new(1), 1),
            Store::open_in_memory().unwrap(),
            "test",
        );
        let (events, _keep_rx) = broadcast::channel(16);
        let handle = spawn(engine, Duration::from_secs(300), events);

        let serial = crate::config::SerialConfig {
            path: "sim".into(),
            baud: 9600,
        };
        let msg = handle
            .call(|reply| Command::Reconnect {
                serial,
                pump_addr: 1,
                reply,
            })
            .await
            .unwrap()
            .unwrap();
        assert!(msg.to_lowercase().contains("simulator"), "got: {msg}");

        handle.shutdown();
    }

    #[tokio::test]
    async fn stop_pump_releases_a_completed_run_hold() {
        let engine = Engine::new(
            Pump::new(SimPump::new(1), 1),
            Store::open_in_memory().unwrap(),
            "test",
        );
        let (events, _keep_rx) = broadcast::channel(16);
        let handle = spawn(engine, Duration::from_secs(300), events);

        let cfg = RunConfig {
            name: "short".into(),
            control_var: ControlVar::Rpm,
            direction: Direction::Cw,
            pump_addr: 1,
            curve: CurveSpec::linear(10.0, 20.0, Duration::from_secs(1)),
        };
        handle
            .call(|reply| Command::StartRun(cfg, reply))
            .await
            .unwrap()
            .unwrap();

        // let the 1 s journal tick fire and complete the run
        tokio::time::sleep(Duration::from_millis(2500)).await;
        let st = handle.call(Command::Status).await.unwrap();
        assert!(st.active.is_none(), "run should have completed");
        assert!(st.holding.is_some(), "pump should be holding the final setpoint");

        handle.call(Command::StopPump).await.unwrap().unwrap();
        let st = handle.call(Command::Status).await.unwrap();
        assert!(st.holding.is_none());

        handle.shutdown();
    }

    #[tokio::test]
    async fn stop_pump_with_nothing_held_errors() {
        let engine = Engine::new(
            Pump::new(SimPump::new(1), 1),
            Store::open_in_memory().unwrap(),
            "test",
        );
        let (events, _keep_rx) = broadcast::channel(16);
        let handle = spawn(engine, Duration::from_secs(300), events);

        let err = handle.call(Command::StopPump).await.unwrap().unwrap_err();
        assert_eq!(err, "no run is active");

        handle.shutdown();
    }

    #[tokio::test]
    async fn reconnect_is_refused_during_a_run() {
        let engine = Engine::new(
            Pump::new(SimPump::new(1), 1),
            Store::open_in_memory().unwrap(),
            "test",
        );
        let (events, _keep_rx) = broadcast::channel(16);
        let handle = spawn(engine, Duration::from_secs(300), events);

        handle
            .call(|reply| Command::StartRun(cadence_run(), reply))
            .await
            .unwrap()
            .unwrap();

        let serial = crate::config::SerialConfig {
            path: "sim".into(),
            baud: 9600,
        };
        let err = handle
            .call(|reply| Command::Reconnect {
                serial,
                pump_addr: 1,
                reply,
            })
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err, "a run is already active");

        handle.shutdown();
    }
}
