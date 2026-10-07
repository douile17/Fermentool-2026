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

use fermentool_modbus::Transport;

use crate::config::{ScaleConfig, SerialConfig};
use crate::engine::{
    ActiveStatus, Engine, ErrDetail, HoldingStatus, RecoveryInfo, RunConfig, TickOutcome,
    REOPEN_AFTER_WRITE_FAILS, TICK_INTERVAL,
};
#[cfg(test)]
use crate::store::RunRow;
use crate::store::{EventLevel, NewEvent, RunKind, RunStatus};
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

/// Consecutive panicking passes of the control loop before the process exits
/// (see `control_loop`).
const MAX_LOOP_PANICS: u32 = 50;

/// An interrupted run is resumed by itself (`[resume] prompt = false`) no
/// sooner than this after the daemon starts, so the link probe has had time to
/// tell a silent pump from a live one.
const AUTO_RESUME_DELAY: Duration = Duration::from_secs(10);
/// Between two automatic resume attempts.
const AUTO_RESUME_RETRY: Duration = Duration::from_secs(15);

/// What one pass of the control loop asks for next.
enum Step {
    Next,
    Stop,
}

/// One request to the control thread. Each carries a `oneshot` reply channel.
pub enum Command {
    Status(oneshot::Sender<DaemonStatus>),
    StartRun(RunConfig, oneshot::Sender<Result<i64, ErrDetail>>),
    /// Stop the active run; `Some(id)`: only if it is that run.
    StopRun(Option<i64>, oneshot::Sender<Result<(), String>>),
    /// Abort the active run; `Some(id)`: only if it is that run.
    AbortRun(Option<i64>, oneshot::Sender<Result<(), String>>),
    Recovery(oneshot::Sender<Result<Option<RecoveryInfo>, String>>),
    Resume(oneshot::Sender<Result<i64, ErrDetail>>),
    DiscardRecovery(RunStatus, oneshot::Sender<Result<(), String>>),
    /// Tests only: the API reads runs on its own connection.
    #[cfg(test)]
    GetRun(i64, oneshot::Sender<Result<Option<RunRow>, String>>),
    /// Delete every run + journal. Refused while a run is active or a crash
    /// recovery is pending.
    ClearHistory(oneshot::Sender<Result<(), String>>),
    /// Delete one finished run; `Ok(false)` when it does not exist.
    DeleteRun(i64, oneshot::Sender<Result<bool, String>>),
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
    /// Apply a `[scale]` section saved from Settings to the live engine.
    /// Replies whether the balance is connected afterwards.
    SetScale(ScaleConfig, oneshot::Sender<Result<bool, String>>),
    /// Operator-declared "about to change the bottle", ahead of the automatic
    /// weight-jump threshold.
    TriggerRefillMode(oneshot::Sender<()>),
    /// Operator-declared "bottle is back, settled".
    TriggerRefillDone(oneshot::Sender<()>),
    /// `[resume] prompt = false`: resume an interrupted dosing run without
    /// asking, once the pump answers. Sent at boot and on each config save.
    SetAutoResume(bool, oneshot::Sender<()>),
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
    /// `false` once the scale link is lost or the trim has alarmed. `true`
    /// (the safe "no problem" default) when no scale is configured.
    pub scale_ok: bool,
    /// `None` when no scale is configured.
    /// `false`: the balance is unplugged or not answering. `None` when no
    /// scale is configured.
    pub scale_connected: Option<bool>,
    /// Live balance reading, grams. `None` when unknown.
    pub scale_weight_g: Option<f64>,
    pub scale_stable: Option<bool>,
    pub scale_state: Option<crate::trim::ScaleState>,
    /// `None` when no scale is configured.
    pub trim_c: Option<f64>,
    /// Diagnostic instantaneous rate, `None` until enough samples are buffered.
    pub rate_g_per_min: Option<f64>,
    /// Cumulative feed tracking, `Some` only during a trimmed run.
    pub tracking: Option<crate::engine::TrackingStatus>,
    /// A Stop did not reach the pump: it may still be running. Sent again
    /// until it gets through; the UI raises it.
    pub stop_pending: bool,
    /// What the start found wrong (config.toml, journal check).
    pub warnings: Vec<String>,
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
        scale_ok: st.scale_ok,
        scale_connected: st.scale_connected,
        scale_weight_g: st.scale_weight_g,
        scale_stable: st.scale_stable,
        scale_state: st.scale_state,
        trim_c: st.trim_c,
        rate_g_per_min: st.rate_g_per_min,
        tracking: st.tracking,
        stop_pending: st.stop_pending,
        warnings: st.warnings,
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
    // The burst whose exact end already fired a tick: if that tick did not
    // see it over (wall and monotonic clocks a hair apart), the next whole-
    // second tick finishes it, instead of re-ticking in a tight loop.
    let mut burst_end_fired: Option<i64> = None;
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
    // Gravimetric-trim scale probe deadline, only scheduled while the active
    // run has `gravimetric_trim` set (see `ActiveStatus::gravimetric_trim`).
    let mut next_scale_probe: Option<Instant> = None;
    // Cooldown between automatic scale-reopen attempts, the balance's
    // counterpart to `next_serial_retry`.
    let mut next_scale_retry: Option<(Instant, Duration)> = None;
    // Next resend of a Stop that did not reach the pump.
    let mut next_stop_retry: Option<Instant> = None;
    // The wall clock is off the monotonic one (see `run_now`): logged when it
    // starts and when it ends, not on every tick in between.
    let mut clock_stepped = false;
    // Automatic resume (`[resume] prompt = false`): when to try next, and how
    // many attempts failed for a reason a later try won't fix.
    let mut next_auto_resume: Instant = Instant::now() + AUTO_RESUME_DELAY;
    let mut auto_resume_failures = 0u32;

    // Each pass runs inside `catch_unwind`, not only the engine calls: a
    // panic anywhere in it used to end this thread while the HTTP server
    // lived on, every command answering "control thread is not running"
    // and the pump left on its last setpoint, unregulated, unjournalled.
    // A pass that keeps panicking ends the process instead, so the log-on
    // task restarts the daemon and crash-resume takes over.
    let mut panics = 0u32;
    let mut auto_resume_flag = false;
    let auto_resume = &mut auto_resume_flag;
    loop {
        let step = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Step {
                // Keep the monotonic run epoch in sync with what the engine is running.
                match engine.active_status() {
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

                // Scale auto-recovery runs on its own backoff whenever the engine
                // wants it (idle, or during a trimmed run), not only from the trim's
                // probe: a trimmed run can't start until the scale is back.
                maybe_recover_scale(engine, events, grace, &mut next_scale_retry);

                if engine.tick_interval().is_none() {
                    next_journal = None;
                    next_write = None;
                    // `[resume] prompt = false`: an interrupted dosing run goes on
                    // by itself once the pump answers, instead of waiting for
                    // someone to open the UI after a night-time power cut.
                    if *auto_resume && auto_resume_failures < 3 && Instant::now() >= next_auto_resume {
                        next_auto_resume = Instant::now() + AUTO_RESUME_RETRY;
                        let tried = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            engine.try_auto_resume(Timestamp::now(), grace)
                        }));
                        match tried {
                            Ok(Some(Ok(id))) => {
                                tracing::info!(run_id = id, "interrupted run resumed by itself");
                                let _ = events.send(current_status(engine, grace));
                                return Step::Next;
                            }
                            Ok(Some(Err(e))) if !e.is_link_problem() => {
                                auto_resume_failures += 1;
                                tracing::warn!(attempt = auto_resume_failures, "automatic resume refused: {e}");
                            }
                            Ok(Some(Err(e))) => tracing::debug!("automatic resume waits for the pump: {e}"),
                            Ok(None) => {}
                            Err(_) => tracing::error!("panic in the automatic resume; loop continues"),
                        }
                    }
                    // A Stop that did not get through is sent again, once a second,
                    // as soon as the link is back: the pump may still be running.
                    if engine.stop_pending() && !engine.serial_lost() {
                        let due = *next_stop_retry.get_or_insert_with(Instant::now);
                        if Instant::now() >= due {
                            let sent = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                engine.retry_pending_stop(Timestamp::now())
                            }))
                            .unwrap_or(false);
                            if sent {
                                tracing::info!("pending Stop delivered, pump stopped");
                                let _ = events.send(current_status(engine, grace));
                            }
                            next_stop_retry = Some(Instant::now() + LINK_PROBE_INTERVAL);
                            return Step::Next;
                        }
                    } else {
                        next_stop_retry = None;
                    }
                    // No run: read the balance about once a second, for the live
                    // weight on screen and to notice it being unplugged between runs.
                    if engine.scale_wanted() {
                        let due =
                            *next_scale_probe.get_or_insert_with(|| Instant::now() + LINK_PROBE_INTERVAL);
                        if Instant::now() >= due {
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                engine.probe_scale()
                            }));
                            let _ = events.send(current_status(engine, grace));
                            next_scale_probe = Some(advance_past(due, LINK_PROBE_INTERVAL));
                            return Step::Next;
                        }
                    } else {
                        next_scale_probe = None;
                    }
                    // No run: keep the serial link's health current so a cable pulled
                    // between runs still trips the alarm and the auto-reopen. A link
                    // already lost is the recovery's to confirm (with its own reads,
                    // on its backoff): probing a silent pump every second as well only
                    // held the control thread on a read timeout each time.
                    if engine.serial_is_real() && !engine.serial_lost() {
                        let probe_due =
                            *next_probe.get_or_insert_with(|| Instant::now() + LINK_PROBE_INTERVAL);
                        if Instant::now() >= probe_due {
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                engine.probe_link()
                            }));
                            maybe_recover_serial(engine, events, grace, &mut next_serial_retry);
                            next_probe = Some(advance_past(probe_due, LINK_PROBE_INTERVAL));
                            return Step::Next;
                        }
                    } else {
                        next_probe = None;
                        // Backoff-gated, so free until a retry is due; here as well
                        // as on a quiet `recv_timeout`, which steady UI traffic can
                        // keep from ever firing.
                        maybe_recover_serial(engine, events, grace, &mut next_serial_retry);
                    }
                } else {
                    next_probe = None;
                    let journal_due = *next_journal.get_or_insert_with(|| Instant::now() + TICK_INTERVAL);
                    let write_due =
                        *next_write.get_or_insert_with(|| Instant::now() + WRITE_SETPOINT_INTERVAL);
                    // A calibration burst ends on its exact length, not on the next
                    // whole-second tick: the flow is computed from that length.
                    let end_due = burst_end(engine.active_status().as_ref(), run_epoch)
                        .filter(|_| burst_end_fired != run_epoch.map(|e| e.3));
                    let tick_due = end_due.map_or(journal_due, |end| end.min(journal_due));

                    if Instant::now() >= tick_due {
                        if end_due.is_some_and(|e| e <= journal_due) {
                            burst_end_fired = run_epoch.map(|e| e.3);
                        }
                        let now = run_now(run_epoch, &mut clock_stepped);
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
                        return Step::Next;
                    }

                    if Instant::now() >= write_due {
                        let now = run_now(run_epoch, &mut clock_stepped);
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
                        return Step::Next;
                    }

                    // A trimmed run reads the balance for its regulation; a
                    // calibration burst only for its journal (weight every second, so
                    // the flow can be checked across the burst).
                    let active = engine.active_status();
                    let trims = active.as_ref().is_some_and(|a| a.gravimetric_trim);
                    let calibrating = engine.scale_wanted()
                        && active.as_ref().is_some_and(|a| a.kind == RunKind::Calibration);
                    if trims || calibrating {
                        let scale_probe_due =
                            *next_scale_probe.get_or_insert_with(|| Instant::now() + LINK_PROBE_INTERVAL);
                        if Instant::now() >= scale_probe_due {
                            let now = run_now(run_epoch, &mut clock_stepped);
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                if trims {
                                    engine.scale_tick(now)
                                } else {
                                    engine.probe_scale()
                                }
                            }));
                            next_scale_probe = Some(advance_past(scale_probe_due, LINK_PROBE_INTERVAL));
                            return Step::Next;
                        }
                    } else {
                        next_scale_probe = None;
                    }
                }

                let wait = match (next_journal, next_write) {
                    (Some(j), Some(w)) => {
                        let end = burst_end(engine.active_status().as_ref(), run_epoch)
                            .filter(|_| burst_end_fired != run_epoch.map(|e| e.3));
                        end.map_or(j, |e| e.min(j)).min(w).saturating_duration_since(Instant::now())
                    }
                    // Idle: wake for the link probe if one is scheduled, else just poll.
                    _ => [next_probe, next_scale_probe]
                        .into_iter()
                        .flatten()
                        .min()
                        .map(|p| p.saturating_duration_since(Instant::now()).min(IDLE_POLL))
                        .unwrap_or(IDLE_POLL),
                };
                match rx.recv_timeout(wait) {
                    Ok(Command::Shutdown) => return Step::Stop,
                    Ok(Command::SetAutoResume(on, reply)) => {
                        if on != *auto_resume {
                            tracing::info!(on, "automatic resume of an interrupted run");
                        }
                        *auto_resume = on;
                        auto_resume_failures = 0;
                        let _ = reply.send(());
                    }
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
                    Err(RecvTimeoutError::Disconnected) => return Step::Stop,
                }
            Step::Next
        }));
        match step {
            Ok(Step::Next) => panics = 0,
            Ok(Step::Stop) => break,
            Err(_) => {
                panics += 1;
                tracing::error!(panics, "panic in the control loop; it carries on");
                if panics >= MAX_LOOP_PANICS {
                    tracing::error!("the control loop keeps panicking; exiting so the daemon is restarted");
                    std::process::exit(1);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    tracing::info!("control loop stopped");
}

/// When the active calibration burst reaches its length, on the monotonic
/// clock of `run_epoch`. A couple of ms late on purpose, so the tick there
/// sees the burst over and finishes it. `None` for any other run.
fn burst_end(
    active: Option<&ActiveStatus>,
    epoch: Option<(Timestamp, Instant, SignedDuration, i64)>,
) -> Option<Instant> {
    let a = active.filter(|a| a.kind == RunKind::Calibration)?;
    let (_, anchor, elapsed_at_anchor, _) = epoch.filter(|e| e.3 == a.run_id)?;
    let left = SignedDuration::from_secs(a.duration_s).checked_sub(elapsed_at_anchor)?;
    let left = Duration::try_from(left).unwrap_or(Duration::ZERO);
    anchor.checked_add(left + Duration::from_millis(2))
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
///
/// `stepped` remembers whether the clocks disagree, so the step is logged
/// when it starts and when it ends, not on each of the ~8 calls a second in
/// between (a step that lasts the rest of a 100 h run flooded the log).
fn run_now(run_epoch: Option<(Timestamp, Instant, SignedDuration, i64)>, stepped: &mut bool) -> Timestamp {
    let Some((wall_start, mono_start, elapsed_at_anchor, _)) = run_epoch else {
        *stepped = false;
        return Timestamp::now();
    };
    let wall = Timestamp::now();
    let wall_secs = wall.duration_since(wall_start).as_secs_f64();
    let mono_secs = elapsed_at_anchor.as_secs_f64() + mono_start.elapsed().as_secs_f64();
    if (wall_secs - mono_secs).abs() > CLOCK_STEP_LIMIT.as_secs_f64() {
        if !*stepped {
            tracing::warn!(
                wall_secs,
                mono_secs,
                "system clock stepped mid-run; using monotonic time until it agrees again"
            );
            *stepped = true;
        }
        wall_start + SignedDuration::from_nanos((mono_secs * 1e9) as i64)
    } else {
        if *stepped {
            tracing::info!("system clock agrees with the run's monotonic time again");
            *stepped = false;
        }
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

/// Reopen the scale on its own after a mid-run disconnect (or a scale that
/// never opened at boot), the gravimetric trim's counterpart to
/// [`maybe_recover_serial`], spaced by the same exponential backoff. Acts
/// only while [`Engine::scale_recovery_wanted`] holds (idle, or a trimmed run
/// is active). An outage (`next_retry` set) ends only on a weight
/// ([`Engine::scale_link_up`]), never on a port that merely reopened: that
/// used to reset the backoff, so a switched-off balance was reopened and read
/// five times every few seconds, journalled lost/recovered each time. No
/// `[scale]` configured: never retries, never logs.
fn maybe_recover_scale<T: Transport>(
    engine: &mut Engine<T>,
    events: &broadcast::Sender<DaemonStatus>,
    grace: Duration,
    next_retry: &mut Option<(Instant, Duration)>,
) {
    // During a run the balance coming and going is part of its story (and
    // notified): journal it. Idle, the log line is enough.
    let journal = |engine: &Engine<T>, kind: &str, level: EventLevel, detail: &str| {
        if let Some(a) = engine.active_status() {
            let _ = engine.store().log_event(&NewEvent {
                run_id: Some(a.run_id),
                wall_time: Timestamp::now(),
                level,
                kind: kind.into(),
                detail: Some(detail.into()),
            });
        }
    };
    let recovered = |engine: &Engine<T>, next_retry: &mut Option<(Instant, Duration)>| {
        tracing::info!("scale link back, balance answering");
        journal(engine, "scale_recovered", EventLevel::Info, "balance answering again");
        *next_retry = None;
        let _ = events.send(current_status(engine, grace));
    };
    if !engine.scale_recovery_wanted() {
        if next_retry.is_some() {
            if engine.scale_link_up() {
                recovered(engine, next_retry);
            } else if !engine.scale_wanted() {
                // The balance was removed in Settings: no outage to follow.
                *next_retry = None;
            }
            // Otherwise a reopened port is waiting for its first weight (or
            // a plain dosing run leaves the balance alone): keep the outage.
        }
        return;
    }
    if next_retry.is_some_and(|(t, _)| Instant::now() < t) {
        return;
    }
    let prev_delay = next_retry.map(|(_, d)| d);
    let reopened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| engine.recover_scale()))
        .unwrap_or(false);
    if engine.scale_link_up() {
        // The balance answered on the link already held.
        if prev_delay.is_some() {
            recovered(engine, next_retry);
        } else {
            *next_retry = None;
        }
        return;
    }
    if reopened {
        tracing::debug!("scale port reopened, waiting for a weight");
    }
    {
        if prev_delay.is_none() {
            tracing::warn!(
                "scale link down; auto-reconnecting (backoff {SERIAL_RETRY_MIN:?}..{SERIAL_RETRY_MAX:?})"
            );
            journal(
                engine,
                "scale_lost",
                EventLevel::Error,
                "balance not answering (cable, power?); correction frozen, retrying",
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

/// Why a Stop or Abort naming run `id` must not act: another run is the
/// active one (a page left open on an older run must not end the current
/// one). `None` to go ahead, an idle engine answering for itself.
fn not_the_active_run<T: Transport>(engine: &Engine<T>, id: Option<i64>) -> Option<String> {
    let (want, active) = (id?, engine.active_status()?.run_id);
    (want != active).then(|| format!("run #{want} is not the active run (#{active} is)"))
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
            let _ = reply.send(engine.start_run(cfg, now).map_err(|e| e.detail()));
            true
        }
        Command::StopRun(id, reply) => {
            let res = match not_the_active_run(engine, id) {
                Some(e) => Err(e),
                None => engine.stop_run(now).map_err(|e| e.to_string()),
            };
            let _ = reply.send(res);
            true
        }
        Command::AbortRun(id, reply) => {
            let res = match not_the_active_run(engine, id) {
                Some(e) => Err(e),
                None => engine.abort_run(now).map_err(|e| e.to_string()),
            };
            let _ = reply.send(res);
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
            let _ = reply.send(engine.resume(now, grace).map_err(|e| e.detail()));
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
        #[cfg(test)]
        Command::GetRun(id, reply) => {
            let _ = reply.send(engine.store().run(id).map_err(|e| e.to_string()));
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
        Command::DeleteRun(id, reply) => {
            let pending = engine
                .pending_recovery(now, grace)
                .ok()
                .flatten()
                .is_some_and(|r| r.run_id == id);
            let status = engine.status();
            let res = if status.active.as_ref().is_some_and(|a| a.run_id == id) {
                Err("this run is active, stop it before deleting it".to_string())
            } else if status.holding.as_ref().is_some_and(|h| h.run_id == id) {
                Err("the pump is holding this run, stop the pump before deleting it".to_string())
            } else if pending {
                Err("a crash recovery is pending for this run, resolve it first".to_string())
            } else {
                engine.store().delete_run(id).map_err(|e| e.to_string())
            };
            let _ = reply.send(res);
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
        Command::SetScale(cfg, reply) => {
            let _ = reply.send(engine.reconfigure_scale(cfg));
            true
        }
        Command::TriggerRefillMode(reply) => {
            engine.trigger_refill_mode();
            let _ = reply.send(());
            true
        }
        Command::TriggerRefillDone(reply) => {
            engine.trigger_refill_done();
            let _ = reply.send(());
            true
        }
        // Handled by the loop itself, which owns the flag.
        Command::SetAutoResume(_, reply) => {
            let _ = reply.send(());
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use fermentool_curves::CurveSpec;
    use fermentool_modbus::{Pump, SimPump};

    use crate::engine::{Engine, RunConfig};
    use crate::store::{ControlVar, Direction, RunKind, Store};

    fn cadence_run() -> RunConfig {
        RunConfig {
            name: "cadence".into(),
            control_var: ControlVar::Rpm,
            direction: Direction::Cw,
            pump_addr: 1,
            curve: CurveSpec::linear(0.0, 100.0, Duration::from_secs(3600)),
            gravimetric_trim: false,
            kind: RunKind::Dosing,
            tubing_calibration_id: None,
            responsible: None,
        }
    }

    /// A balance switched off mid-run, then on again. One `scale_lost` for
    /// the whole outage and a growing backoff, not a lost/recovered pair (and
    /// five failed reads) every few seconds; no reopen of a port that works;
    /// one `scale_recovered` once a weight comes back.
    #[test]
    fn a_silent_balance_is_lost_once_and_recovered_once() {
        use fermentool_modbus::TransportError;
        use std::sync::atomic::{AtomicBool, Ordering};
        static ON: AtomicBool = AtomicBool::new(false);
        struct Balance;
        impl Transport for Balance {
            fn transaction(&mut self, _: &[u8]) -> Result<Vec<u8>, TransportError> {
                if ON.load(Ordering::SeqCst) {
                    Ok(b"S S      640.0 g\r\n".to_vec())
                } else {
                    Err(TransportError::Timeout)
                }
            }
        }
        fn open_never(_: &ScaleConfig) -> Option<Box<dyn Transport + Send>> {
            panic!("a silent balance on a working port is not reopened")
        }

        let mut e = Engine::new(Pump::new(SimPump::new(1), 1), Store::open_in_memory().unwrap(), "test");
        e.attach_scale(Some(Box::new(Balance)), ScaleConfig { path: "COM-off".into(), ..Default::default() });
        e.set_scale_opener_for_test(open_never);
        let run = RunConfig { control_var: ControlVar::MlMin, gravimetric_trim: true, ..cadence_run() };
        let id = e.start_run(run, Timestamp::now()).unwrap();
        let (events, _rx) = broadcast::channel(64);
        let grace = Duration::from_secs(300);
        let mut next_retry = None;
        let retry_now = |next: &mut Option<(Instant, Duration)>| {
            if let Some((_, d)) = *next {
                *next = Some((Instant::now() - Duration::from_millis(1), d));
            }
        };

        for _ in 0..REOPEN_AFTER_WRITE_FAILS {
            e.scale_tick(Timestamp::now());
        }
        let mut delays = vec![];
        for _ in 0..6 {
            maybe_recover_scale(&mut e, &events, grace, &mut next_retry);
            delays.push(next_retry.expect("the outage is still followed").1);
            e.scale_tick(Timestamp::now());
            retry_now(&mut next_retry);
        }
        assert_eq!(delays.last(), Some(&SERIAL_RETRY_MAX), "backoff grows: {delays:?}");
        assert_eq!(e.status().scale_connected, Some(false));

        ON.store(true, Ordering::SeqCst);
        maybe_recover_scale(&mut e, &events, grace, &mut next_retry);
        assert!(next_retry.is_none(), "a weight ends the outage");
        assert_eq!(e.status().scale_connected, Some(true));

        let kinds: Vec<String> = e.store().events(Some(id), 200).unwrap().into_iter().map(|ev| ev.kind).collect();
        let count = |k: &str| kinds.iter().filter(|x| *x == k).count();
        assert_eq!(count("scale_lost"), 1, "{kinds:?}");
        assert_eq!(count("scale_recovered"), 1, "{kinds:?}");
    }

    /// A calibration burst lasts what was asked, to the millisecond, not
    /// until the next whole-second tick: its length is what the flow is
    /// computed from.
    #[tokio::test]
    async fn a_calibration_burst_ends_on_its_exact_length() {
        let engine = Engine::new(
            Pump::new(SimPump::new(1), 1),
            Store::open_in_memory().unwrap(),
            "test",
        );
        let (events, _keep_rx) = broadcast::channel(64);
        let handle = spawn(engine, Duration::from_secs(300), events);
        let cfg = RunConfig {
            curve: CurveSpec::linear(50.0, 50.0, Duration::from_secs(2)),
            kind: RunKind::Calibration,
            ..cadence_run()
        };
        // Start mid-second, so the burst's end falls between two ticks.
        tokio::time::sleep(Duration::from_millis(370)).await;
        let id = handle.call(|reply| Command::StartRun(cfg, reply)).await.unwrap().unwrap();
        tokio::time::sleep(Duration::from_millis(2600)).await;
        let run = handle.call(|reply| Command::GetRun(id, reply)).await.unwrap().unwrap().unwrap();
        let ended = run.ended_at.expect("the burst finished by itself");
        let ms = ended.duration_since(run.started_at).as_millis();
        assert!((2000..2040).contains(&ms), "burst lasted {ms} ms");
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
            gravimetric_trim: false,
            kind: RunKind::Dosing,
            tubing_calibration_id: None,
            responsible: None,
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
    async fn a_finished_curve_keeps_the_run_active_until_stop() {
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
            gravimetric_trim: false,
            kind: RunKind::Dosing,
            tubing_calibration_id: None,
            responsible: None,
        };
        let id = handle
            .call(|reply| Command::StartRun(cfg, reply))
            .await
            .unwrap()
            .unwrap();

        // Let the 1 s curve run out: the run stays active, holding its end
        // value and still ticking, with no separate "hold" left behind.
        tokio::time::sleep(Duration::from_millis(2500)).await;
        let st = handle.call(Command::Status).await.unwrap();
        let a = st.active.expect("the run stays active past its curve");
        assert!(a.curve_done);
        assert!(a.last_seq.unwrap() >= 1, "journal keeps ticking in the hold");
        assert!(st.holding.is_none());

        handle.call(|r| Command::StopRun(Some(id), r)).await.unwrap().unwrap();
        let st = handle.call(Command::Status).await.unwrap();
        assert!(st.active.is_none());
        let run = handle.call(|r| Command::GetRun(id, r)).await.unwrap().unwrap().unwrap();
        assert_eq!(run.status, crate::store::RunStatus::Completed);

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
        assert_eq!(err.code, "simulator_not_allowed");
        assert!(err.message.to_lowercase().contains("simulator"), "got: {}", err.message);
        assert!(err.hint.is_some());

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

        let handed = run_now(epoch, &mut false);
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

        let mut stepped = false;
        let handed = run_now(epoch, &mut stepped);
        let elapsed = handed.duration_since(wall_start).as_secs_f64();
        assert!(stepped, "the step is noted, to be logged once");
        assert!(
            elapsed < 5.0,
            "clock-step guard did not pin to the anchor: elapsed {elapsed}s"
        );
    }
}
