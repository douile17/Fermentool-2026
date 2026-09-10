//! The control thread: the single owner of the [`Engine`].
//!
//! `Engine` is `!Sync` (it holds a SQLite connection and the serial port), so it
//! lives on one dedicated OS thread. The async API talks to it over a command
//! channel and gets replies through per-call `oneshot`s. The loop blocks on
//! `recv_timeout(next_tick)` so a command is served immediately and ticks still
//! fire on cadence. Each command / tick runs inside `catch_unwind`, a panic is
//! logged and the loop continues; it never takes the process down.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use jiff::{SignedDuration, Timestamp};
use serde::Serialize;
use tokio::sync::{broadcast, oneshot};

use fermentool_curves::CurveSpec;
use fermentool_modbus::Transport;

use crate::config::SerialConfig;
use crate::engine::{
    ActiveStatus, Engine, HoldingStatus, RecoveryInfo, RunConfig, TickOutcome,
    REOPEN_AFTER_WRITE_FAILS, TICK_INTERVAL,
};
use crate::store::{EventRow, RunRow, RunStatus, TickRow};
use crate::transport::{SwapTransport, TransportKind};

const IDLE_POLL: Duration = Duration::from_millis(500);

/// Sub-second cadence for `Engine::apply_setpoint`. 150 ms is ~3x a MODBUS write
/// transaction at 9600 8E1 and well under the 200 ms serial timeout, so a steep
/// ramp steps through each pump grid value without loading the bus.
const WRITE_SETPOINT_INTERVAL: Duration = Duration::from_millis(150);

/// While the serial link is lost, retry reopening the port on an exponential
/// backoff: `SERIAL_RETRY_MIN`, then doubling, capped at `SERIAL_RETRY_MAX`. The
/// first retry is quick so a cable plugged straight back in recovers in ~1 s;
/// the backoff then keeps a mid-run recovery attempt (a port close/reopen plus
/// one or two blocking MODBUS confirm reads, ~0.5-1 s) from churning a marginal
/// port every second and starving the 150 ms setpoint writes. The log stays
/// quiet regardless, `maybe_recover_serial` reports only the down/up edges.
const SERIAL_RETRY_MIN: Duration = Duration::from_secs(1);
const SERIAL_RETRY_MAX: Duration = Duration::from_secs(15);

/// When no run is active, poll the pump this often so a cable pulled between
/// runs (or during a completed-run hold) is still noticed and auto-recovered.
/// Matches the 1 s journal cadence, so an idle dead link trips the alarm on the
/// same ~5-fail streak as a mid-run one.
const LINK_PROBE_INTERVAL: Duration = Duration::from_secs(1);

/// If the wall clock and the monotonic clock disagree by more than this since
/// the run started, the system clock has stepped (NTP correction, VM
/// resume, …): the run's elapsed time is pinned to the monotonic clock for that
/// tick so the pump doesn't lurch to the wrong point on the curve.
const CLOCK_STEP_LIMIT: Duration = Duration::from_secs(5);

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
    /// Delete every run + journal. Refused while a run is active or a crash
    /// recovery is pending.
    ClearHistory(oneshot::Sender<Result<(), String>>),
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
    /// Push a changed `serial.allow_simulator` from a config save to the live
    /// engine, so enabling simulator runs in Settings takes effect without a
    /// reconnect or restart.
    SetAllowSimulator(bool, oneshot::Sender<()>),
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
    /// `false` while the serial link to the pump is lost (the daemon is
    /// retrying); the UI should raise a loud alarm.
    pub serial_ok: bool,
    /// Consecutive failed pump writes since the last success.
    pub write_fails: u32,
    /// `false` once the pump stops matching the commanded setpoint on readback.
    pub pump_confirmed: bool,
    /// `false` once tick journalling has been failing (disk full / unwritable).
    /// The pump keeps running; this warns that the run's history is being lost.
    pub journal_ok: bool,
    /// The engine is driving the in-process simulator, not a real serial port.
    pub simulator: bool,
    /// `serial.allow_simulator`, simulator runs are explicitly enabled.
    pub allow_simulator: bool,
}

/// Sending / receiving on the control channel failed, the thread is gone.
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
        serial_ok: st.serial_ok,
        write_fails: st.write_fails,
        pump_confirmed: st.pump_confirmed,
        journal_ok: st.journal_ok,
        simulator: st.simulator,
        allow_simulator: st.allow_simulator,
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
    // locked to the run start, command traffic between them can't shift the
    // cadence and no timing error accumulates over a long run:
    //
    //   * `next_journal` (+= TICK_INTERVAL, 1 s): `Engine::tick`, writes the
    //     pump, appends a journal row, owns run completion. Also the heartbeat
    //     that keeps writing when the curve is flat.
    //   * `next_write` (+= WRITE_SETPOINT_INTERVAL, 150 ms): `Engine::apply_setpoint`
    //, writes the pump only when the setpoint has moved by a pump step,
    //     so a steep ramp steps through every grid value instead of jumping.
    let mut next_journal: Option<Instant> = None;
    let mut next_write: Option<Instant> = None;
    // (run's wall `started_at`, monotonic anchor, run's elapsed time when that
    // anchor was taken, run id), lets a tick detect a system-clock step and use
    // monotonic time instead. The elapsed-at-anchor term is ~0 for a fresh run
    // but carries the pre-restart elapsed on a resume, so the expected
    // wall-vs-monotonic gap after a resume isn't mistaken for a clock step.
    let mut run_epoch: Option<(Timestamp, Instant, SignedDuration, i64)> = None;
    // Cooldown between automatic serial-reopen attempts while the link is down.
    // `(when to try the next reopen, the backoff delay that scheduled it)`.
    let mut next_serial_retry: Option<(Instant, Duration)> = None;
    // Idle-time link probe deadline.
    let mut next_probe: Option<Instant> = None;

    loop {
        // Keep the monotonic run epoch in sync with what the engine is running.
        match engine.status().active {
            Some(a) if run_epoch.map(|(_, _, _, id)| id) != Some(a.run_id) => {
                let anchor = Instant::now();
                let elapsed_at_anchor = Timestamp::now()
                    .duration_since(a.started_at)
                    .max(SignedDuration::ZERO);
                run_epoch = Some((a.started_at, anchor, elapsed_at_anchor, a.run_id));
            }
            Some(_) => {}
            None => run_epoch = None,
        }

        if engine.tick_interval().is_none() {
            next_journal = None;
            next_write = None;
            // No run: keep the serial link's health current so a cable pulled
            // between runs still trips the alarm and the auto-reopen.
            if engine.serial_is_real() {
                let probe_due =
                    *next_probe.get_or_insert_with(|| Instant::now() + LINK_PROBE_INTERVAL);
                if Instant::now() >= probe_due {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        engine.probe_link()
                    }));
                    maybe_recover_serial(engine, events, grace, &mut next_serial_retry);
                    next_probe = Some(advance_past(probe_due, LINK_PROBE_INTERVAL));
                    continue;
                }
            } else {
                next_probe = None;
            }
        } else {
            next_probe = None;
            let journal_due = *next_journal.get_or_insert_with(|| Instant::now() + TICK_INTERVAL);
            let write_due =
                *next_write.get_or_insert_with(|| Instant::now() + WRITE_SETPOINT_INTERVAL);

            if Instant::now() >= journal_due {
                let now = run_now(run_epoch);
                let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    match engine.tick(now) {
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
                maybe_recover_serial(engine, events, grace, &mut next_serial_retry);
                next_journal = Some(advance_past(journal_due, TICK_INTERVAL));
                // The journal tick just wrote the pump, hold the next sub-second
                // write a full interval off so we don't write twice in a row.
                next_write = Some(Instant::now() + WRITE_SETPOINT_INTERVAL);
                continue;
            }

            if Instant::now() >= write_due {
                let now = run_now(run_epoch);
                let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    engine.apply_setpoint(now)
                }));
                match done {
                    Ok(Ok(true)) => {
                        let _ = events.send(current_status(engine, grace));
                    }
                    Ok(Ok(false)) => {}
                    Ok(Err(e)) => tracing::warn!("apply_setpoint error: {e}"),
                    Err(_) => tracing::error!("panic in apply_setpoint; loop continues"),
                }
                maybe_recover_serial(engine, events, grace, &mut next_serial_retry);
                next_write = Some(advance_past(write_due, WRITE_SETPOINT_INTERVAL));
                continue;
            }
        }

        let wait = match (next_journal, next_write) {
            (Some(j), Some(w)) => j.min(w).saturating_duration_since(Instant::now()),
            // Idle: wake for the link probe if one is scheduled, else just poll.
            _ => next_probe
                .map(|p| p.saturating_duration_since(Instant::now()).min(IDLE_POLL))
                .unwrap_or(IDLE_POLL),
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
            // loop do the work. Also keep chipping at a lost serial link even
            // when no run is active.
            Err(RecvTimeoutError::Timeout) => {
                maybe_recover_serial(engine, events, grace, &mut next_serial_retry);
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    tracing::info!("control loop stopped");
}

/// Advance `deadline` by whole `step`s until it is in the future, keeps the
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

/// The timestamp to hand a tick. Normally wall-clock `now`, but if the wall
/// clock has drifted from the monotonic clock by more than [`CLOCK_STEP_LIMIT`]
/// since the run started, it stepped, pin the run's elapsed time to the
/// monotonic anchor for this tick so the pump doesn't jump on the curve.
///
/// `mono_secs` is the run's elapsed time estimated along the monotonic path:
/// what it was when the anchor was taken (`elapsed_at_anchor`, ~0 for a fresh
/// run, the pre-restart elapsed for a resume) plus real monotonic time since.
/// Without the `elapsed_at_anchor` term every resume would look like a multi-
/// minute clock step and rewind the curve to its start.
fn run_now(run_epoch: Option<(Timestamp, Instant, SignedDuration, i64)>) -> Timestamp {
    let Some((wall_start, mono_start, elapsed_at_anchor, _)) = run_epoch else {
        return Timestamp::now();
    };
    let wall = Timestamp::now();
    let wall_secs = wall.duration_since(wall_start).as_secs_f64();
    let mono_secs = elapsed_at_anchor.as_secs_f64() + mono_start.elapsed().as_secs_f64();
    if (wall_secs - mono_secs).abs() > CLOCK_STEP_LIMIT.as_secs_f64() {
        tracing::warn!(
            wall_secs,
            mono_secs,
            "system clock stepped mid-run; using monotonic time for this tick"
        );
        wall_start + SignedDuration::from_nanos((mono_secs * 1e9) as i64)
    } else {
        wall
    }
}

/// Reopen the pump's serial port on its own after a mid-run adapter/cable
/// glitch. Runs when the link is lost or writes are failing in a streak;
/// attempts are spaced by an exponential backoff between [`SERIAL_RETRY_MIN`]
/// and [`SERIAL_RETRY_MAX`] while it stays down. `next_retry` doubles as the
/// "already reported" flag: it is `None` outside a recovery streak, so the
/// down/up log lines fire only on the edges.
fn maybe_recover_serial<T: Transport + SwapTransport>(
    engine: &mut Engine<T>,
    events: &broadcast::Sender<DaemonStatus>,
    grace: Duration,
    next_retry: &mut Option<(Instant, Duration)>,
) {
    let needs_recovery =
        engine.serial_lost() || engine.write_fails() >= REOPEN_AFTER_WRITE_FAILS;
    if !needs_recovery {
        *next_retry = None;
        return;
    }
    if next_retry.is_some_and(|(t, _)| Instant::now() < t) {
        return;
    }
    // `None` here means this is the first attempt of a fresh outage; log the
    // down/up transitions only, never the retries in between.
    let prev_delay = next_retry.map(|(_, d)| d);
    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        engine.recover_serial(Timestamp::now())
    }))
    .unwrap_or(false);
    if ok {
        if prev_delay.is_some() {
            tracing::info!("serial link reopened");
        }
        *next_retry = None;
    } else {
        if prev_delay.is_none() {
            tracing::warn!(
                "serial link down; auto-reconnecting (backoff {SERIAL_RETRY_MIN:?}..{SERIAL_RETRY_MAX:?})"
            );
        }
        let delay = match prev_delay {
            Some(d) => (d * 2).min(SERIAL_RETRY_MAX),
            None => SERIAL_RETRY_MIN,
        };
        *next_retry = Some((Instant::now() + delay, delay));
    }
    let _ = events.send(current_status(engine, grace));
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
        Command::ClearHistory(reply) => {
            let pending = engine
                .pending_recovery(now, grace)
                .map(|o| o.is_some())
                .unwrap_or(false);
            let status = engine.status();
            let res = if status.active.is_some() {
                Err("a run is active, stop it before clearing history".to_string())
            } else if status.holding.is_some() {
                // the pump is still driving a completed run's final setpoint;
                // deleting that run's row would orphan what the pump is doing.
                Err("the pump is holding a completed run, stop the pump before clearing history"
                    .to_string())
            } else if pending {
                Err("a crash recovery is pending, resolve it before clearing history".to_string())
            } else {
                engine.store().clear_history().map_err(|e| e.to_string())
            };
            let _ = reply.send(res);
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
            let path = serial.path.clone();
            let msg = match engine.swap_transport(&serial, pump_addr) {
                Ok(kind) => {
                    // `swap_transport` has just run a live probe read for a real
                    // port, so `serial_ok` now reflects whether a pump actually
                    // answered, not merely that the COM port opened.
                    let pump_responding = engine.status().serial_ok;
                    Ok(match kind {
                        TransportKind::Serial(name) if pump_responding => {
                            format!("connected to {name}, pump responding")
                        }
                        TransportKind::Serial(name) => format!(
                            "{name} opened, but the pump is not responding, check power, \
                             wiring, MODBUS address and baud"
                        ),
                        TransportKind::Sim if wanted_serial => {
                            format!("could not open {path}, running on the pump simulator")
                        }
                        TransportKind::Sim => "running on the pump simulator".to_string(),
                    })
                }
                Err(e) => Err(e.to_string()),
            };
            let _ = reply.send(msg);
            true
        }
        Command::StopPump(reply) => {
            let _ = reply.send(engine.stop_pump(now).map_err(|e| e.to_string()));
            true
        }
        Command::SetAllowSimulator(allow, reply) => {
            engine.set_allow_simulator(allow);
            let _ = reply.send(());
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
            allow_simulator: false,
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
    async fn idle_link_probe_flags_a_dead_port_with_no_run() {
        // A real port is configured and was "open" at boot, but every read now
        // fails (cable pulled). With no run to carry recovery, the idle-time
        // link probe must still drive the write-fail streak and latch the alarm.
        let mut sim = SimPump::new(1);
        sim.drop_next = 100_000; // every probe read (and its retry) fails
        let transport: Box<dyn Transport + Send> = Box::new(sim);
        let mut engine = Engine::new(
            Pump::new(transport, 1),
            Store::open_in_memory().unwrap(),
            "test",
        );
        // Kind first, so set_serial doesn't pre-flag the link, detection here
        // must come from the probe streak, not the boot check.
        engine.set_transport_kind(TransportKind::Serial("NOPE_NOT_A_REAL_PORT_99999".into()));
        engine.set_serial(
            crate::config::SerialConfig {
                path: "NOPE_NOT_A_REAL_PORT_99999".into(),
                baud: 9600,
                allow_simulator: false,
            },
            1,
        );
        assert!(!engine.serial_lost(), "precondition: link starts healthy");

        let (events, _keep_rx) = broadcast::channel(16);
        let handle = spawn(engine, Duration::from_secs(300), events);

        // 1 s probe cadence: ~5 failed probes cross REOPEN_AFTER_WRITE_FAILS,
        // then recover_serial can't reopen the port and latches serial_lost.
        tokio::time::sleep(Duration::from_millis(8000)).await;

        let st = handle.call(Command::Status).await.unwrap();
        assert!(st.active.is_none());
        assert!(!st.serial_ok, "idle probe should have flagged the dead link");

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
            allow_simulator: false,
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

    #[tokio::test]
    async fn start_run_is_refused_on_the_simulator_when_not_enabled() {
        let mut engine = Engine::new(
            Pump::new(SimPump::new(1), 1),
            Store::open_in_memory().unwrap(),
            "test",
        );
        // Shipped default: on the simulator, simulator runs not enabled.
        engine.set_serial(
            crate::config::SerialConfig {
                path: "sim".into(),
                baud: 9600,
                allow_simulator: false,
            },
            1,
        );
        let (events, _keep_rx) = broadcast::channel(16);
        let handle = spawn(engine, Duration::from_secs(300), events);

        let err = handle
            .call(|reply| Command::StartRun(cadence_run(), reply))
            .await
            .unwrap()
            .unwrap_err();
        assert!(err.to_lowercase().contains("simulator"), "got: {err}");

        let st = handle.call(Command::Status).await.unwrap();
        assert!(st.simulator);
        assert!(!st.allow_simulator);
        assert!(st.active.is_none());

        handle.shutdown();
    }

    /// A resume adopts a run whose `started_at` is minutes/hours in the past, so
    /// the monotonic anchor is taken with a large elapsed-at-anchor. `run_now`
    /// must treat that as normal and keep returning real wall-clock time, not
    /// mistake the wall/monotonic gap for a clock step and rewind the curve.
    #[test]
    fn run_now_after_resume_keeps_real_elapsed() {
        let elapsed_at_resume = SignedDuration::from_secs(3600); // run was 1 h in
        let wall_start = Timestamp::now() - elapsed_at_resume;
        let epoch = Some((wall_start, Instant::now(), elapsed_at_resume, 1_i64));

        let handed = run_now(epoch);
        let elapsed = handed.duration_since(wall_start).as_secs_f64();
        assert!(
            (elapsed - 3600.0).abs() < 2.0,
            "resume rewound the run clock: elapsed {elapsed}s, expected ~3600s"
        );
    }

    /// The guard must still fire for a genuine forward wall-clock step: the
    /// anchor says ~0 elapsed a moment ago, but the wall clock now reads far
    /// past the start. Pin to the monotonic anchor, not the jumped wall time.
    #[test]
    fn run_now_still_pins_a_real_wall_clock_jump() {
        let wall_start = Timestamp::now() - SignedDuration::from_secs(120);
        let epoch = Some((wall_start, Instant::now(), SignedDuration::ZERO, 1_i64));

        let handed = run_now(epoch);
        let elapsed = handed.duration_since(wall_start).as_secs_f64();
        assert!(
            elapsed < 5.0,
            "clock-step guard did not pin to the anchor: elapsed {elapsed}s"
        );
    }
}
