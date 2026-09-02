//! The control engine: run lifecycle + the tick loop.
//!
//! [`Engine::tick`] does exactly one tick's work — compute the setpoint for the
//! **real elapsed time** (`now - started_at`), write it to the pump, journal it,
//! and finish the run when the curve is done. Time is injected, so a 100 h run
//! is exercised in milliseconds by calling `tick` with synthetic timestamps
//! (`docs/IMPLEMENTATION_PLAN.md` §4.5).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use fermentool_curves::CurveSpec;
use fermentool_modbus::{limits, Pump, PumpError, PumpTransport, Transport};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::store::{
    ControlVar, Direction, EventLevel, NewEvent, NewRun, NewTick, RunStatus, Store, StoreError,
};

/// Fixed **journal** cadence — not user-tunable.
///
/// [`Engine::tick`] fires once a second: it appends a journal row, owns run
/// completion, and is the heartbeat that keeps writing the pump when the curve
/// is flat. 1 s keeps the journal small over a 100 h run.
///
/// The pump setpoint itself is refreshed faster — the control loop also calls
/// [`Engine::apply_setpoint`] ~every 150 ms so a steep ramp steps the pump
/// through each grid value ([`setpoint_grid`]) instead of jumping a whole
/// second's worth; that write is skipped when the value hasn't moved, so the
/// bus stays quiet on gentle curves.
pub const TICK_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub enum EngineError {
    Pump(PumpError),
    Store(StoreError),
    /// The run configuration is invalid.
    Config(String),
    /// A run is already active.
    Busy,
    /// No run is active.
    Idle,
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Pump(e) => write!(f, "pump: {e}"),
            EngineError::Store(e) => write!(f, "store: {e}"),
            EngineError::Config(s) => write!(f, "invalid run config: {s}"),
            EngineError::Busy => write!(f, "a run is already active"),
            EngineError::Idle => write!(f, "no run is active"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<PumpError> for EngineError {
    fn from(e: PumpError) -> Self {
        EngineError::Pump(e)
    }
}

impl From<StoreError> for EngineError {
    fn from(e: StoreError) -> Self {
        EngineError::Store(e)
    }
}

type Result<T> = std::result::Result<T, EngineError>;

/// What the caller supplies to start a run (also the `POST /api/runs` body).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunConfig {
    pub name: String,
    pub control_var: ControlVar,
    pub direction: Direction,
    pub pump_addr: u8,
    pub curve: CurveSpec,
}

/// What [`Engine::pending_recovery`] found: a run that was `running` when the
/// process last stopped, i.e. it did not end cleanly.
#[derive(Debug, Clone, Serialize)]
pub struct RecoveryInfo {
    pub run_id: i64,
    pub name: String,
    pub started_at: Timestamp,
    pub now: Timestamp,
    /// Real time elapsed since `started_at`.
    pub elapsed_s: f64,
    pub duration_s: i64,
    pub control_var: ControlVar,
    /// The setpoint the curve prescribes for `elapsed_s` right now — where a
    /// resume would put the pump.
    pub resume_target: f64,
    /// `elapsed` is past `duration + grace`: the curve finished while offline,
    /// so the daemon should offer *finish* / *abort*, not *resume*.
    pub past_end: bool,
    /// `seq` of the last journalled tick before the interruption.
    pub last_seq: Option<i64>,
}

/// Result of a single [`Engine::tick`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum TickOutcome {
    /// No run is active.
    Idle,
    /// The setpoint was computed and journalled; `written_ok` is the pump write.
    Applied {
        seq: i64,
        target: f64,
        written_ok: bool,
    },
    /// The curve reached its end; the run is now `completed`.
    Finished { seq: i64, target: f64 },
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineStatus {
    pub active: Option<ActiveStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActiveStatus {
    pub run_id: i64,
    pub started_at: Timestamp,
    pub duration_s: i64,
    pub control_var: ControlVar,
    /// Always [`TICK_INTERVAL`] in seconds — kept in the payload for display.
    pub tick_interval_s: u32,
    pub last_seq: Option<i64>,
    pub last_target: Option<f64>,
}

struct ActiveRun {
    id: i64,
    started_at: Timestamp,
    /// Resolved and clamp-intersected with the pump limits.
    spec: CurveSpec,
    control_var: ControlVar,
    duration_s: i64,
    next_seq: i64,
    last_target: Option<f64>,
    /// Setpoint last written to the pump, already snapped to [`setpoint_grid`].
    /// [`Engine::apply_setpoint`] compares against this to skip no-op writes.
    last_write_q: Option<f64>,
}

/// Owns the pump and the journal for the lifetime of the process.
pub struct Engine<T: Transport> {
    pump: Pump<T>,
    store: Store,
    app_version: String,
    active: Option<ActiveRun>,
}

impl<T: Transport> Engine<T> {
    pub fn new(pump: Pump<T>, store: Store, app_version: impl Into<String>) -> Self {
        Self {
            pump,
            store,
            app_version: app_version.into(),
            active: None,
        }
    }

    /// Read access to the journal (for the API / diagnostics).
    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn status(&self) -> EngineStatus {
        EngineStatus {
            active: self.active.as_ref().map(|a| ActiveStatus {
                run_id: a.id,
                started_at: a.started_at,
                duration_s: a.duration_s,
                control_var: a.control_var,
                tick_interval_s: TICK_INTERVAL.as_secs() as u32,
                last_seq: (a.next_seq > 0).then_some(a.next_seq - 1),
                last_target: a.last_target,
            }),
        }
    }

    /// [`TICK_INTERVAL`] while a run is active, so the control loop knows how
    /// long to wait between ticks.
    pub fn tick_interval(&self) -> Option<Duration> {
        self.active.as_ref().map(|_| TICK_INTERVAL)
    }

    /// Start a run at `now`: intersect the curve clamps with the pump limits,
    /// validate, run the pump start sequence, insert the run, begin ticking.
    pub fn start_run(&mut self, cfg: RunConfig, now: Timestamp) -> Result<i64> {
        if self.active.is_some() {
            return Err(EngineError::Busy);
        }

        let (lo, hi) = match cfg.control_var {
            ControlVar::Rpm => (limits::RPM_MIN, limits::RPM_MAX),
            ControlVar::MlMin => (limits::FLOW_MIN, limits::FLOW_MAX),
        };
        let mut spec = cfg.curve.clone();
        spec.clamp_min = spec.clamp_min.max(lo);
        spec.clamp_max = spec.clamp_max.min(hi);
        spec.validate().map_err(EngineError::Config)?;

        let duration_s = spec.duration.as_secs() as i64;
        let first = quantize(spec.value_at(Duration::ZERO), setpoint_grid(cfg.control_var));

        // Pump start sequence (docs/IMPLEMENTATION_PLAN.md §4.3). The pump's
        // head-type / tubing-size registers are deliberately left untouched — see
        // the note in migrations/0001_init.sql.
        self.pump.set_direction(cfg.direction == Direction::Cw)?;
        write_setpoint(&mut self.pump, cfg.control_var, first)?;
        self.pump.start()?;

        let id = self.store.insert_run(&NewRun {
            name: cfg.name.clone(),
            started_at: now,
            control_var: cfg.control_var,
            direction: cfg.direction,
            tick_interval_s: TICK_INTERVAL.as_secs() as i64,
            pump_addr: cfg.pump_addr,
            app_version: self.app_version.clone(),
            curve: spec.clone(),
        })?;
        self.store.log_event(&NewEvent {
            run_id: Some(id),
            wall_time: now,
            level: EventLevel::Info,
            kind: "start".into(),
            detail: Some(format!("first setpoint {first:.3}")),
        })?;

        self.active = Some(ActiveRun {
            id,
            started_at: now,
            spec,
            control_var: cfg.control_var,
            duration_s,
            next_seq: 0,
            last_target: Some(first),
            last_write_q: Some(first),
        });
        Ok(id)
    }

    /// One tick: compute the setpoint for the real elapsed time, write it,
    /// journal it, and finish the run if the curve is done.
    pub fn tick(&mut self, now: Timestamp) -> Result<TickOutcome> {
        let Some(active) = self.active.as_mut() else {
            return Ok(TickOutcome::Idle);
        };
        let id = active.id;
        let control_var = active.control_var;
        let duration_s = active.duration_s;
        let elapsed_s = now.duration_since(active.started_at).as_secs_f64().max(0.0);
        let target = quantize(
            active.spec.value_at(Duration::from_secs_f64(elapsed_s)),
            setpoint_grid(control_var),
        );
        let seq = active.next_seq;
        active.next_seq += 1;
        active.last_target = Some(target);
        active.last_write_q = Some(target);

        let write = write_setpoint(&mut self.pump, control_var, target);
        let written_ok = write.is_ok();
        if let Err(e) = &write {
            self.store.log_event(&NewEvent {
                run_id: Some(id),
                wall_time: now,
                level: EventLevel::Warn,
                kind: "write_fail".into(),
                detail: Some(e.to_string()),
            })?;
        }
        self.store.append_tick(&NewTick {
            run_id: id,
            seq,
            wall_time: now,
            elapsed_s,
            target,
            written_ok,
            readback: None,
            note: None,
        })?;

        if elapsed_s >= duration_s as f64 {
            let _ = self.pump.stop();
            self.store.finish_run(id, RunStatus::Completed, now)?;
            self.store.log_event(&NewEvent {
                run_id: Some(id),
                wall_time: now,
                level: EventLevel::Info,
                kind: "curve_done".into(),
                detail: None,
            })?;
            self.active = None;
            return Ok(TickOutcome::Finished { seq, target });
        }
        Ok(TickOutcome::Applied {
            seq,
            target,
            written_ok,
        })
    }

    /// Sub-second setpoint write between journal ticks: compute the setpoint for
    /// the **real elapsed time**, snap it to [`setpoint_grid`], and write it to
    /// the pump **only if it changed** since the last write. No journal row, no
    /// completion check — [`Engine::tick`] owns those. Returns `true` when a new
    /// value was written.
    ///
    /// Called ~every 150 ms by the control loop so a steep ramp steps the pump
    /// through each grid value instead of jumping a journal tick's worth at once.
    pub fn apply_setpoint(&mut self, now: Timestamp) -> Result<bool> {
        let Some(active) = self.active.as_mut() else {
            return Ok(false);
        };
        let id = active.id;
        let control_var = active.control_var;
        let elapsed_s = now.duration_since(active.started_at).as_secs_f64().max(0.0);
        // The curve's final value belongs to the completing tick.
        if elapsed_s >= active.duration_s as f64 {
            return Ok(false);
        }
        let target = quantize(
            active.spec.value_at(Duration::from_secs_f64(elapsed_s)),
            setpoint_grid(control_var),
        );
        if active.last_write_q == Some(target) {
            return Ok(false);
        }

        match write_setpoint(&mut self.pump, control_var, target) {
            Ok(()) => {
                active.last_write_q = Some(target);
                active.last_target = Some(target);
                Ok(true)
            }
            Err(e) => {
                // The next journal tick will journal the write state; here just
                // note it. Keep the run going — a transient bus error recovers.
                self.store.log_event(&NewEvent {
                    run_id: Some(id),
                    wall_time: now,
                    level: EventLevel::Warn,
                    kind: "write_fail".into(),
                    detail: Some(e.to_string()),
                })?;
                Ok(false)
            }
        }
    }

    /// Graceful stop: stop the pump, mark the run `stopped`.
    pub fn stop_run(&mut self, now: Timestamp) -> Result<()> {
        self.end_run(now, RunStatus::Stopped, "stop")
    }

    /// Abort: stop the pump, mark the run `aborted`.
    pub fn abort_run(&mut self, now: Timestamp) -> Result<()> {
        self.end_run(now, RunStatus::Aborted, "abort")
    }

    fn end_run(&mut self, now: Timestamp, status: RunStatus, kind: &str) -> Result<()> {
        let Some(active) = self.active.take() else {
            return Err(EngineError::Idle);
        };
        let _ = self.pump.stop();
        self.store.finish_run(active.id, status, now)?;
        self.store.log_event(&NewEvent {
            run_id: Some(active.id),
            wall_time: now,
            level: EventLevel::Info,
            kind: kind.to_string(),
            detail: None,
        })?;
        Ok(())
    }

    // ---- crash recovery (docs/IMPLEMENTATION_PLAN.md §4.7) ----

    /// Inspect the journal for a run that was `running` when the process last
    /// stopped. Read-only — safe to call repeatedly (e.g. from a polling UI).
    /// Returns `None` if nothing needs recovering, or if this process is already
    /// running the run.
    pub fn pending_recovery(
        &self,
        now: Timestamp,
        grace: Duration,
    ) -> Result<Option<RecoveryInfo>> {
        if self.active.is_some() {
            return Ok(None);
        }
        let Some(run) = self.store.running_run()? else {
            return Ok(None);
        };
        let elapsed_s = now.duration_since(run.started_at).as_secs_f64().max(0.0);
        let resume_target = run.curve.value_at(Duration::from_secs_f64(elapsed_s));
        let last_seq = self.store.last_tick(run.id)?.map(|t| t.seq);
        Ok(Some(RecoveryInfo {
            run_id: run.id,
            name: run.name,
            started_at: run.started_at,
            now,
            elapsed_s,
            duration_s: run.duration_s,
            control_var: run.control_var,
            resume_target,
            past_end: elapsed_s > run.duration_s as f64 + grace.as_secs_f64(),
            last_seq,
        }))
    }

    /// Resume the interrupted run: re-run the **full** pump start sequence (the
    /// pump may have been power-cycled) at the setpoint the curve prescribes for
    /// the real elapsed time, then continue ticking. `started_at` is unchanged,
    /// so timing stays absolute.
    pub fn resume(&mut self, now: Timestamp, grace: Duration) -> Result<i64> {
        if self.active.is_some() {
            return Err(EngineError::Busy);
        }
        let Some(run) = self.store.running_run()? else {
            return Err(EngineError::Idle);
        };
        let elapsed_s = now.duration_since(run.started_at).as_secs_f64().max(0.0);
        if elapsed_s > run.duration_s as f64 + grace.as_secs_f64() {
            return Err(EngineError::Config(
                "run is past its end plus grace; finish or abort instead".into(),
            ));
        }
        let target = quantize(
            run.curve.value_at(Duration::from_secs_f64(elapsed_s)),
            setpoint_grid(run.control_var),
        );

        self.log_crash_detected(run.id, now, elapsed_s, run.duration_s)?;

        self.pump.set_direction(run.direction == Direction::Cw)?;
        write_setpoint(&mut self.pump, run.control_var, target)?;
        self.pump.start()?;

        let next_seq = self.store.last_tick(run.id)?.map_or(0, |t| t.seq + 1);
        self.store.log_event(&NewEvent {
            run_id: Some(run.id),
            wall_time: now,
            level: EventLevel::Info,
            kind: "resume".into(),
            detail: Some(format!(
                "elapsed {elapsed_s:.0}s, setpoint {target:.3}, seq from {next_seq}"
            )),
        })?;

        self.active = Some(ActiveRun {
            id: run.id,
            started_at: run.started_at,
            spec: run.curve,
            control_var: run.control_var,
            duration_s: run.duration_s,
            next_seq,
            last_target: Some(target),
            last_write_q: Some(target),
        });
        Ok(run.id)
    }

    /// End the interrupted run without resuming it: stop the pump and mark it
    /// `status` (must be terminal). Used for the *finish* / *abort* choices.
    pub fn discard_recovery(&mut self, now: Timestamp, status: RunStatus) -> Result<()> {
        if self.active.is_some() {
            return Err(EngineError::Busy);
        }
        if status == RunStatus::Running {
            return Err(EngineError::Config(
                "discard status must be terminal".into(),
            ));
        }
        let Some(run) = self.store.running_run()? else {
            return Err(EngineError::Idle);
        };
        let elapsed_s = now.duration_since(run.started_at).as_secs_f64().max(0.0);
        self.log_crash_detected(run.id, now, elapsed_s, run.duration_s)?;
        let _ = self.pump.stop();
        self.store.finish_run(run.id, status, now)?;
        let kind = match status {
            RunStatus::Aborted => "abort",
            RunStatus::Stopped => "stop",
            RunStatus::Completed => "curve_done",
            RunStatus::Running => unreachable!("guarded above"),
        };
        self.store.log_event(&NewEvent {
            run_id: Some(run.id),
            wall_time: now,
            level: EventLevel::Info,
            kind: kind.into(),
            detail: Some("resolved without resuming".into()),
        })?;
        Ok(())
    }

    fn log_crash_detected(
        &self,
        run_id: i64,
        now: Timestamp,
        elapsed_s: f64,
        duration_s: i64,
    ) -> Result<()> {
        self.store.log_event(&NewEvent {
            run_id: Some(run_id),
            wall_time: now,
            level: EventLevel::Warn,
            kind: "crash_detected".into(),
            detail: Some(format!(
                "unclean shutdown; elapsed {elapsed_s:.0}s of {duration_s}s at restart"
            )),
        })?;
        Ok(())
    }
}

fn write_setpoint<T: Transport>(
    pump: &mut Pump<T>,
    control_var: ControlVar,
    value: f64,
) -> std::result::Result<(), PumpError> {
    match control_var {
        ControlVar::Rpm => pump.set_speed_rpm(value as f32),
        ControlVar::MlMin => pump.set_flow_ml_min(value as f32),
    }
}

/// The pump's usable setpoint step for a control variable.
///
/// `Rpm` snaps to the documented 0.1 rpm motor step. `MlMin` snaps to the pump's
/// 4-decimal display resolution — its own firmware then collapses that onto a
/// motor step, since Fermentool does not own the tube calibration and cannot do
/// that conversion itself (see `docs/DESIGN.md`).
fn setpoint_grid(control_var: ControlVar) -> f64 {
    match control_var {
        ControlVar::Rpm => 0.1,
        ControlVar::MlMin => 1e-4,
    }
}

/// Snap `value` to the nearest multiple of `step`.
fn quantize(value: f64, step: f64) -> f64 {
    (value / step).round() * step
}

/// Drive an engine in real time until the run finishes or `stop` is set. The
/// supervised, panic-catching wrapper comes with the daemon wiring (milestone 7).
pub fn run_blocking<T: Transport>(engine: &mut Engine<T>, stop: &AtomicBool) -> Result<()> {
    while !stop.load(Ordering::Relaxed) {
        match engine.tick(Timestamp::now())? {
            TickOutcome::Idle | TickOutcome::Finished { .. } => break,
            TickOutcome::Applied { .. } => {}
        }
        sleep_interruptible(engine.tick_interval().unwrap_or(TICK_INTERVAL), stop);
    }
    Ok(())
}

fn sleep_interruptible(total: Duration, stop: &AtomicBool) {
    let slice = Duration::from_millis(250);
    let mut left = total;
    while left > Duration::ZERO && !stop.load(Ordering::Relaxed) {
        let step = left.min(slice);
        std::thread::sleep(step);
        left = left.saturating_sub(step);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fermentool_modbus::SimPump;
    use jiff::SignedDuration;

    fn t0() -> Timestamp {
        "2026-09-01T09:00:00Z".parse().unwrap()
    }

    fn at(secs: i64) -> Timestamp {
        t0() + SignedDuration::from_secs(secs)
    }

    fn engine() -> Engine<SimPump> {
        Engine::new(
            Pump::new(SimPump::new(1), 1),
            Store::open_in_memory().unwrap(),
            "test",
        )
    }

    fn linear_cfg() -> RunConfig {
        RunConfig {
            name: "r".into(),
            control_var: ControlVar::Rpm,
            direction: Direction::Cw,
            pump_addr: 1,
            curve: CurveSpec::linear(0.0, 100.0, Duration::from_secs(3600)),
        }
    }

    #[test]
    fn start_runs_the_pump_sequence_and_records_the_run() {
        let mut e = engine();
        let id = e.start_run(linear_cfg(), t0()).unwrap();
        assert!(e.pump.transport().running());
        assert!(e.pump.transport().direction_cw());
        // value_at(0) = 0.0, clamp-intersected to RPM_MIN
        assert!((e.pump.transport().speed_rpm() as f64 - limits::RPM_MIN).abs() < 1e-3);

        let row = e.store().run(id).unwrap().unwrap();
        assert_eq!(row.status, RunStatus::Running);
        assert!(row.curve.clamp_max <= limits::RPM_MAX + 1e-9);
        assert!(row.curve.clamp_min >= limits::RPM_MIN - 1e-9);
    }

    #[test]
    fn second_start_is_busy() {
        let mut e = engine();
        e.start_run(linear_cfg(), t0()).unwrap();
        assert!(matches!(
            e.start_run(linear_cfg(), t0()),
            Err(EngineError::Busy)
        ));
    }

    #[test]
    fn tick_tracks_the_curve_and_appends_journal_rows() {
        let mut e = engine();
        let id = e.start_run(linear_cfg(), t0()).unwrap();

        let o = e.tick(at(1800)).unwrap(); // 30 min into a 60 min 0->100 ramp
        assert!(matches!(o, TickOutcome::Applied { seq: 0, .. }));
        assert!((e.pump.transport().speed_rpm() - 50.0).abs() < 0.5);

        e.tick(at(1810)).unwrap();
        assert_eq!(e.store().tick_count(id).unwrap(), 2);
        assert_eq!(e.store().last_tick(id).unwrap().unwrap().seq, 1);
    }

    #[test]
    fn tick_at_duration_completes_the_run() {
        let mut e = engine();
        let id = e.start_run(linear_cfg(), t0()).unwrap();

        let o = e.tick(at(3600)).unwrap();
        assert!(matches!(o, TickOutcome::Finished { .. }));
        assert!(!e.pump.transport().running());
        assert!(e.store().running_run().unwrap().is_none());
        assert_eq!(
            e.store().run(id).unwrap().unwrap().status,
            RunStatus::Completed
        );
        assert!(matches!(e.tick(at(3610)).unwrap(), TickOutcome::Idle));
    }

    #[test]
    fn write_failure_is_journalled_but_the_run_continues() {
        let mut e = engine();
        let id = e.start_run(linear_cfg(), t0()).unwrap();

        e.pump.transport_mut().drop_next = 1;
        let o = e.tick(at(600)).unwrap();
        assert!(matches!(
            o,
            TickOutcome::Applied {
                written_ok: false,
                ..
            }
        ));
        assert!(!e.store().last_tick(id).unwrap().unwrap().written_ok);
        assert!(e
            .store()
            .events(Some(id), 20)
            .unwrap()
            .iter()
            .any(|ev| ev.kind == "write_fail"));

        // recovers on the next tick
        let o2 = e.tick(at(610)).unwrap();
        assert!(matches!(
            o2,
            TickOutcome::Applied {
                written_ok: true,
                ..
            }
        ));
    }

    #[test]
    fn stop_and_abort_end_the_run() {
        let mut e = engine();
        let id = e.start_run(linear_cfg(), t0()).unwrap();
        e.stop_run(at(120)).unwrap();
        assert!(!e.pump.transport().running());
        assert_eq!(
            e.store().run(id).unwrap().unwrap().status,
            RunStatus::Stopped
        );
        assert!(matches!(e.stop_run(at(130)), Err(EngineError::Idle)));

        let id2 = e.start_run(linear_cfg(), at(200)).unwrap();
        e.abort_run(at(260)).unwrap();
        assert_eq!(
            e.store().run(id2).unwrap().unwrap().status,
            RunStatus::Aborted
        );
    }

    #[test]
    fn clamps_are_intersected_with_pump_limits() {
        let mut e = engine();
        let mut cfg = linear_cfg();
        cfg.curve =
            CurveSpec::linear(0.0, 5000.0, Duration::from_secs(3600)).with_clamp(0.0, 5000.0);
        e.start_run(cfg, t0()).unwrap();
        e.tick(at(3599)).unwrap();
        assert!(e.pump.transport().speed_rpm() as f64 <= limits::RPM_MAX + 1e-3);
    }

    #[test]
    fn ml_min_run_writes_the_flow_register_only() {
        let mut e = engine();
        let cfg = RunConfig {
            control_var: ControlVar::MlMin,
            curve: CurveSpec::linear(10.0, 40.0, Duration::from_secs(3600)),
            ..linear_cfg()
        };
        e.start_run(cfg, t0()).unwrap();
        assert!(e.pump.transport().running());
        // head / tubing registers are never touched
        assert_eq!(e.pump.transport().head_code(), 0);
        assert_eq!(e.pump.transport().tubing_code(), 0);
        assert!((e.pump.transport().flow_ml_min() - 10.0).abs() < 1e-3);
    }

    #[test]
    fn tick_with_no_run_is_idle() {
        let mut e = engine();
        assert!(matches!(e.tick(t0()).unwrap(), TickOutcome::Idle));
    }

    #[test]
    fn quantize_snaps_to_the_grid() {
        assert!((quantize(8.04, 0.1) - 8.0).abs() < 1e-9);
        assert!((quantize(8.06, 0.1) - 8.1).abs() < 1e-9);
        assert!((quantize(10.0083333, 1e-4) - 10.0083).abs() < 1e-9);
        assert_eq!(setpoint_grid(ControlVar::Rpm), 0.1);
        assert_eq!(setpoint_grid(ControlVar::MlMin), 1e-4);
    }

    #[test]
    fn apply_setpoint_writes_on_a_grid_step_and_never_journals() {
        let mut e = engine();
        let mut cfg = linear_cfg();
        // 10 → 370 rpm over 1 h ⇒ 0.1 rpm/s near t0; clamp-intersected to ≤ 350.
        cfg.curve =
            CurveSpec::linear(10.0, 370.0, Duration::from_secs(3600)).with_clamp(0.0, 5000.0);
        let id = e.start_run(cfg, t0()).unwrap();
        let at_ms = |ms: i64| t0() + SignedDuration::from_millis(ms);

        // 400 ms in: still on the 10.0 rpm grid point → no write.
        assert!(!e.apply_setpoint(at_ms(400)).unwrap());
        assert_eq!(e.store().tick_count(id).unwrap(), 0);

        // 6 s in: curve is at 10.6 → crosses the grid, writes the quantised value.
        assert!(e.apply_setpoint(at_ms(6_000)).unwrap());
        assert!((e.pump.transport().speed_rpm() as f64 - 10.6).abs() < 1e-3);

        // same grid point again → no second write.
        assert!(!e.apply_setpoint(at_ms(6_040)).unwrap());

        // journalling stays the 1 s tick's job.
        assert_eq!(e.store().tick_count(id).unwrap(), 0);
        e.tick(at_ms(7_000)).unwrap();
        assert_eq!(e.store().tick_count(id).unwrap(), 1);
    }

    #[test]
    fn apply_setpoint_quantises_ml_min_to_the_display_grid() {
        let mut e = engine();
        let cfg = RunConfig {
            control_var: ControlVar::MlMin,
            // 10 → 46 ml/min over 1 h ⇒ 0.01 ml/min per second, so one 1e-4 grid
            // step every 10 ms.
            curve: CurveSpec::linear(10.0, 46.0, Duration::from_secs(3600)),
            ..linear_cfg()
        };
        e.start_run(cfg, t0()).unwrap();
        // 1235 ms in: raw 10.012350, exactly between two 1e-4 grid points.
        e.apply_setpoint(t0() + SignedDuration::from_millis(1235))
            .unwrap();
        let f = e.pump.transport().flow_ml_min() as f64;
        let on_grid = (f * 10_000.0).round() / 10_000.0;
        assert!((f - on_grid).abs() < 1e-5, "not snapped to the 1e-4 grid: {f}");
    }

    #[test]
    fn apply_setpoint_is_false_with_no_run() {
        let mut e = engine();
        assert!(!e.apply_setpoint(t0()).unwrap());
    }

    // ---- crash recovery ----

    use std::sync::atomic::AtomicU64;

    /// A throwaway on-disk database so a run survives dropping the `Engine`
    /// (an in-memory DB dies with its connection).
    struct TempDb(std::path::PathBuf);

    impl TempDb {
        fn new() -> Self {
            static CTR: AtomicU64 = AtomicU64::new(0);
            let n = CTR.fetch_add(1, Ordering::Relaxed);
            let p = std::env::temp_dir()
                .join(format!("fermentool-rec-{}-{n}.sqlite", std::process::id()));
            let _ = std::fs::remove_file(&p);
            TempDb(p)
        }
        fn store(&self) -> Store {
            Store::open(&self.0).unwrap()
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            for ext in ["sqlite", "sqlite-wal", "sqlite-shm"] {
                let _ = std::fs::remove_file(self.0.with_extension(ext));
            }
        }
    }

    fn grace() -> Duration {
        Duration::from_secs(300)
    }

    #[test]
    fn resumes_a_running_run_after_a_simulated_crash() {
        let db = TempDb::new();
        let id = {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            let id = a.start_run(linear_cfg(), t0()).unwrap();
            a.tick(at(600)).unwrap();
            a.tick(at(1200)).unwrap();
            id // drop engine A -> "crash"
        };

        // Fresh engine + fresh pump (regs zeroed = power-cycled).
        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        assert!(!b.pump.transport().running());

        let info = b.pending_recovery(at(1800), grace()).unwrap().unwrap();
        assert_eq!(info.run_id, id);
        assert!((info.elapsed_s - 1800.0).abs() < 1.0);
        assert!((info.resume_target - 50.0).abs() < 0.5);
        assert!(!info.past_end);
        assert_eq!(info.last_seq, Some(1));

        assert_eq!(b.resume(at(1800), grace()).unwrap(), id);
        assert!(b.pump.transport().running());
        assert!(b.pump.transport().direction_cw());
        assert!((b.pump.transport().speed_rpm() - 50.0).abs() < 0.5);

        // ticking continues from the next seq, timing still absolute
        assert!(matches!(
            b.tick(at(1810)).unwrap(),
            TickOutcome::Applied { seq: 2, .. }
        ));
        assert!(matches!(
            b.tick(at(3600)).unwrap(),
            TickOutcome::Finished { .. }
        ));
        assert_eq!(
            b.store().run(id).unwrap().unwrap().status,
            RunStatus::Completed
        );

        let kinds: Vec<_> = b
            .store()
            .events(Some(id), 50)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(kinds.contains(&"crash_detected".to_string()));
        assert!(kinds.contains(&"resume".to_string()));
    }

    #[test]
    fn no_recovery_without_an_unclean_running_run() {
        let db = TempDb::new();
        // nothing yet
        {
            let b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            assert!(b.pending_recovery(t0(), grace()).unwrap().is_none());
        }
        // a cleanly stopped run does not count
        {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            a.start_run(linear_cfg(), t0()).unwrap();
            a.stop_run(at(60)).unwrap();
        }
        let b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        assert!(b.pending_recovery(at(120), grace()).unwrap().is_none());
    }

    #[test]
    fn pending_recovery_is_none_while_this_process_owns_the_run() {
        let db = TempDb::new();
        let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        a.start_run(linear_cfg(), t0()).unwrap();
        assert!(a.pending_recovery(at(100), grace()).unwrap().is_none());
    }

    #[test]
    fn discard_recovery_ends_the_orphan_run() {
        let db = TempDb::new();
        let id = {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            let id = a.start_run(linear_cfg(), t0()).unwrap();
            a.tick(at(300)).unwrap();
            id
        };

        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        assert!(matches!(
            b.discard_recovery(at(600), RunStatus::Running),
            Err(EngineError::Config(_))
        ));
        b.discard_recovery(at(600), RunStatus::Aborted).unwrap();
        assert!(b.store().running_run().unwrap().is_none());
        assert_eq!(
            b.store().run(id).unwrap().unwrap().status,
            RunStatus::Aborted
        );
        assert!(!b.pump.transport().running());
    }

    #[test]
    fn resume_is_refused_past_the_grace_window() {
        let db = TempDb::new();
        {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            let mut cfg = linear_cfg();
            cfg.curve = CurveSpec::linear(1.0, 9.0, Duration::from_secs(100));
            a.start_run(cfg, t0()).unwrap();
            a.tick(at(10)).unwrap();
        }
        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        let g = Duration::from_secs(60);
        let info = b.pending_recovery(at(1000), g).unwrap().unwrap();
        assert!(info.past_end);
        assert!(matches!(b.resume(at(1000), g), Err(EngineError::Config(_))));
        b.discard_recovery(at(1000), RunStatus::Completed).unwrap();
        assert_eq!(b.store().running_run().unwrap().map(|r| r.status), None);
    }

    #[test]
    fn resume_within_grace_applies_the_end_value_and_completes() {
        let db = TempDb::new();
        {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            let mut cfg = linear_cfg();
            cfg.curve = CurveSpec::linear(1.0, 9.0, Duration::from_secs(100));
            a.start_run(cfg, t0()).unwrap();
            a.tick(at(10)).unwrap();
        }
        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        let g = Duration::from_secs(60);
        let info = b.pending_recovery(at(130), g).unwrap().unwrap();
        assert!(!info.past_end);
        assert!((info.resume_target - 9.0).abs() < 1e-6);
        b.resume(at(130), g).unwrap();
        assert!(matches!(
            b.tick(at(140)).unwrap(),
            TickOutcome::Finished { .. }
        ));
    }

    #[test]
    fn resume_is_busy_when_a_run_is_active() {
        let mut e = engine();
        e.start_run(linear_cfg(), t0()).unwrap();
        assert!(matches!(e.resume(t0(), grace()), Err(EngineError::Busy)));
    }
}
