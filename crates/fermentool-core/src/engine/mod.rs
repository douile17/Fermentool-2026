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

use crate::store::{
    ControlVar, Direction, EventLevel, NewEvent, NewRun, NewTick, RunStatus, Store, StoreError,
};

/// Smallest / largest allowed tick interval.
pub const MIN_TICK_INTERVAL_S: u32 = 1;
pub const MAX_TICK_INTERVAL_S: u32 = 300;

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

/// What the caller supplies to start a run.
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub name: String,
    pub control_var: ControlVar,
    pub direction: Direction,
    pub tick_interval_s: u32,
    pub pump_addr: u8,
    /// Pump-head code, required for `ml_min` runs.
    pub pump_head: Option<u16>,
    /// Tubing-size code, required for `ml_min` runs.
    pub tubing: Option<u16>,
    pub curve: CurveSpec,
}

/// Result of a single [`Engine::tick`].
#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone)]
pub struct EngineStatus {
    pub active: Option<ActiveStatus>,
}

#[derive(Debug, Clone)]
pub struct ActiveStatus {
    pub run_id: i64,
    pub started_at: Timestamp,
    pub duration_s: i64,
    pub control_var: ControlVar,
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
    tick_interval: Duration,
    tick_interval_s: u32,
    next_seq: i64,
    last_target: Option<f64>,
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
                tick_interval_s: a.tick_interval_s,
                last_seq: (a.next_seq > 0).then_some(a.next_seq - 1),
                last_target: a.last_target,
            }),
        }
    }

    /// The active run's tick cadence, if a run is running.
    pub fn tick_interval(&self) -> Option<Duration> {
        self.active.as_ref().map(|a| a.tick_interval)
    }

    /// Start a run at `now`: intersect the curve clamps with the pump limits,
    /// validate, run the pump start sequence, insert the run, begin ticking.
    pub fn start_run(&mut self, cfg: RunConfig, now: Timestamp) -> Result<i64> {
        if self.active.is_some() {
            return Err(EngineError::Busy);
        }
        if !(MIN_TICK_INTERVAL_S..=MAX_TICK_INTERVAL_S).contains(&cfg.tick_interval_s) {
            return Err(EngineError::Config(format!(
                "tick interval must be {MIN_TICK_INTERVAL_S}..={MAX_TICK_INTERVAL_S} s"
            )));
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
        let first = spec.value_at(Duration::ZERO);

        // Pump start sequence (docs/IMPLEMENTATION_PLAN.md §4.3).
        self.pump.set_direction(cfg.direction == Direction::Cw)?;
        if cfg.control_var == ControlVar::MlMin {
            if let Some(code) = cfg.pump_head {
                self.pump.set_head_type(code)?;
            }
            if let Some(code) = cfg.tubing {
                self.pump.set_tubing_size(code)?;
            }
        }
        write_setpoint(&mut self.pump, cfg.control_var, first)?;
        self.pump.start()?;

        let id = self.store.insert_run(&NewRun {
            name: cfg.name.clone(),
            started_at: now,
            control_var: cfg.control_var,
            direction: cfg.direction,
            tick_interval_s: i64::from(cfg.tick_interval_s),
            pump_addr: cfg.pump_addr,
            pump_head: cfg.pump_head.map(|c| c.to_string()),
            tubing: cfg.tubing.map(|c| c.to_string()),
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
            tick_interval: Duration::from_secs(u64::from(cfg.tick_interval_s)),
            tick_interval_s: cfg.tick_interval_s,
            next_seq: 0,
            last_target: Some(first),
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
        let target = active.spec.value_at(Duration::from_secs_f64(elapsed_s));
        let seq = active.next_seq;
        active.next_seq += 1;
        active.last_target = Some(target);

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

/// Drive an engine in real time until the run finishes or `stop` is set. The
/// supervised, panic-catching wrapper comes with the daemon wiring (milestone 7).
pub fn run_blocking<T: Transport>(engine: &mut Engine<T>, stop: &AtomicBool) -> Result<()> {
    while !stop.load(Ordering::Relaxed) {
        match engine.tick(Timestamp::now())? {
            TickOutcome::Idle | TickOutcome::Finished { .. } => break,
            TickOutcome::Applied { .. } => {}
        }
        let interval = engine
            .tick_interval()
            .unwrap_or_else(|| Duration::from_secs(10));
        sleep_interruptible(interval, stop);
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
            tick_interval_s: 10,
            pump_addr: 1,
            pump_head: None,
            tubing: None,
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
    fn bad_tick_interval_is_rejected() {
        let mut e = engine();
        let mut cfg = linear_cfg();
        cfg.tick_interval_s = 0;
        assert!(matches!(
            e.start_run(cfg, t0()),
            Err(EngineError::Config(_))
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
    fn ml_min_run_sets_head_and_tubing() {
        let mut e = engine();
        let cfg = RunConfig {
            control_var: ControlVar::MlMin,
            pump_head: Some(0),
            tubing: Some(16),
            curve: CurveSpec::linear(10.0, 40.0, Duration::from_secs(3600)),
            ..linear_cfg()
        };
        e.start_run(cfg, t0()).unwrap();
        assert!(e.pump.transport().running());
        assert_eq!(e.pump.transport().tubing_code(), 16);
        assert!((e.pump.transport().flow_ml_min() - 10.0).abs() < 1e-3);
    }

    #[test]
    fn tick_with_no_run_is_idle() {
        let mut e = engine();
        assert!(matches!(e.tick(t0()).unwrap(), TickOutcome::Idle));
    }
}
