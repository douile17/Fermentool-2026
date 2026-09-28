//! The control engine: run lifecycle + the tick loop.
//!
//! [`Engine::tick`] does exactly one tick's work, compute the setpoint for the
//! **real elapsed time** (`now - started_at`), write it to the pump, journal it,
//! and finish the run when the curve is done. Time is injected, so a 100 h run
//! is exercised in milliseconds by calling `tick` with synthetic timestamps
//! (`docs/IMPLEMENTATION_PLAN.md` §4.5).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use fermentool_curves::CurveSpec;
use fermentool_modbus::{limits, PduError, Pump, PumpError, PumpTransport, Transport};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::config::{ScaleConfig, SerialConfig};
use crate::scale;
use crate::store::{
    ControlVar, Direction, EventLevel, NewEvent, NewRun, NewTick, RunKind, RunStatus, Store,
    StoreError,
};
use crate::transport::{SwapTransport, TransportKind};
use crate::trim;

/// Fixed **journal** cadence, not user-tunable.
///
/// [`Engine::tick`] fires once a second: it appends a journal row, owns run
/// completion, and is the heartbeat that keeps writing the pump when the curve
/// is flat. 1 s keeps the journal small over a 100 h run.
///
/// The pump setpoint itself is refreshed faster, the control loop also calls
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
    /// A real serial port is configured but not open, starting/resuming a run
    /// would silently drive nothing.
    SerialDown,
    /// The engine is on the pump simulator and simulator runs aren't enabled
    /// (`serial.allow_simulator`), starting/resuming a run would drive nothing.
    SimulatorNotAllowed,
    /// The pump answered a setpoint write with a MODBUS exception (typically
    /// `0x03`, illegal data value): the frame was well-formed, the pump refused
    /// the value. On the LabQ this is a ml/min setpoint above the flow the
    /// configured pump head + tubing can deliver (or with no head/tubing set at
    /// all). The one-line [`Display`](std::fmt::Display) stays terse; the
    /// actionable detail is in [`EngineError::hint`].
    PumpRejectedSetpoint {
        code: u8,
        control_var: ControlVar,
        value: f64,
    },
    /// The run asked for the gravimetric trim but no scale is attached.
    ScaleUnavailable,
    /// A gravimetric-trim run in rpm without a tubing calibration: nothing
    /// turns its rpm curve into the volume the mass balance needs.
    GravimetricTrimNeedsCalibration,
}

/// Structured error for the HTTP API: a one-line `message` (the [`Display`] of
/// an [`EngineError`]), a stable `code` for the UI to switch on, and an
/// optional longer `hint` it can reveal on demand.
///
/// [`Display`]: std::fmt::Display
#[derive(Debug, Clone, Serialize)]
pub struct ErrDetail {
    pub message: String,
    pub code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Pump(e) => write!(f, "pump: {e}"),
            EngineError::Store(e) => write!(f, "store: {e}"),
            EngineError::Config(s) => write!(f, "invalid run config: {s}"),
            EngineError::Busy => write!(f, "a run is already active"),
            EngineError::Idle => write!(f, "no run is active"),
            EngineError::ScaleUnavailable => write!(
                f,
                "gravimetric trim needs a scale, and none is connected"
            ),
            EngineError::GravimetricTrimNeedsCalibration => write!(
                f,
                "gravimetric trim in rpm needs a tubing calibration"
            ),
            EngineError::SerialDown => write!(
                f,
                "the pump link is down. Connect the pump (or set serial.path to \"sim\") before starting a run"
            ),
            EngineError::SimulatorNotAllowed => write!(
                f,
                "running on the pump simulator. Configure a real serial port, or enable simulator runs in Settings, before starting a run"
            ),
            EngineError::PumpRejectedSetpoint {
                code,
                control_var,
                value,
            } => {
                let unit = match control_var {
                    ControlVar::Rpm => "rpm",
                    ControlVar::MlMin => "ml/min",
                };
                write!(
                    f,
                    "the pump refused the {value:.3} {unit} setpoint (MODBUS 0x{code:02X}: {})",
                    fermentool_modbus::exception_name(*code)
                )
            }
        }
    }
}

impl EngineError {
    /// Machine-stable identifier for this error kind (the API `code` field).
    pub fn code(&self) -> &'static str {
        match self {
            EngineError::Pump(_) => "pump_error",
            EngineError::Store(_) => "store_error",
            EngineError::Config(_) => "invalid_config",
            EngineError::Busy => "busy",
            EngineError::Idle => "idle",
            EngineError::SerialDown => "serial_down",
            EngineError::SimulatorNotAllowed => "simulator_not_allowed",
            EngineError::PumpRejectedSetpoint { .. } => "pump_setpoint_rejected",
            EngineError::ScaleUnavailable => "scale_unavailable",
            EngineError::GravimetricTrimNeedsCalibration => "trim_needs_calibration",
        }
    }

    /// A longer, operator-facing explanation, for the errors where the one-line
    /// message is not enough to act on. `None` when the message already says
    /// everything.
    pub fn hint(&self) -> Option<String> {
        let text = match self {
            EngineError::PumpRejectedSetpoint { control_var, .. } => match control_var {
                ControlVar::MlMin => {
                    "That flow is outside the range the LabQ's configured pump head and tubing can \
                     deliver. Read the maximum flow on the pump's own display, then lower the \
                     target, fit larger tubing, or run this cycle in rpm. (The pump also rejects \
                     every ml/min write when it has no head or tubing configured at all.)"
                }
                ControlVar::Rpm => {
                    "That speed is outside the range the pump accepts. Use a value within the \
                     pump's rated rpm."
                }
            },
            EngineError::SerialDown => {
                "Fermentool drives the pump over MODBUS on the configured serial port. Plug in the \
                 USB-RS485 adapter and pick its port in the connection bar, or set the port to \
                 \"sim\" to run against the built-in simulator."
            }
            EngineError::ScaleUnavailable => {
                "The trim reads a balance under the feed bottle to correct the pump. Set the \
                 balance's serial port under [scale] in config.toml and check its cable (a \
                 configured scale that is unplugged is retried in the background), or start \
                 this run without the gravimetric trim."
            }
            EngineError::GravimetricTrimNeedsCalibration => {
                "The trim compares the mass that left the bottle with the volume the curve asked \
                 for. In rpm there is no volume behind the curve until this tube has been \
                 calibrated. Calibrate it (Tubing calibration), pick that calibration for the run, \
                 or run in ml/min, where a calibration is optional."
            }
            EngineError::SimulatorNotAllowed => {
                "The engine is on the in-process simulator, so a run would move nothing real. \
                 Connect a pump and select its port, or tick \"Allow runs on the pump simulator\" \
                 in Settings for bench testing."
            }
            _ => return None,
        };
        Some(text.to_string())
    }

    /// The full structured form for the HTTP API.
    pub fn detail(&self) -> ErrDetail {
        ErrDetail {
            message: self.to_string(),
            code: self.code(),
            hint: self.hint(),
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
    /// Opt-in per run: multiply the curve's setpoint by a slowly-adapting
    /// factor learned from a scale under the feed bottle. `false` (the
    /// default) is byte-for-byte today's open-loop behavior.
    #[serde(default)]
    pub gravimetric_trim: bool,
    /// `calibration` for a tubing-calibration burst, kept out of the dosing
    /// history. Defaults to `dosing`.
    #[serde(default)]
    pub kind: RunKind,
    /// The tubing calibration to start the trim from (its `c0`, or in rpm
    /// mode its rpm-to-volume conversion). Required for a trimmed rpm run.
    #[serde(default)]
    pub tubing_calibration_id: Option<i64>,
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
    /// The setpoint the curve prescribes for `elapsed_s` right now, where a
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
    /// `"sim"` or the open serial port name.
    pub transport: String,
    /// Set when a run completed and the pump is still holding its final
    /// setpoint (it is not stopped on natural completion). Cleared by
    /// [`Engine::stop_pump`] or by starting another run.
    pub holding: Option<HoldingStatus>,
    /// `false` once the serial link to the pump is lost (the configured port
    /// won't reopen). The daemon keeps retrying; the UI should raise an alarm.
    pub serial_ok: bool,
    /// Consecutive failed pump writes since the last success, a non-zero value
    /// means the link is degrading even if not yet fully lost.
    pub write_fails: u32,
    /// `false` once the pump stops matching the commanded setpoint on readback
    /// (a stall / fault the writes alone don't reveal).
    pub pump_confirmed: bool,
    /// `false` once tick journalling has failed several times in a row (disk
    /// full / unwritable). The pump keeps being driven, this is a data-loss
    /// warning, not a control fault.
    pub journal_ok: bool,
    /// The engine is driving the in-process simulator, not a real serial port.
    pub simulator: bool,
    /// `serial.allow_simulator`, simulator runs are explicitly enabled, so a
    /// run may start even on the simulator.
    pub allow_simulator: bool,
    /// `false` once the scale link is lost or the trim has alarmed. `true`
    /// (the safe "no problem" default) when no scale is configured at all.
    pub scale_ok: bool,
    /// `None` when no scale is configured.
    pub scale_state: Option<trim::ScaleState>,
    /// `None` when no scale is configured.
    pub trim_c: Option<f64>,
    /// Diagnostic instantaneous rate from the Theil-Sen estimator, `None`
    /// until at least two samples have been buffered since the last refill.
    pub rate_g_per_min: Option<f64>,
}

/// A completed run whose final setpoint the pump is still holding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldingStatus {
    pub run_id: i64,
    pub name: String,
    pub control_var: ControlVar,
    /// The setpoint the pump is holding (curve's final, quantised value).
    pub value: f64,
    pub finished_at: Timestamp,
    /// Commanded volume delivered over the run's planned duration, mL, frozen
    /// at the moment the curve completed. `Some` only for `ControlVar::MlMin`.
    /// The pump keeps running at `value` after this, the UI adds
    /// `value * (now - finished_at)` on top to show a running total, no
    /// further engine-side accounting needed since the rate is constant.
    #[serde(default)]
    pub volume_added_ml: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActiveStatus {
    pub run_id: i64,
    pub started_at: Timestamp,
    pub duration_s: i64,
    pub control_var: ControlVar,
    /// Always [`TICK_INTERVAL`] in seconds, kept in the payload for display.
    pub tick_interval_s: u32,
    pub last_seq: Option<i64>,
    pub last_target: Option<f64>,
    /// Commanded volume delivered so far, mL. `Some` only for
    /// `ControlVar::MlMin`. An open-loop estimate (trapezoid integral of the
    /// setpoint the daemon wrote), not a measured flow.
    pub volume_added_ml: Option<f64>,
    /// Whether this run opted into the gravimetric trim, so the control loop
    /// knows to schedule `scale_tick` without reaching into `Engine` internals.
    pub gravimetric_trim: bool,
    /// `calibration` for a tubing-calibration burst.
    pub kind: RunKind,
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
    /// `elapsed_s` of the last sample folded into `volume_added_ml`, the left
    /// edge of the next trapezoid slice.
    vol_elapsed_s: f64,
    /// The target value at `vol_elapsed_s`, the left edge of the next
    /// trapezoid slice. Deliberately separate from `last_target`: that field
    /// is also written by `Engine::apply_setpoint` every ~150 ms between
    /// journal ticks, so reusing it here would silently pull in whatever
    /// setpoint was last written just before the *next* tick rather than the
    /// one that was true at `vol_elapsed_s`, inflating every slice of a
    /// moving curve (not just the last one).
    vol_target: f64,
    /// Commanded volume delivered so far, mL: a trapezoid integral of the
    /// applied setpoint over elapsed time. `Some` only for `ControlVar::MlMin`
    /// (a `Rpm` run has no head/tubing calibration to turn speed into volume,
    /// see the rpm-only convention). This is what the daemon *told* the pump
    /// to do, not a measured flow, there is no flow meter in this loop, see
    /// `docs/superpowers/specs/2026-09-11-gravimetric-feed-trim-design.md`.
    volume_added_ml: Option<f64>,
    /// Copied from `RunConfig` at `start_run`/`resume` time. Persisted on the
    /// `runs` row (`gravimetric_trim` column) precisely so `resume()` can
    /// restore it: without a durable copy, a `trim_c` that survived a crash
    /// via `app_state` (see `save_trim_state`) would silently stop being
    /// applied because `scale_tick` gates on this flag.
    gravimetric_trim: bool,
    kind: RunKind,
}

/// Trapezoid slice of a commanded-volume integral between two
/// `(elapsed_s, target_ml_min)` samples, in mL.
fn volume_slice(prev: (f64, f64), next: (f64, f64)) -> f64 {
    let dt_min = (next.0 - prev.0).max(0.0) / 60.0;
    (prev.1 + next.1) / 2.0 * dt_min
}

/// Owns the pump and the journal for the lifetime of the process.
pub struct Engine<T: Transport> {
    pump: Pump<T>,
    store: Store,
    app_version: String,
    active: Option<ActiveRun>,
    /// Which transport the pump is currently driving, for the status frame.
    transport: TransportKind,
    /// A completed run the pump is still holding at its final setpoint.
    holding: Option<HoldingStatus>,
    /// The configured serial link, kept so the daemon can reopen it on its own
    /// after a mid-run cable/adapter glitch.
    serial: SerialConfig,
    pump_addr: u8,
    /// Consecutive failed pump writes since the last success.
    write_fails: u32,
    /// The real port was configured but is not currently open.
    serial_lost: bool,
    /// `serial.allow_simulator`, simulator runs are explicitly enabled. When
    /// false, [`start_run`](Self::start_run) / [`resume`](Self::resume) refuse
    /// while the engine is on the simulator instead of driving nothing.
    allow_simulator: bool,
    /// Consecutive readback checks where the pump did not match the setpoint.
    readback_fails: u32,
    /// `false` once the pump has stopped tracking the commanded setpoint for
    /// several consecutive readbacks (a fault / stall the writes don't reveal).
    pump_confirmed: bool,
    /// Consecutive failed `append_tick` writes (disk full / unwritable). The
    /// pump stays driven; this only feeds the `journal_ok` status flag.
    journal_fails: u32,
    /// The balance link, `None` when `[scale]` is unconfigured. Boxed rather
    /// than generic over a second `Transport` type: the pump and the scale
    /// are independent links, unrelated to `Engine<T>`'s `T`.
    scale: Option<Box<dyn Transport + Send>>,
    /// The `[scale]` config `scale` was attached from, kept so
    /// [`recover_scale`](Self::recover_scale) can reopen the same port later.
    scale_cfg: ScaleConfig,
    /// Dimensionless correction multiplied onto every curve setpoint:
    /// `target = curve.value_at(elapsed) * trim_c`. `1.0` is a no-op, the
    /// value while `gravimetric_trim` is off (or unset) on the active run.
    trim_c: f64,
    scale_state: trim::ScaleState,
    scale_read_fails: u32,
    /// `false` once the trim update alarms (`trim::TrimOutcome::Alarm`).
    /// Sticky until the next `start_run`. A down link is tracked separately
    /// by `scale_read_fails` (see [`scale_link_down`](Self::scale_link_down));
    /// the status frame's `scale_ok` combines both. `trim_c` freezes either
    /// way and the pump keeps running on it.
    scale_ok: bool,
    /// The scale reading and moment `scale_state` last entered `Normal`
    /// (after a refill settles, or at run start): the reference point the
    /// cumulative mass balance measures from.
    refill_weight_g: Option<f64>,
    refill_at: Option<Timestamp>,
    /// `(seconds_since_refill, weight_g)` samples since `refill_at`, feeding
    /// the Theil-Sen rate estimate. Cleared on every new refill reference.
    weight_buffer: Vec<(f64, f64)>,
    last_weight_g: Option<f64>,
    settling_since: Option<Timestamp>,
    manual_refill_mode: bool,
    /// One-shot: consumed (reset to `false`) by the next `scale_tick`.
    manual_refill_done_flag: bool,
    /// Diagnostic only (not fed back into `trim_c`, the cumulative mass
    /// balance drives the correction): the instantaneous measured rate.
    last_rate_g_per_min: Option<f64>,
    scale_density_g_per_ml: f64,
    /// Set from a tubing calibration (Task 12) for `control_var: Rpm` runs:
    /// mL/min delivered per commanded rpm, so `integrate_curve_mass` can
    /// convert the curve's rpm output into a volumetric rate. `None` (the
    /// default, and always for `ControlVar::MlMin`, where the curve is
    /// already volumetric) is treated as `1.0`, a no-op factor.
    rpm_to_ml_min: Option<f64>,
}

/// Cap on `weight_buffer`'s length: a gravimetric run with no refill appends
/// one point per second, so over 100 h that vector would grow without bound.
/// Past the cap it is decimated back down to `trim::MAX_SLOPE_POINTS`.
const WEIGHT_BUFFER_CAP: usize = trim::MAX_SLOPE_POINTS * 4;

/// Consecutive failed writes after which the control loop reopens the port.
pub const REOPEN_AFTER_WRITE_FAILS: u32 = 5;

/// Read the pump back and compare it to the setpoint every this many journal
/// ticks (≈ every 10 s), one extra MODBUS read.
const READBACK_EVERY_TICKS: i64 = 10;

/// Consecutive readback mismatches before the pump is flagged as not tracking
/// (≈ 30 s of sustained deviation, so a ramp's motor lag doesn't trip it).
const READBACK_MISMATCH_LIMIT: u32 = 3;

/// Consecutive failed `append_tick` writes before the journal is flagged stalled
/// (disk full / permissions). The pump keeps being driven; the UI raises it.
const JOURNAL_STALL_LIMIT: u32 = 5;

/// `app_state` key holding the JSON of the current [`HoldingStatus`], so a
/// completed-run hold survives a daemon restart (the pump is still physically
/// running its final setpoint). Empty value = nothing held.
const HOLDING_KEY: &str = "holding";

/// `app_state` key holding the JSON of the current [`PersistedTrim`], so a
/// gravimetric trim that converged over many hours doesn't reset to `1.0` on
/// a restart.
const TRIM_STATE_KEY: &str = "trim_state";

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedTrim {
    trim_c: f64,
    scale_state: trim::ScaleState,
    refill_weight_g: Option<f64>,
    refill_at: Option<Timestamp>,
}

impl<T: Transport> Engine<T> {
    pub fn new(pump: Pump<T>, store: Store, app_version: impl Into<String>) -> Self {
        // A run that completed naturally leaves the pump running at its final
        // setpoint; if the daemon restarts before the operator stops it, restore
        // the hold so the UI still offers a Stop button instead of showing
        // "idle" while the pump runs.
        let holding = store
            .get_state(HOLDING_KEY)
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
            .and_then(|s| serde_json::from_str::<HoldingStatus>(&s).ok());
        let persisted_trim = store
            .get_state(TRIM_STATE_KEY)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<PersistedTrim>(&s).ok());
        let (trim_c, scale_state, refill_weight_g, refill_at) = match persisted_trim {
            Some(p) => (p.trim_c, p.scale_state, p.refill_weight_g, p.refill_at),
            None => (1.0, trim::ScaleState::Normal, None, None),
        };
        Self {
            pump,
            store,
            app_version: app_version.into(),
            active: None,
            transport: TransportKind::Sim,
            holding,
            serial: SerialConfig {
                path: "sim".into(),
                baud: 9600,
                allow_simulator: false,
            },
            pump_addr: 0,
            write_fails: 0,
            serial_lost: false,
            // Permissive until `set_serial` applies the real config, matching
            // the `serial: "sim"` placeholder above, the daemon always calls
            // `set_serial` at boot, so this only affects bare in-process
            // engines (tests, embedding).
            allow_simulator: true,
            readback_fails: 0,
            pump_confirmed: true,
            journal_fails: 0,
            scale: None,
            scale_cfg: ScaleConfig::default(),
            trim_c,
            scale_state,
            scale_read_fails: 0,
            scale_ok: true,
            refill_weight_g,
            refill_at,
            weight_buffer: Vec::new(),
            last_weight_g: None,
            settling_since: None,
            manual_refill_mode: false,
            manual_refill_done_flag: false,
            last_rate_g_per_min: None,
            scale_density_g_per_ml: 1.0,
            rpm_to_ml_min: None,
        }
    }

    /// Set (or clear) the completed-run hold, persisting it to `app_state` so it
    /// survives a restart. Every `self.holding` transition goes through here.
    fn set_holding(&mut self, holding: Option<HoldingStatus>) {
        let json = holding
            .as_ref()
            .and_then(|h| serde_json::to_string(h).ok())
            .unwrap_or_default();
        let _ = self.store.set_state(HOLDING_KEY, &json);
        self.holding = holding;
    }

    /// Persist `trim_c`/`scale_state`/the refill reference to `app_state`, the
    /// same mechanism `set_holding` uses, so a converged trim doesn't reset to
    /// `1.0` on a restart. Cheap enough to call from `scale_tick` on every
    /// actual change (at most once per adaptive window in steady state).
    fn save_trim_state(&self) {
        let p = PersistedTrim {
            trim_c: self.trim_c,
            scale_state: self.scale_state,
            refill_weight_g: self.refill_weight_g,
            refill_at: self.refill_at,
        };
        if let Ok(json) = serde_json::to_string(&p) {
            let _ = self.store.set_state(TRIM_STATE_KEY, &json);
        }
    }

    /// Record the serial link the daemon should reopen on its own after a
    /// failure. If a real port is configured but the pump is currently on the
    /// simulator (it wouldn't open at boot), mark the link lost so recovery
    /// starts retrying immediately.
    pub fn set_serial(&mut self, serial: SerialConfig, pump_addr: u8) {
        let lost = !serial.use_simulator() && self.transport == TransportKind::Sim;
        self.allow_simulator = serial.allow_simulator;
        self.serial = serial;
        self.pump_addr = pump_addr;
        self.serial_lost = lost;
        if lost {
            let _ = self.store.log_event(&NewEvent {
                run_id: None,
                wall_time: Timestamp::now(),
                level: EventLevel::Error,
                kind: "serial_lost".into(),
                detail: Some(format!("{} not open at startup; retrying", self.serial.path)),
            });
        }
    }

    /// Consecutive failed pump writes since the last success.
    pub fn write_fails(&self) -> u32 {
        self.write_fails
    }

    /// Apply a changed `serial.allow_simulator` from a live config save.
    pub fn set_allow_simulator(&mut self, allow: bool) {
        self.allow_simulator = allow;
    }

    /// The configured real port is not currently open.
    pub fn serial_lost(&self) -> bool {
        self.serial_lost
    }

    /// `false` once the pump stopped tracking the setpoint (readback mismatch).
    pub fn pump_confirmed(&self) -> bool {
        self.pump_confirmed
    }

    /// A real serial port is configured (not the simulator), the control loop
    /// keeps the link alive with a periodic probe even when no run is active.
    pub fn serial_is_real(&self) -> bool {
        !self.serial.use_simulator()
    }

    /// The pump is currently driven by the in-process simulator, not a real port.
    fn on_simulator(&self) -> bool {
        matches!(self.transport, TransportKind::Sim)
    }

    /// Poll the pump to keep the link's health current while idle / holding.
    /// A failed read counts toward the same streak as a failed write, so the
    /// loop's recovery kicks in whether or not a run is active.
    pub fn probe_link(&mut self) -> bool {
        match self.pump.read_speed_rpm() {
            Ok(_) => {
                self.write_fails = 0;
                true
            }
            Err(_) => {
                self.write_fails = self.write_fails.saturating_add(1);
                false
            }
        }
    }

    /// Record which transport this engine is driving (called at boot once the
    /// real port has been opened).
    pub fn set_transport_kind(&mut self, kind: TransportKind) {
        self.transport = kind;
    }

    /// The transport the pump is currently driving.
    pub fn transport_kind(&self) -> &TransportKind {
        &self.transport
    }

    /// Rebuild the pump transport in place from `serial` (simulator ⇄ real
    /// port). Refused while a run is active, stop the run first. Returns the
    /// transport now in use (which may be the simulator if the port failed to
    /// open).
    pub fn swap_transport(
        &mut self,
        serial: &SerialConfig,
        pump_addr: u8,
    ) -> Result<TransportKind>
    where
        T: SwapTransport,
    {
        if self.active.is_some() {
            return Err(EngineError::Busy);
        }
        let kind = self.pump.transport_mut().swap(serial, pump_addr);
        self.pump.set_address(pump_addr);
        self.transport = kind.clone();
        self.allow_simulator = serial.allow_simulator;
        self.write_fails = 0;
        self.serial = serial.clone();
        self.pump_addr = pump_addr;

        let now = Timestamp::now();
        if serial.use_simulator() {
            // Intentional switch to the simulator, nothing to confirm.
            self.serial_lost = false;
        } else if kind == TransportKind::Sim {
            // Asked for a real port but `swap` fell back to the simulator: the
            // port did not open. Keep it flagged so the alarm stays up and the
            // auto-recovery loop keeps retrying.
            self.serial_lost = true;
            let _ = self.store.log_event(&NewEvent {
                run_id: None,
                wall_time: now,
                level: EventLevel::Error,
                kind: "serial_lost".into(),
                detail: Some(format!(
                    "{} did not open on reconnect; retrying automatically",
                    serial.path
                )),
            });
        } else {
            // The port opened, but only a live MODBUS read proves a pump is
            // actually on the other end. `confirm_link` sets `serial_lost` and
            // logs the outcome; leave the *previous* `serial_lost` in place so
            // it can see the link was lost and journal `serial_recovered`.
            self.confirm_link(now);
        }
        Ok(kind)
    }

    /// Automatic serial recovery: reopen the configured port in place. Unlike
    /// [`swap_transport`](Self::swap_transport) this is allowed while a run is
    /// active (a mid-run adapter glitch is exactly when it's needed) and does
    /// **not** fall back to the simulator, a failed reopen leaves a no-op sink
    /// so the pump keeps its last real setpoint and the caller retries.
    /// Returns `true` when the real port is (back) open.
    pub fn recover_serial(&mut self, now: Timestamp) -> bool
    where
        T: SwapTransport,
    {
        if self.serial.use_simulator() {
            return true; // nothing to recover
        }
        let run_id = self.active.as_ref().map(|a| a.id);
        match self
            .pump
            .transport_mut()
            .swap_strict(&self.serial, self.pump_addr)
        {
            Some(kind) => {
                self.transport = kind;
                // The port (re)opened, but "open" is not "connected". On
                // Windows a USB-RS485 COM port opens whether or not a pump is
                // powered/wired at the other end. Only a real MODBUS round-trip
                // clears the alarm; otherwise keep retrying so a port that opens
                // into the void can't masquerade as a healthy link. Two reads,
                // not one: this path also runs mid-run on a noisy-bus write-fail
                // streak, where a single missed read shouldn't flip the alarm
                // from "degrading" (amber) to "not connected" (red).
                if self.confirm_read() || self.confirm_read() {
                    let was_lost = self.serial_lost;
                    self.serial_lost = false;
                    self.write_fails = 0;
                    if was_lost {
                        let _ = self.store.log_event(&NewEvent {
                            run_id,
                            wall_time: now,
                            level: EventLevel::Info,
                            kind: "serial_recovered".into(),
                            detail: Some(format!("reopened {}; pump responding", self.serial.path)),
                        });
                    }
                    true
                } else {
                    if !self.serial_lost {
                        self.serial_lost = true;
                        let _ = self.store.log_event(&NewEvent {
                            run_id,
                            wall_time: now,
                            level: EventLevel::Error,
                            kind: "serial_lost".into(),
                            detail: Some(format!(
                                "{} opened but the pump is not responding; retrying",
                                self.serial.path
                            )),
                        });
                    }
                    false
                }
            }
            None => {
                if !self.serial_lost {
                    self.serial_lost = true;
                    let _ = self.store.log_event(&NewEvent {
                        run_id,
                        wall_time: now,
                        level: EventLevel::Error,
                        kind: "serial_lost".into(),
                        detail: Some(format!(
                            "{} will not open; pump holding last setpoint, retrying",
                            self.serial.path
                        )),
                    });
                }
                false
            }
        }
    }

    /// One MODBUS read, used purely as a "is the pump actually there?" probe.
    /// No state, no logging, the callers own that. Returns `false` without
    /// touching the bus when the engine is on the simulator fallback (a
    /// successful read of the no-op `SimPump` would prove nothing about the
    /// real port).
    fn confirm_read(&mut self) -> bool {
        if matches!(self.transport, TransportKind::Sim) && !self.serial.use_simulator() {
            return false;
        }
        self.pump.read_speed_rpm().is_ok()
    }

    /// Confirm the configured real port with a live MODBUS read: an open port is
    /// not a connected pump. Called at boot (after the port is opened) and on an
    /// operator reconnect. On the simulator it is a no-op that reports healthy.
    ///
    /// Success clears `serial_lost` (logging `serial_recovered` if it flips);
    /// failure latches `serial_lost` so runs are refused and the control loop's
    /// auto-recovery keeps retrying.
    pub fn confirm_link(&mut self, now: Timestamp) -> bool {
        if self.serial.use_simulator() {
            return true;
        }
        let was_lost = self.serial_lost;
        if self.confirm_read() {
            self.serial_lost = false;
            self.write_fails = 0;
            if was_lost {
                let _ = self.store.log_event(&NewEvent {
                    run_id: self.active.as_ref().map(|a| a.id),
                    wall_time: now,
                    level: EventLevel::Info,
                    kind: "serial_recovered".into(),
                    detail: Some(format!("{}: pump responding", self.serial.path)),
                });
            }
            true
        } else {
            self.serial_lost = true;
            if !was_lost {
                let _ = self.store.log_event(&NewEvent {
                    run_id: self.active.as_ref().map(|a| a.id),
                    wall_time: now,
                    level: EventLevel::Error,
                    kind: "serial_lost".into(),
                    detail: Some(format!(
                        "{}: no response from the pump (check power, wiring, MODBUS address, baud)",
                        self.serial.path
                    )),
                });
            }
            false
        }
    }

    /// Read access to the journal (for the API / diagnostics).
    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn status(&self) -> EngineStatus {
        // A configured scale is reported even while it is down, so the UI can
        // show it as down instead of hiding the indicator.
        let scale_shown = self.scale.is_some() || self.scale_wanted();
        EngineStatus {
            active: self.active.as_ref().map(|a| ActiveStatus {
                run_id: a.id,
                started_at: a.started_at,
                duration_s: a.duration_s,
                control_var: a.control_var,
                tick_interval_s: TICK_INTERVAL.as_secs() as u32,
                last_seq: (a.next_seq > 0).then_some(a.next_seq - 1),
                last_target: a.last_target,
                volume_added_ml: a.volume_added_ml,
                gravimetric_trim: a.gravimetric_trim,
                kind: a.kind,
            }),
            transport: self.transport.label(),
            holding: self.holding.clone(),
            serial_ok: !self.serial_lost,
            write_fails: self.write_fails,
            pump_confirmed: self.pump_confirmed,
            journal_ok: self.journal_fails < JOURNAL_STALL_LIMIT,
            simulator: self.on_simulator(),
            allow_simulator: self.allow_simulator,
            scale_ok: !scale_shown || (self.scale_ok && !self.scale_link_down()),
            scale_state: scale_shown.then_some(self.scale_state),
            trim_c: scale_shown.then_some(self.trim_c),
            rate_g_per_min: scale_shown.then_some(self.last_rate_g_per_min).flatten(),
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
        // Refuse to start against a dead link: a real port is configured but the
        // engine is on the no-op simulator fallback, so every pump write would
        // "succeed" while the pump does nothing.
        if self.serial_lost {
            return Err(EngineError::SerialDown);
        }
        // Same hazard when the engine is deliberately on the simulator but
        // simulator runs weren't enabled, don't advance a cycle that drives
        // nothing just because `config.toml` still has the shipped `path = "sim"`.
        if self.on_simulator() && !self.allow_simulator {
            return Err(EngineError::SimulatorNotAllowed);
        }

        // The chosen tubing calibration, checked before anything changes.
        let calibration = match cfg.tubing_calibration_id {
            Some(cal_id) => {
                let cal = self.store.calibration(cal_id)?.ok_or_else(|| {
                    EngineError::Config(format!("tubing calibration {cal_id} does not exist"))
                })?;
                if cal.control_var != cfg.control_var {
                    return Err(EngineError::Config(format!(
                        "tubing calibration {cal_id} was made in {}, this run is in {}",
                        cal.control_var.as_str(),
                        cfg.control_var.as_str()
                    )));
                }
                Some(cal)
            }
            None => None,
        };
        // In rpm the mass balance has no volume to compare against without
        // a calibration; in ml/min the curve is already volumetric.
        if cfg.gravimetric_trim && cfg.control_var == ControlVar::Rpm && calibration.is_none() {
            return Err(EngineError::GravimetricTrimNeedsCalibration);
        }
        // Without a scale nothing would ever read the balance or correct c,
        // and the UI would show no trim indicator: refuse rather than run a
        // trim that silently does nothing.
        if cfg.gravimetric_trim && self.scale.is_none() {
            return Err(EngineError::ScaleUnavailable);
        }

        // A fresh run gets a fresh gravimetric-trim window: the previous run's
        // refill baseline is meaningless against this run's own `started_at`,
        // and reusing it would make the first cumulative check compare this
        // run's theoretical mass against a stale measured one, tripping a
        // spurious alarm. `trim_c` restarts at 1.0 too: a per-tube starting
        // value comes from a tubing calibration, not from whatever the last
        // run happened to learn.
        self.trim_c = 1.0;
        self.scale_state = trim::ScaleState::Normal;
        self.refill_weight_g = None;
        self.refill_at = None;
        self.weight_buffer.clear();
        self.last_weight_g = None;
        self.settling_since = None;
        self.scale_read_fails = 0;
        self.scale_ok = true;
        self.manual_refill_mode = false;
        self.manual_refill_done_flag = false;
        self.last_rate_g_per_min = None;
        self.rpm_to_ml_min = None;
        if let Some(cal) = &calibration {
            match cfg.control_var {
                // Dimensionless: seeds the trim, within its usual bounds.
                ControlVar::MlMin => self.trim_c = cal.c0.clamp(trim::TRIM_MIN, trim::TRIM_MAX),
                // rpm per (ml/min), not a trim factor: at the calibrated
                // setpoint the conversion is exact, so c starts at 1.0.
                ControlVar::Rpm => self.rpm_to_ml_min = Some(calibration_ml_per_rpm(cal)),
            }
        }
        // Persist the fresh state now: a crash before anything else changes
        // would otherwise restore the previous run's c and baseline.
        self.save_trim_state();

        // A new run supersedes any completed-run hold.
        self.set_holding(None);

        let (lo, hi) = match cfg.control_var {
            ControlVar::Rpm => (limits::RPM_MIN, limits::RPM_MAX),
            ControlVar::MlMin => (limits::FLOW_MIN, limits::FLOW_MAX),
        };
        let mut spec = cfg.curve.clone();
        spec.clamp_min = spec.clamp_min.max(lo);
        spec.clamp_max = spec.clamp_max.min(hi);
        spec.validate().map_err(EngineError::Config)?;

        let duration_s = spec.duration.as_secs() as i64;
        let c = if cfg.gravimetric_trim { self.trim_c } else { 1.0 };
        let first = quantize(spec.value_at(Duration::ZERO) * c, setpoint_grid(cfg.control_var));

        // Pump start sequence (docs/IMPLEMENTATION_PLAN.md §4.3). The pump's
        // head-type / tubing-size registers are deliberately left untouched, see
        // the note in migrations/0001_init.sql.
        self.pump.set_direction(cfg.direction == Direction::Cw)?;
        write_setpoint(&mut self.pump, cfg.control_var, first)
            .map_err(|e| setpoint_error(e, cfg.control_var, first))?;
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
            gravimetric_trim: cfg.gravimetric_trim,
            kind: cfg.kind,
            tubing_calibration_id: cfg.tubing_calibration_id,
        })?;
        self.store.log_event(&NewEvent {
            run_id: Some(id),
            wall_time: now,
            level: EventLevel::Info,
            kind: "start".into(),
            detail: Some(format!("first setpoint {first:.3}")),
        })?;

        self.readback_fails = 0;
        self.pump_confirmed = true;
        self.active = Some(ActiveRun {
            id,
            started_at: now,
            spec,
            control_var: cfg.control_var,
            duration_s,
            next_seq: 0,
            last_target: Some(first),
            last_write_q: Some(first),
            vol_elapsed_s: 0.0,
            vol_target: first,
            volume_added_ml: (cfg.control_var == ControlVar::MlMin).then_some(0.0),
            gravimetric_trim: cfg.gravimetric_trim,
            kind: cfg.kind,
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
        let run_kind = active.kind;
        let elapsed_s = now.duration_since(active.started_at).as_secs_f64().max(0.0);
        let c = if active.gravimetric_trim { self.trim_c } else { 1.0 };
        let raw = active.spec.value_at(Duration::from_secs_f64(elapsed_s)) * c;
        let target = quantize(raw, setpoint_grid(control_var));
        let seq = active.next_seq;
        active.next_seq += 1;
        if let Some(vol) = active.volume_added_ml {
            // The finishing tick can land a little past `duration_s` (the
            // control loop hits its 1s deadline "at or after", never exactly
            // on it); `target` is already clamped to the curve's end value by
            // `value_at`, but without clamping this slice's time bound too,
            // that overshoot gets counted as extra time at the final rate,
            // inflating the frozen run-end total by a few tenths of a mL.
            let te = elapsed_s.min(duration_s as f64);
            active.volume_added_ml =
                Some(vol + volume_slice((active.vol_elapsed_s, active.vol_target), (te, target)));
        }
        active.vol_elapsed_s = elapsed_s;
        active.vol_target = target;
        active.last_target = Some(target);
        active.last_write_q = Some(target);
        let volume_added_ml = active.volume_added_ml;

        let write = write_setpoint(&mut self.pump, control_var, target);
        let written_ok = write.is_ok();
        self.write_fails = if written_ok {
            0
        } else {
            self.write_fails.saturating_add(1)
        };
        if let Err(e) = &write {
            // Diagnostics only, a failed journal write must never abort the
            // tick's pump-control / completion logic below.
            let _ = self.store.log_event(&NewEvent {
                run_id: Some(id),
                wall_time: now,
                level: EventLevel::Warn,
                kind: "write_fail".into(),
                detail: Some(e.to_string()),
            });
        }

        // Every Nth tick, read the pump back and confirm it is on the setpoint.
        // The pump reaches a new speed in well under the sub-second write
        // cadence, so by readback time it should match `target`; a sustained
        // gap means a stall / fault the writes alone don't reveal.
        let readback: Option<f64> = if written_ok && seq > 0 && seq % READBACK_EVERY_TICKS == 0 {
            match read_actual(&mut self.pump, control_var) {
                Ok(actual) => {
                    let actual = actual as f64;
                    let tol = (5.0 * setpoint_grid(control_var)).max(0.04 * target.abs());
                    if (actual - target).abs() > tol {
                        self.readback_fails = self.readback_fails.saturating_add(1);
                        if self.readback_fails >= READBACK_MISMATCH_LIMIT && self.pump_confirmed {
                            self.pump_confirmed = false;
                            let _ = self.store.log_event(&NewEvent {
                                run_id: Some(id),
                                wall_time: now,
                                level: EventLevel::Error,
                                kind: "readback_mismatch".into(),
                                detail: Some(format!(
                                    "commanded {target:.3}, pump reports {actual:.3}"
                                )),
                            });
                        }
                    } else {
                        self.readback_fails = 0;
                        if !self.pump_confirmed {
                            self.pump_confirmed = true;
                            let _ = self.store.log_event(&NewEvent {
                                run_id: Some(id),
                                wall_time: now,
                                level: EventLevel::Info,
                                kind: "readback_ok".into(),
                                detail: Some(format!("pump back on setpoint at {actual:.3}")),
                            });
                        }
                    }
                    Some(actual)
                }
                Err(e) => {
                    let _ = self.store.log_event(&NewEvent {
                        run_id: Some(id),
                        wall_time: now,
                        level: EventLevel::Warn,
                        kind: "readback_fail".into(),
                        detail: Some(e.to_string()),
                    });
                    None
                }
            }
        } else {
            None
        };

        // The pump was already commanded above. A journal write that fails
        // (disk full / unwritable) must not stop the run, crash-resume works
        // off the immutable `started_at`, so a gap in the journal is survivable.
        // Track the streak so the UI can raise `journal_ok = false`.
        match self.store.append_tick(&NewTick {
            run_id: id,
            seq,
            wall_time: now,
            elapsed_s,
            target,
            written_ok,
            readback,
            note: None,
        }) {
            Ok(_) => self.journal_fails = 0,
            Err(e) => {
                self.journal_fails = self.journal_fails.saturating_add(1);
                tracing::warn!("append_tick failed ({e}); run continues, journal stalled");
                if self.journal_fails == JOURNAL_STALL_LIMIT {
                    let _ = self.store.log_event(&NewEvent {
                        run_id: Some(id),
                        wall_time: now,
                        level: EventLevel::Error,
                        kind: "journal_stalled".into(),
                        detail: Some(format!("{JOURNAL_STALL_LIMIT} consecutive tick writes failed: {e}")),
                    });
                }
            }
        }

        if elapsed_s >= duration_s as f64 {
            // The pump is NOT stopped on natural completion, it keeps running
            // at the curve's final setpoint (written just above) until the user
            // stops it via `stop_pump`.
            if let Err(e) = self.store.finish_run(id, RunStatus::Completed, now) {
                // Couldn't record completion (disk full / unwritable). The pump
                // is already holding the final setpoint, don't wedge the engine
                // or drop the completion: keep the run active and retry on the
                // next tick. The journal-stall streak surfaces the disk problem
                // through `journal_ok`.
                self.journal_fails = self.journal_fails.saturating_add(1);
                if self.journal_fails == JOURNAL_STALL_LIMIT {
                    let _ = self.store.log_event(&NewEvent {
                        run_id: Some(id),
                        wall_time: now,
                        level: EventLevel::Error,
                        kind: "journal_stalled".into(),
                        detail: Some(format!(
                            "{JOURNAL_STALL_LIMIT} consecutive journal writes failed: {e}"
                        )),
                    });
                }
                tracing::warn!("finish_run failed at completion ({e}); retrying next tick");
                return Ok(TickOutcome::Applied {
                    seq,
                    target,
                    written_ok,
                });
            }
            self.journal_fails = 0;
            if run_kind == RunKind::Calibration {
                // A burst is weighed against its own started_at..ended_at, so
                // the pump stops here instead of holding: any flow after
                // ended_at would land in the bottle with no time to match it.
                let _ = self.pump.stop();
                let _ = self.store.log_event(&NewEvent {
                    run_id: Some(id),
                    wall_time: now,
                    level: EventLevel::Info,
                    kind: "curve_done".into(),
                    detail: Some("calibration burst done, pump stopped".into()),
                });
                self.active = None;
                return Ok(TickOutcome::Finished { seq, target });
            }
            let _ = self.store.log_event(&NewEvent {
                run_id: Some(id),
                wall_time: now,
                level: EventLevel::Info,
                kind: "curve_done".into(),
                detail: Some(format!("pump holding at {target:.3}")),
            });
            let name = self
                .store
                .run(id)
                .ok()
                .flatten()
                .map(|r| r.name)
                .unwrap_or_default();
            self.set_holding(Some(HoldingStatus {
                run_id: id,
                name,
                control_var,
                value: target,
                finished_at: now,
                volume_added_ml,
            }));
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
    /// completion check, [`Engine::tick`] owns those. Returns `true` when a new
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
        let c = if active.gravimetric_trim { self.trim_c } else { 1.0 };
        let raw = active.spec.value_at(Duration::from_secs_f64(elapsed_s)) * c;
        let target = quantize(raw, setpoint_grid(control_var));
        if active.last_write_q == Some(target) {
            return Ok(false);
        }

        match write_setpoint(&mut self.pump, control_var, target) {
            Ok(()) => {
                active.last_write_q = Some(target);
                active.last_target = Some(target);
                self.write_fails = 0;
                Ok(true)
            }
            Err(e) => {
                // The next journal tick will journal the write state; here just
                // note it. Keep the run going, a transient bus error recovers.
                self.write_fails = self.write_fails.saturating_add(1);
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

    /// Read the scale (if configured and the active run opted in), advance the
    /// perturbation/refill state machine, and, while `Normal`, update `trim_c`
    /// from the cumulative mass balance. A no-op whenever there's no scale, no
    /// active run, or the active run has `gravimetric_trim = false`.
    pub fn scale_tick(&mut self, now: Timestamp) {
        let wants_trim = self.active.as_ref().is_some_and(|a| a.gravimetric_trim);
        if !wants_trim {
            return;
        }
        // Once the link is known down, stop asking: each failed read can cost
        // up to the watchdog's timeout on the shared control thread, which
        // would starve the pump's 150 ms setpoint cadence. `recover_scale`,
        // on its own backoff, owns bringing the link back.
        if self.scale_link_down() {
            return;
        }
        let Some(scale) = self.scale.as_mut() else {
            return;
        };
        let reply = scale
            .transaction(scale::SICS_IMMEDIATE)
            .map_err(scale::ScaleError::from)
            .and_then(|r| scale::parse_sics_weight(&r));

        let weight_g = match reply {
            Ok((w, _stable)) => {
                // `scale_ok` is sticky: a good read clears the read-failure
                // streak but must not erase an alarm the trim itself raised
                // before the operator has seen it. Only `start_run` clears it.
                self.scale_read_fails = 0;
                w
            }
            Err(_) => {
                self.scale_read_fails = self.scale_read_fails.saturating_add(1);
                return;
            }
        };

        // Snapshot so the (at most once per call) persistence write only
        // happens when `trim_c`, `scale_state` or the baseline actually move.
        let before = (self.trim_c, self.scale_state, self.refill_weight_g);

        let prev = self.last_weight_g.unwrap_or(weight_g);
        self.last_weight_g = Some(weight_g);
        let seconds_in_settling = self
            .settling_since
            .map(|s| now.duration_since(s).as_secs_f64())
            .unwrap_or(0.0);
        let input = trim::StateInput {
            weight_g,
            prev_weight_g: prev,
            // v1 simplification: no rolling variance of the settling window
            // yet, REFILL_SETTLE_SECONDS alone gates RefillSettling -> Normal.
            // Functionally safe (a bottle still being handled just takes the
            // fixed delay instead of also requiring low variance first),
            // slightly less adaptive. A follow-up computing this from
            // `weight_buffer`'s tail is a one-function addition.
            recent_variance_g: 0.0,
            seconds_in_settling,
            manual_refill_mode: self.manual_refill_mode,
            manual_refill_done: self.manual_refill_done_flag,
        };
        self.manual_refill_done_flag = false;
        let next = trim::next_state(self.scale_state, &input);

        if next == trim::ScaleState::RefillSettling && self.scale_state != trim::ScaleState::RefillSettling {
            self.settling_since = Some(now);
        }
        // Only a real refill completing resets the cumulative reference. A
        // 5..=50 g bump (Perturbation -> Normal) is transient: freeze, don't
        // reset, or a routine touch of the bottle would throw away hours of
        // accumulated balance.
        if next == trim::ScaleState::Normal && self.scale_state == trim::ScaleState::RefillSettling {
            self.refill_weight_g = Some(weight_g);
            self.refill_at = Some(now);
            self.weight_buffer.clear();
            // A manual refill request is one-shot: close it here even if the
            // operator never calls refill_done, or the next tick would go
            // straight back to RefillPending and cycle forever.
            self.manual_refill_mode = false;
        }
        self.scale_state = next;

        if self.scale_state != trim::ScaleState::Normal {
            if (self.trim_c, self.scale_state, self.refill_weight_g) != before {
                self.save_trim_state();
            }
            return;
        }

        if self.refill_weight_g.is_none() {
            // First read in Normal with no baseline: a brand new run never
            // crosses RefillSettling -> Normal, so seed the cumulative check
            // here or it would stay dead until the first refill cycle.
            self.refill_weight_g = Some(weight_g);
            self.refill_at = Some(now);
        }

        let t_refill = self.refill_at.unwrap_or(now);
        let elapsed_since_refill = now.duration_since(t_refill).as_secs_f64().max(0.0);
        self.weight_buffer.push((elapsed_since_refill, weight_g));
        if self.weight_buffer.len() > WEIGHT_BUFFER_CAP {
            self.weight_buffer = trim::decimate(&self.weight_buffer, trim::MAX_SLOPE_POINTS);
        }
        self.last_rate_g_per_min = trim::theil_sen_slope(&trim::decimate(
            &self.weight_buffer,
            trim::MAX_SLOPE_POINTS,
        ))
        .map(|s| s * 60.0);

        if let (Some(w0), Some(active)) = (self.refill_weight_g, self.active.as_ref()) {
            let density = self.scale_density_g_per_ml;
            let elapsed = now.duration_since(active.started_at).as_secs_f64().max(0.0);
            let refill_elapsed = now.duration_since(t_refill).as_secs_f64().max(0.0);
            let start_elapsed = (elapsed - refill_elapsed).max(0.0);
            let ml_per_unit = self.rpm_to_ml_min.unwrap_or(1.0);
            // Deliberately NOT scaled by `trim_c`: the theoretical side is what
            // the raw curve asks for, the measured side is what the pump
            // (running at curve * c) really delivered, so error_frac = 1 - k*c
            // and the loop converges on c = 1/k. Scaling both by c would
            // cancel it out and ratchet c to its clamp.
            let mass_theoretical =
                integrate_curve_mass(&active.spec, start_elapsed, elapsed, density, ml_per_unit);
            let mass_measured = w0 - weight_g;
            if mass_theoretical.abs() > 1e-6 {
                let error_frac = (mass_theoretical - mass_measured) / mass_theoretical;
                match trim::update_trim(self.trim_c, error_frac) {
                    trim::TrimOutcome::Unchanged => {}
                    trim::TrimOutcome::Trimmed { new_c } => self.trim_c = new_c,
                    trim::TrimOutcome::Alarm => self.scale_ok = false,
                }
            }
        }

        if (self.trim_c, self.scale_state, self.refill_weight_g) != before {
            self.save_trim_state();
        }
    }

    /// Operator-declared "I'm about to change the bottle": forces the state
    /// machine into `RefillPending` on the next `scale_tick`, ahead of the
    /// automatic weight-jump threshold.
    pub fn trigger_refill_mode(&mut self) {
        self.manual_refill_mode = true;
    }

    /// Operator-declared "bottle is back, settled": lets `RefillPending`
    /// advance to `RefillSettling` without waiting on the variance threshold.
    /// One-shot, consumed by the next `scale_tick`.
    pub fn trigger_refill_done(&mut self) {
        self.manual_refill_mode = false;
        self.manual_refill_done_flag = true;
    }

    /// Attach the scale link the boot sequence built (`None`: no `[scale]`
    /// path, or a configured scale that did not open), and remember its
    /// config: the feed density for the mass balance, and the port for
    /// [`recover_scale`](Self::recover_scale) to retry.
    pub fn attach_scale(&mut self, scale: Option<Box<dyn Transport + Send>>, cfg: ScaleConfig) {
        self.scale = scale;
        self.scale_density_g_per_ml = cfg.density_g_per_ml;
        self.scale_cfg = cfg;
    }

    /// Whether `[scale]` names a port at all, attached or not. Tells "a
    /// configured scale that is down" (retry it) from "no balance" (nothing
    /// to do, ever).
    pub fn scale_wanted(&self) -> bool {
        self.scale_cfg.configured()
    }

    /// Whether the scale *link* needs reopening: never attached, or its read
    /// failures crossed [`REOPEN_AFTER_WRITE_FAILS`]. Deliberately distinct
    /// from a trim alarm on a reachable scale, which recovery must leave alone.
    pub fn scale_link_down(&self) -> bool {
        self.scale.is_none() || self.scale_read_fails >= REOPEN_AFTER_WRITE_FAILS
    }

    /// The scale's counterpart to [`recover_serial`](Self::recover_serial):
    /// reopen the configured port from scratch. No simulator fallback, a
    /// failed attempt leaves `self.scale` as it was and returns `false` so the
    /// caller retries on its own backoff. Returns `true` without doing
    /// anything when no scale is configured or the link is healthy.
    pub fn recover_scale(&mut self) -> bool {
        if !self.scale_wanted() || !self.scale_link_down() {
            return true;
        }
        let Some(watchdog) = scale::open_watchdogged(&self.scale_cfg) else {
            return false;
        };
        self.scale = Some(Box::new(watchdog));
        self.scale_read_fails = 0;
        true
    }

    #[cfg(test)]
    pub(crate) fn attach_scale_for_test(&mut self, t: Box<dyn Transport + Send>) {
        self.scale = Some(t);
    }

    #[cfg(test)]
    pub(crate) fn trim_c(&self) -> f64 {
        self.trim_c
    }

    #[cfg(test)]
    pub(crate) fn set_trim_c_for_test(&mut self, c: f64) {
        self.trim_c = c;
    }

    #[cfg(test)]
    pub(crate) fn weight_buffer_len(&self) -> usize {
        self.weight_buffer.len()
    }

    /// Graceful stop: stop the pump, mark the run `stopped`.
    pub fn stop_run(&mut self, now: Timestamp) -> Result<()> {
        self.end_run(now, RunStatus::Stopped, "stop")
    }

    /// Abort: stop the pump, mark the run `aborted`.
    pub fn abort_run(&mut self, now: Timestamp) -> Result<()> {
        self.end_run(now, RunStatus::Aborted, "abort")
    }

    /// Stop a pump that is still holding a completed run's final setpoint.
    /// `Err(Idle)` when nothing is being held (no run has completed, or the
    /// hold was already released).
    pub fn stop_pump(&mut self, now: Timestamp) -> Result<()> {
        let Some(h) = self.holding.clone() else {
            return Err(EngineError::Idle);
        };
        self.set_holding(None);
        let _ = self.pump.stop();
        self.store.log_event(&NewEvent {
            run_id: Some(h.run_id),
            wall_time: now,
            level: EventLevel::Info,
            kind: "pump_stop".into(),
            detail: Some("held setpoint released".into()),
        })?;
        Ok(())
    }

    fn end_run(&mut self, now: Timestamp, status: RunStatus, kind: &str) -> Result<()> {
        let Some(active) = self.active.take() else {
            return Err(EngineError::Idle);
        };
        self.set_holding(None);
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
    /// stopped. Read-only, safe to call repeatedly (e.g. from a polling UI).
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
        if self.serial_lost {
            return Err(EngineError::SerialDown);
        }
        if self.on_simulator() && !self.allow_simulator {
            return Err(EngineError::SimulatorNotAllowed);
        }
        let Some(run) = self.store.running_run()? else {
            return Err(EngineError::Idle);
        };
        self.set_holding(None);
        let elapsed_s = now.duration_since(run.started_at).as_secs_f64().max(0.0);
        if elapsed_s > run.duration_s as f64 + grace.as_secs_f64() {
            return Err(EngineError::Config(
                "run is past its end plus grace; finish or abort instead".into(),
            ));
        }
        // `trim_c` was restored from `app_state` by `Engine::new`; the rpm
        // conversion is not persisted there, rebuild it from the run's own
        // calibration. A calibration deleted since is no reason to refuse a
        // crash resume: the pump keeps its curve, the trim alarms and freezes.
        self.rpm_to_ml_min = None;
        if run.control_var == ControlVar::Rpm {
            if let Some(cal_id) = run.tubing_calibration_id {
                self.rpm_to_ml_min = self.store.calibration(cal_id)?.map(|c| calibration_ml_per_rpm(&c));
            }
        }
        // `trim_c` only applies if this run opted in, same gate as
        // `tick`/`apply_setpoint`.
        let c = if run.gravimetric_trim { self.trim_c } else { 1.0 };
        let target = quantize(
            run.curve.value_at(Duration::from_secs_f64(elapsed_s)) * c,
            setpoint_grid(run.control_var),
        );

        self.log_crash_detected(run.id, now, elapsed_s, run.duration_s)?;

        self.pump.set_direction(run.direction == Direction::Cw)?;
        write_setpoint(&mut self.pump, run.control_var, target)
            .map_err(|e| setpoint_error(e, run.control_var, target))?;
        self.pump.start()?;

        let next_seq = self.store.last_tick(run.id)?.map_or(0, |t| t.seq + 1);
        let volume_added_ml = if run.control_var == ControlVar::MlMin {
            let start = quantize(run.curve.value_at(Duration::ZERO), setpoint_grid(run.control_var));
            Some(self.reconstruct_volume_ml(run.id, start, run.duration_s, elapsed_s, target)?)
        } else {
            None
        };
        self.store.log_event(&NewEvent {
            run_id: Some(run.id),
            wall_time: now,
            level: EventLevel::Info,
            kind: "resume".into(),
            detail: Some(format!(
                "elapsed {elapsed_s:.0}s, setpoint {target:.3}, seq from {next_seq}"
            )),
        })?;

        self.readback_fails = 0;
        self.pump_confirmed = true;
        self.active = Some(ActiveRun {
            id: run.id,
            started_at: run.started_at,
            spec: run.curve,
            control_var: run.control_var,
            duration_s: run.duration_s,
            next_seq,
            last_target: Some(target),
            last_write_q: Some(target),
            vol_elapsed_s: elapsed_s,
            vol_target: target,
            volume_added_ml,
            gravimetric_trim: run.gravimetric_trim,
            kind: run.kind,
        });
        Ok(run.id)
    }

    /// Reconstruct `volume_added_ml` after a crash: the in-memory accumulator
    /// from before the restart is gone, but every tick's `(elapsed_s, target)`
    /// is already journalled, trapezoid-integrate straight from those rows
    /// (seeded at `(0, start)`), then fold in the gap between the last
    /// journalled tick and this resume moment (`resume_elapsed_s`,
    /// `resume_target`), the pump kept running at whatever it was last told
    /// for however long the daemon was down.
    fn reconstruct_volume_ml(
        &self,
        run_id: i64,
        start: f64,
        duration_s: i64,
        resume_elapsed_s: f64,
        resume_target: f64,
    ) -> Result<f64> {
        // Clamp every time bound to `duration_s`, same reasoning as `tick`'s
        // finishing-slice clamp: a resume can land minutes into the grace
        // window past the curve's end, and a persisted tick can itself carry
        // a touch of scheduling overshoot, neither should count as extra time
        // at the (already curve-clamped) final rate.
        let d = duration_s as f64;
        let ticks = self.store.ticks(run_id, 0, i64::MAX)?;
        let mut total = 0.0;
        let mut prev = (0.0, start);
        for t in &ticks {
            let te = t.elapsed_s.min(d);
            total += volume_slice(prev, (te, t.target));
            prev = (te, t.target);
        }
        total += volume_slice(prev, (resume_elapsed_s.min(d), resume_target));
        Ok(total)
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

/// Classify a failed setpoint write. Only MODBUS `0x03` (illegal *data value*)
/// means the pump received the frame and refused the *value*, the operator
/// actionable case (see [`EngineError::PumpRejectedSetpoint`]). `0x02` (illegal
/// data *address*) is a register-map / firmware mismatch, not something the
/// operator fixes by changing the setpoint, so it stays a plain
/// [`EngineError::Pump`] along with every other failure.
fn setpoint_error(e: PumpError, control_var: ControlVar, value: f64) -> EngineError {
    match &e {
        PumpError::Pdu(PduError::Exception(0x03)) => EngineError::PumpRejectedSetpoint {
            code: 0x03,
            control_var,
            value,
        },
        _ => EngineError::Pump(e),
    }
}

/// Read the pump's current value for the run's control variable.
fn read_actual<T: Transport>(
    pump: &mut Pump<T>,
    control_var: ControlVar,
) -> std::result::Result<f32, PumpError> {
    match control_var {
        ControlVar::Rpm => pump.read_speed_rpm(),
        ControlVar::MlMin => pump.read_flow_ml_min(),
    }
}

/// The pump's usable setpoint step for a control variable.
///
/// `Rpm` snaps to the documented 0.1 rpm motor step. `MlMin` snaps to the pump's
/// 3-decimal display resolution, its own firmware then collapses that onto a
/// motor step, since Fermentool does not own the tube calibration and cannot do
/// that conversion itself (see `docs/DESIGN.md`).
fn setpoint_grid(control_var: ControlVar) -> f64 {
    match control_var {
        ControlVar::Rpm => 0.1,
        ControlVar::MlMin => 1e-3,
    }
}

/// ml/min delivered per commanded rpm, from an rpm-mode tubing calibration.
fn calibration_ml_per_rpm(cal: &crate::store::CalibrationRow) -> f64 {
    cal.mean_measured_ml_min / cal.setpoint
}

/// Snap `value` to the nearest multiple of `step`.
fn quantize(value: f64, step: f64) -> f64 {
    (value / step).round() * step
}

/// Theoretical mass delivered over `[start_s, end_s]` (elapsed seconds since
/// run start), grams: Simpson's rule over `spec.value_at(t) * ml_per_unit *
/// density / 60` (the curve is per minute, `t` is in seconds), never scaled by
/// `trim_c`: it is what the raw curve asks for. Pure,
/// no I/O; `ml_per_unit` converts the curve's own unit into mL/min first
/// (`1.0` when the curve is already ml/min).
fn integrate_curve_mass(
    spec: &CurveSpec,
    start_s: f64,
    end_s: f64,
    density_g_per_ml: f64,
    ml_per_unit: f64,
) -> f64 {
    const STEPS: usize = 50;
    let span = (end_s - start_s).max(0.0);
    if span <= 0.0 {
        return 0.0;
    }
    let h = span / STEPS as f64;
    let rate = |t_s: f64| -> f64 {
        spec.value_at(Duration::from_secs_f64(t_s)) * ml_per_unit * density_g_per_ml / 60.0
    };
    let mut sum = rate(start_s) + rate(end_s);
    for i in 1..STEPS {
        let t = start_s + h * i as f64;
        sum += rate(t) * if i % 2 == 0 { 2.0 } else { 4.0 };
    }
    sum * h / 3.0
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
            gravimetric_trim: false,
            kind: RunKind::Dosing,
            tubing_calibration_id: None,
        }
    }

    struct ScriptedScale {
        replies: std::collections::VecDeque<std::result::Result<Vec<u8>, fermentool_modbus::TransportError>>,
    }
    impl ScriptedScale {
        fn new(weights: &[f64]) -> Self {
            let replies = weights
                .iter()
                .map(|w| Ok(format!("S S {w:>10.1} g\r\n").into_bytes()))
                .collect();
            Self { replies }
        }
    }
    impl Transport for ScriptedScale {
        fn transaction(
            &mut self,
            _req: &[u8],
        ) -> std::result::Result<Vec<u8>, fermentool_modbus::TransportError> {
            self.replies
                .pop_front()
                .unwrap_or(Err(fermentool_modbus::TransportError::Timeout))
        }
    }

    #[test]
    fn gravimetric_trim_multiplies_the_curve_target() {
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[1000.0; 50])));
        let cfg = trim_cfg();
        e.start_run(cfg, t0()).unwrap();
        e.scale_tick(t0());
        let o = e.tick(at(1)).unwrap();
        assert!(matches!(o, TickOutcome::Applied { .. }));
        assert_eq!(e.trim_c(), 1.0);
    }

    #[test]
    fn scale_failure_freezes_c_and_keeps_the_pump_running() {
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale { replies: Default::default() }));
        let cfg = trim_cfg();
        e.start_run(cfg, t0()).unwrap();
        for i in 0..10 {
            e.scale_tick(at(i));
        }
        assert!(!e.status().scale_ok);
        let o = e.tick(at(20)).unwrap();
        assert!(matches!(o, TickOutcome::Applied { .. }));
    }

    #[test]
    fn trim_state_survives_a_restart() {
        let db = TempDb::new();
        {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            a.attach_scale_for_test(Box::new(ScriptedScale::new(&[1000.0])));
            let cfg = trim_cfg();
            a.start_run(cfg, t0()).unwrap();
            a.scale_tick(t0());
            a.set_trim_c_for_test(1.05);
            a.save_trim_state();
        }
        let b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        assert_eq!(b.trim_c(), 1.05);
    }

    /// A trimmed run in ml/min, the mode where the mass balance needs no
    /// tubing calibration. A scale is attached by each test that uses it.
    fn trim_cfg() -> RunConfig {
        RunConfig {
            control_var: ControlVar::MlMin,
            gravimetric_trim: true,
            ..linear_cfg()
        }
    }

    /// A flat 60 ml/min curve: at density 1.0 g/mL that is exactly 1.000 g/s of
    /// theoretical mass delivery, which makes the mass balance easy to reason
    /// about by hand.
    fn flat_60_ml_min_cfg() -> RunConfig {
        RunConfig {
            curve: CurveSpec::linear(60.0, 60.0, Duration::from_secs(36_000)),
            ..trim_cfg()
        }
    }

    /// Drive `ticks` one-second `scale_tick`s against a bottle that drains at
    /// `k` times whatever the pump is actually commanded, i.e. a pump whose
    /// real delivered/commanded ratio is `k`. Each reading is computed from the
    /// engine's own `trim_c` as of the previous tick, so this exercises the
    /// real closed loop (`scale_tick` -> `integrate_curve_mass` ->
    /// `update_trim`) rather than a pre-scripted sequence. Returns the highest
    /// `trim_c` seen along the way.
    fn drain_at_ratio(e: &mut Engine<SimPump>, k: f64, start_g: f64, ticks: i64) -> f64 {
        let mut w = start_g;
        let mut peak_c = e.trim_c();
        for i in 0..ticks {
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            // Over the next second the pump runs at curve * c = 60 * c ml/min
            // and really delivers k times that, i.e. c * k grams (density 1.0).
            w -= e.trim_c() * k;
            peak_c = peak_c.max(e.trim_c());
        }
        peak_c
    }

    #[test]
    fn trim_c_converges_toward_the_pumps_real_delivery_ratio() {
        // Commanded a flat 60 ml/min (1 g/s at density 1.0) but really
        // delivering k = 0.85 of that: c must walk toward 1/k and hold there,
        // not ratchet to TRIM_MAX (the bug when the theoretical side was also
        // multiplied by c, which cancelled c out of error_frac).
        const K: f64 = 0.85;
        let target = 1.0 / K;
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        e.start_run(flat_60_ml_min_cfg(), t0()).unwrap();
        assert_eq!(e.trim_c(), 1.0, "c must start un-trimmed");

        let peak_c = drain_at_ratio(&mut e, K, 4_000.0, 600);
        let c = e.trim_c();
        assert!((c - target).abs() < 0.02, "c did not converge: {c}, target {target}");
        let residual = 1.0 - K * c;
        assert!(
            residual.abs() <= trim::TRIM_IGNORE_BAND,
            "residual mass-balance error too large: c = {c}, residual {residual}"
        );
        // The cumulative window lags c right after the baseline, so c
        // overshoots on the way up; what matters is that it comes back.
        assert!(c < trim::TRIM_MAX - 1e-6, "c ended pinned at the clamp: {c}");
        assert!(peak_c - c > 0.01, "c never came back off its overshoot (peak {peak_c}, final {c})");
        assert!(e.status().scale_ok, "a converging loop must not alarm");
    }

    #[test]
    fn a_correctly_delivering_pump_leaves_c_alone() {
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        e.start_run(flat_60_ml_min_cfg(), t0()).unwrap();
        drain_at_ratio(&mut e, 1.0, 4_000.0, 120);
        assert!((e.trim_c() - 1.0).abs() < 1e-9, "c drifted with a perfect pump: {}", e.trim_c());
        assert!(e.status().scale_ok);
    }

    #[test]
    fn the_first_normal_read_seeds_the_mass_balance_baseline() {
        // A brand new run never crosses RefillSettling -> Normal, so without
        // seeding on the first read the cumulative check would stay dead
        // until the operator ran a full refill cycle.
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[1000.0, 1000.0])));
        e.start_run(flat_60_ml_min_cfg(), t0()).unwrap();
        e.scale_tick(t0());
        // Nothing left the bottle while the curve commanded ~1 g: alarms.
        e.scale_tick(at(1));
        assert!(!e.status().scale_ok, "the mass balance never ran");
    }

    #[test]
    fn a_trim_alarm_is_not_erased_by_the_next_healthy_read() {
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[1000.0, 1000.0])));
        e.start_run(flat_60_ml_min_cfg(), t0()).unwrap();
        e.scale_tick(t0());
        e.scale_tick(at(1));
        assert!(!e.status().scale_ok, "a gross mass-balance error must alarm");
        let c_at_alarm = e.trim_c();

        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[998.0])));
        e.scale_tick(at(2));
        assert!(!e.status().scale_ok, "scale_ok must be sticky until the next start_run");
        assert_eq!(e.trim_c(), c_at_alarm, "c stays frozen through the alarm");

        e.stop_run(at(3)).unwrap();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[1000.0; 4])));
        e.start_run(flat_60_ml_min_cfg(), at(4)).unwrap();
        assert!(e.status().scale_ok);
    }

    #[test]
    fn the_weight_buffer_stays_bounded_over_a_long_run() {
        // A run with no refill for hours would otherwise append one point per
        // second for the life of the run.
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        e.start_run(flat_60_ml_min_cfg(), t0()).unwrap();
        drain_at_ratio(&mut e, 1.0, 20_000.0, 3_000);
        assert!(
            e.weight_buffer_len() <= WEIGHT_BUFFER_CAP,
            "weight buffer grew unbounded: {}",
            e.weight_buffer_len()
        );
    }

    #[test]
    fn a_manual_run_ignores_a_learned_trim() {
        // gravimetric_trim = false must be byte-for-byte the plain curve at
        // every write site, whatever trim_c holds.
        let mut e = engine();
        let cfg = RunConfig {
            curve: CurveSpec::linear(100.0, 100.0, Duration::from_secs(3600)),
            ..linear_cfg()
        };
        e.start_run(cfg, t0()).unwrap();
        e.set_trim_c_for_test(1.20);
        e.tick(at(10)).unwrap();
        assert!((e.pump.transport().speed_rpm() as f64 - 100.0).abs() < 1e-3);
        e.apply_setpoint(at(20)).unwrap();
        assert!((e.pump.transport().speed_rpm() as f64 - 100.0).abs() < 1e-3);
    }

    #[test]
    fn a_gravimetric_run_does_apply_the_learned_trim() {
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let cfg = RunConfig {
            curve: CurveSpec::linear(50.0, 50.0, Duration::from_secs(3600)),
            ..trim_cfg()
        };
        e.start_run(cfg, t0()).unwrap();
        e.set_trim_c_for_test(1.10);
        e.tick(at(10)).unwrap();
        assert!((e.pump.transport().flow_ml_min() as f64 - 55.0).abs() < 1e-3);
    }

    #[test]
    fn a_new_run_starts_from_a_clean_trim_state() {
        // Run A: a stable read, then a big jump that lands the state machine
        // in RefillPending, and a learned c.
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[1000.0, 1000.0, 1700.0])));
        e.start_run(trim_cfg(), t0()).unwrap();
        e.scale_tick(t0());
        e.scale_tick(at(1));
        e.scale_tick(at(2)); // +700 g jump -> RefillPending
        assert_eq!(e.status().scale_state, Some(trim::ScaleState::RefillPending));
        e.set_trim_c_for_test(1.15);
        e.stop_run(at(3)).unwrap();

        // Run B must not inherit A's refill baseline, mid-cycle state or c.
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[500.0; 10])));
        e.start_run(trim_cfg(), at(100)).unwrap();
        assert_eq!(e.status().scale_state, Some(trim::ScaleState::Normal));
        assert!(e.status().scale_ok);
        assert_eq!(e.trim_c(), 1.0);
        assert_eq!(e.weight_buffer_len(), 0);
    }

    #[test]
    fn a_crash_early_in_a_new_run_does_not_restore_the_previous_runs_trim() {
        // Run A learns c = 1.15 and persists it. Run B starts fresh (c = 1.0,
        // no baseline) and the daemon dies before anything in B changes c:
        // the restart must come back with B's state, not A's.
        let db = TempDb::new();
        {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            a.attach_scale_for_test(Box::new(ScriptedScale::new(&[1000.0])));
            a.start_run(trim_cfg(), t0()).unwrap();
            a.scale_tick(t0());
            a.set_trim_c_for_test(1.15);
            a.save_trim_state();
            a.stop_run(at(10)).unwrap();
            a.attach_scale_for_test(Box::new(ScriptedScale::new(&[800.0])));
            a.start_run(trim_cfg(), at(20)).unwrap();
            a.scale_tick(at(20)); // seeds B's baseline at 800 g, c unchanged
        }
        let b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        assert_eq!(b.trim_c(), 1.0, "restored the previous run's c");
        assert_eq!(b.refill_weight_g, Some(800.0), "lost the new run's baseline");
    }

    #[test]
    fn a_resumed_trimmed_run_applies_the_persisted_trim() {
        let db = TempDb::new();
        let cfg = RunConfig {
            curve: CurveSpec::linear(50.0, 50.0, Duration::from_secs(3600)),
            ..trim_cfg()
        };
        {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            a.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
            a.start_run(cfg, t0()).unwrap();
            a.set_trim_c_for_test(1.10);
            a.save_trim_state();
        }
        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        b.resume(at(600), grace()).unwrap();
        assert!((b.pump.transport().flow_ml_min() as f64 - 55.0).abs() < 1e-3);
    }

    fn scale_cfg(path: &str) -> ScaleConfig {
        ScaleConfig {
            path: path.into(),
            baud: 9600,
            density_g_per_ml: 1.0,
        }
    }

    #[test]
    fn a_routine_perturbation_does_not_reset_the_cumulative_baseline() {
        // "Transient: freeze, don't reset": a 5..=50 g bump that settles back
        // one tick later must not wipe the buffer or the baseline, only a real
        // RefillSettling -> Normal completion may.
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[
            1000.0, 999.0, 998.0, // 3 stable readings, buffer at 3
            1020.0, // +22 g -> Perturbation, buffer untouched
            1022.0, // settles -> Normal, buffer resumes at 4
        ])));
        e.start_run(trim_cfg(), t0()).unwrap();
        for i in 0..5 {
            e.scale_tick(at(i));
        }
        assert_eq!(e.status().scale_state, Some(trim::ScaleState::Normal));
        assert_eq!(e.weight_buffer_len(), 4);
    }

    #[test]
    fn manual_refill_mode_does_not_cycle_forever_if_refill_done_is_never_called() {
        // An operator who triggers refill mode and never calls refill_done
        // must still end up in Normal, not cycle Normal -> RefillPending ->
        // RefillSettling -> Normal forever, wiping the baseline every lap.
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[1000.0; 60])));
        e.start_run(trim_cfg(), t0()).unwrap();
        e.trigger_refill_mode();
        // The buggy cycle has a 12-tick period and happens to sit in Normal
        // at some ticks, so check every tick once it should have settled.
        for i in 0..60 {
            e.scale_tick(at(i));
            if i >= 20 {
                assert_eq!(
                    e.status().scale_state,
                    Some(trim::ScaleState::Normal),
                    "still cycling at tick {i}"
                );
            }
        }
    }

    #[test]
    fn start_run_refuses_gravimetric_trim_without_an_attached_scale() {
        let mut e = engine(); // no scale attached
        let err = e.start_run(trim_cfg(), t0()).unwrap_err();
        assert!(matches!(err, EngineError::ScaleUnavailable), "got {err:?}");
        assert!(err.hint().is_some());
        assert!(e.status().active.is_none());
    }

    #[test]
    fn a_configured_scale_that_did_not_open_reports_not_ok() {
        let mut e = engine();
        e.attach_scale(None, scale_cfg("NOPE_NOT_A_REAL_PORT_99999"));
        let st = e.status();
        assert!(!st.scale_ok, "a configured but absent scale must not look healthy");
        assert!(st.scale_state.is_some());
    }

    #[test]
    fn attach_scale_uses_the_configured_density() {
        // Density 2.0 g/mL: a flat 60 ml/min curve is 2 g/s, so a bottle
        // losing exactly 2 g/s must leave c alone and not alarm.
        let mut e = engine();
        e.attach_scale(
            Some(Box::new(ScriptedScale::new(&[1000.0, 998.0, 996.0]))),
            ScaleConfig { density_g_per_ml: 2.0, ..scale_cfg("sim-scale") },
        );
        e.start_run(flat_60_ml_min_cfg(), t0()).unwrap();
        for i in 0..3 {
            e.scale_tick(at(i));
        }
        assert!(e.status().scale_ok);
        assert_eq!(e.trim_c(), 1.0);
    }

    /// Always errors, and counts how many times it was asked to.
    struct CountingFailingScale {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Transport for CountingFailingScale {
        fn transaction(
            &mut self,
            _req: &[u8],
        ) -> std::result::Result<Vec<u8>, fermentool_modbus::TransportError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(fermentool_modbus::TransportError::Timeout)
        }
    }

    #[test]
    fn scale_tick_fast_fails_once_the_link_is_confirmed_down() {
        // Each failing read can cost up to the watchdog's timeout on the
        // shared control thread; once the link is known down, stop asking.
        let mut e = engine();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        e.attach_scale_for_test(Box::new(CountingFailingScale { calls: calls.clone() }));
        e.start_run(trim_cfg(), t0()).unwrap();
        let trip = REOPEN_AFTER_WRITE_FAILS as i64;
        for i in 0..trip {
            e.scale_tick(at(i));
        }
        assert!(e.scale_link_down());
        let at_trip = calls.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(at_trip, REOPEN_AFTER_WRITE_FAILS as usize);
        for i in trip..trip + 50 {
            e.scale_tick(at(i));
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), at_trip);
    }

    #[test]
    fn recover_scale_is_a_noop_when_nothing_is_configured() {
        let mut e = engine();
        assert!(e.recover_scale());
        assert!(!e.scale_wanted());
    }

    #[test]
    fn recover_scale_leaves_a_healthy_link_alone() {
        let mut e = engine();
        e.attach_scale(Some(Box::new(ScriptedScale::new(&[1000.0]))), scale_cfg("sim-scale"));
        assert!(!e.scale_link_down());
        assert!(e.recover_scale());
    }

    #[test]
    fn recover_scale_stays_down_when_the_port_will_not_open() {
        let mut e = engine();
        e.attach_scale(None, scale_cfg("NOPE_NOT_A_REAL_PORT_99999"));
        assert!(e.scale_wanted());
        assert!(e.scale_link_down());
        assert!(!e.recover_scale());
        assert!(e.scale_link_down());
    }

    #[test]
    fn recover_scale_does_not_treat_a_trim_alarm_as_a_down_link() {
        // scale_ok can be false from a mass-balance alarm on a perfectly
        // reachable scale; recovery must leave that for start_run to clear.
        let mut e = engine();
        e.attach_scale(
            Some(Box::new(ScriptedScale::new(&[1000.0, 1000.0]))),
            scale_cfg("sim-scale"),
        );
        e.start_run(flat_60_ml_min_cfg(), t0()).unwrap();
        e.scale_tick(t0());
        e.scale_tick(at(1));
        assert!(!e.status().scale_ok, "must have alarmed");
        assert!(!e.scale_link_down());
        assert!(e.recover_scale());
        assert!(!e.status().scale_ok, "recover_scale must not clear an alarm it didn't cause");
    }

    // ---- tubing calibration gate (Task 12) ----

    /// Record a calibration of `setpoint` (in `control_var`'s unit) from three
    /// finished 5-minute bursts that each delivered `weight_g` grams.
    fn seed_calibration(
        store: &Store,
        control_var: ControlVar,
        setpoint: f64,
        weight_g: f64,
    ) -> i64 {
        let mut runs = [0; 3];
        for (i, run) in runs.iter_mut().enumerate() {
            let started = at(-3600 + 600 * i as i64);
            *run = store
                .insert_run(&NewRun {
                    name: "cal".into(),
                    started_at: started,
                    control_var,
                    direction: Direction::Cw,
                    tick_interval_s: 1,
                    pump_addr: 1,
                    app_version: "test".into(),
                    curve: CurveSpec::linear(setpoint, setpoint, Duration::from_secs(600)),
                    gravimetric_trim: false,
                    kind: RunKind::Calibration,
                    tubing_calibration_id: None,
                })
                .unwrap();
            store
                .finish_run(*run, RunStatus::Stopped, started + SignedDuration::from_mins(5))
                .unwrap();
        }
        store
            .insert_calibration(&crate::store::NewCalibration {
                tubing_lot_id: "LOT".into(),
                tubing_size: "1.6mm".into(),
                control_var,
                setpoint,
                density_g_per_ml: 1.0,
                run_ids: runs,
                weights_g: [weight_g; 3],
                operator: None,
                note: None,
            })
            .unwrap()
    }

    #[test]
    fn a_calibration_burst_stops_the_pump_at_its_end_instead_of_holding() {
        // A burst's delivered mass is weighed against its own
        // started_at..ended_at: a pump still running after the curve ends
        // would put extra grams in the bottle that no duration accounts for.
        let mut e = engine();
        let cfg = RunConfig {
            control_var: ControlVar::MlMin,
            curve: CurveSpec::linear(10.0, 10.0, Duration::from_secs(300)),
            kind: RunKind::Calibration,
            ..linear_cfg()
        };
        let id = e.start_run(cfg, t0()).unwrap();
        assert_eq!(e.status().active.as_ref().map(|a| a.kind), Some(RunKind::Calibration));
        let o = e.tick(at(300)).unwrap();
        assert!(matches!(o, TickOutcome::Finished { .. }));
        assert!(!e.pump.transport().running(), "a burst must not keep pumping");
        assert!(e.status().holding.is_none());
        let run = e.store.run(id).unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Completed);
        assert_eq!(run.ended_at, Some(at(300)));
    }

    #[test]
    fn rpm_mode_trim_without_a_calibration_is_refused() {
        let mut e = engine();
        let cfg = RunConfig {
            control_var: ControlVar::Rpm,
            gravimetric_trim: true,
            tubing_calibration_id: None,
            ..linear_cfg()
        };
        let err = e.start_run(cfg, t0()).unwrap_err();
        assert!(matches!(err, EngineError::GravimetricTrimNeedsCalibration), "got {err:?}");
        assert!(err.hint().is_some());
        assert!(e.status().active.is_none());
    }

    #[test]
    fn ml_min_mode_trim_without_a_calibration_still_starts() {
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let cfg = RunConfig { tubing_calibration_id: None, ..trim_cfg() };
        assert!(e.start_run(cfg, t0()).is_ok());
        assert_eq!(e.trim_c(), 1.0);
    }

    #[test]
    fn an_ml_min_calibration_seeds_trim_c_with_its_c0() {
        let mut e = engine();
        // 10 ml/min commanded, 45 g in 5 min = 9 ml/min: c0 = 10/9.
        let cal = seed_calibration(&e.store, ControlVar::MlMin, 10.0, 45.0);
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let cfg = RunConfig { tubing_calibration_id: Some(cal), ..trim_cfg() };
        let id = e.start_run(cfg, t0()).unwrap();
        assert!((e.trim_c() - 10.0 / 9.0).abs() < 1e-9, "got {}", e.trim_c());
        assert_eq!(e.store.run(id).unwrap().unwrap().tubing_calibration_id, Some(cal));
    }

    #[test]
    fn a_calibration_c0_outside_the_trim_bounds_is_clamped() {
        let mut e = engine();
        // 10 ml/min commanded, 25 g in 5 min = 5 ml/min: c0 = 2.0.
        let cal = seed_calibration(&e.store, ControlVar::MlMin, 10.0, 25.0);
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let cfg = RunConfig { tubing_calibration_id: Some(cal), ..trim_cfg() };
        e.start_run(cfg, t0()).unwrap();
        assert_eq!(e.trim_c(), trim::TRIM_MAX);
    }

    #[test]
    fn an_rpm_calibration_converts_the_curve_for_the_mass_balance() {
        // Calibrated at 50 rpm -> 600 g in 5 min = 120 ml/min, i.e. 2.4 ml/min
        // per rpm. A flat 50 rpm run is then 120 ml/min = 2 g/s at 1 g/mL.
        // A bottle losing exactly that must leave c at 1.0 without alarming;
        // treating rpm as ml/min would expect 50/60 g/s and alarm at once.
        // (Whole grams per second: the scripted scale reports 0.1 g.)
        let mut e = engine();
        let cal = seed_calibration(&e.store, ControlVar::Rpm, 50.0, 600.0);
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let cfg = RunConfig {
            control_var: ControlVar::Rpm,
            gravimetric_trim: true,
            tubing_calibration_id: Some(cal),
            curve: CurveSpec::linear(50.0, 50.0, Duration::from_secs(36_000)),
            ..linear_cfg()
        };
        e.start_run(cfg, t0()).unwrap();
        assert_eq!(e.trim_c(), 1.0, "an rpm c0 is not a trim factor");
        let mut w = 4_000.0;
        for i in 0..120 {
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            w -= 2.0 * e.trim_c();
        }
        assert!(e.status().scale_ok, "the rpm curve was not converted to volume");
        assert!((e.trim_c() - 1.0).abs() < 1e-9, "got {}", e.trim_c());
    }

    #[test]
    fn a_calibration_for_the_other_unit_or_an_unknown_id_is_refused() {
        let mut e = engine();
        let rpm_cal = seed_calibration(&e.store, ControlVar::Rpm, 50.0, 100.0);
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let cfg = RunConfig { tubing_calibration_id: Some(rpm_cal), ..trim_cfg() };
        assert!(matches!(e.start_run(cfg, t0()), Err(EngineError::Config(_))));
        let cfg = RunConfig { tubing_calibration_id: Some(9999), ..trim_cfg() };
        assert!(matches!(e.start_run(cfg, t0()), Err(EngineError::Config(_))));
        assert!(e.status().active.is_none());
    }

    #[test]
    fn a_resumed_rpm_trim_run_keeps_its_volume_conversion() {
        let db = TempDb::new();
        let cfg_for = |cal| RunConfig {
            control_var: ControlVar::Rpm,
            gravimetric_trim: true,
            tubing_calibration_id: Some(cal),
            curve: CurveSpec::linear(50.0, 50.0, Duration::from_secs(36_000)),
            ..linear_cfg()
        };
        {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            let cal = seed_calibration(&a.store, ControlVar::Rpm, 50.0, 600.0);
            a.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
            a.start_run(cfg_for(cal), t0()).unwrap();
        }
        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        b.resume(at(10), grace()).unwrap();
        let mut w = 4_000.0;
        for i in 10..130 {
            b.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            b.scale_tick(at(i));
            w -= 2.0 * b.trim_c();
        }
        assert!(b.status().scale_ok, "resume lost the rpm-to-volume conversion");
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
    fn commanded_volume_accumulates_as_trapezoid_integral() {
        // F(t) = t_min ml/min on this line, so the exact integral from 0 to
        // t_min is a clean t_min^2 / 2, checkable by hand at every tick.
        let mut e = engine();
        let cfg = RunConfig {
            control_var: ControlVar::MlMin,
            curve: CurveSpec::linear(0.0, 60.0, Duration::from_secs(3600)),
            ..linear_cfg()
        };
        e.start_run(cfg, t0()).unwrap();
        assert_eq!(e.status().active.unwrap().volume_added_ml, Some(0.0));

        e.tick(at(600)).unwrap(); // 10 min in, target = 10 ml/min
        let v = e.status().active.unwrap().volume_added_ml.unwrap();
        assert!((v - 50.0).abs() < 1e-6, "got {v}"); // 10^2 / 2

        e.tick(at(1200)).unwrap(); // 20 min in, target = 20 ml/min
        let v = e.status().active.unwrap().volume_added_ml.unwrap();
        assert!((v - 200.0).abs() < 1e-6, "got {v}"); // 20^2 / 2
    }

    #[test]
    fn commanded_volume_is_none_for_an_rpm_run() {
        let mut e = engine();
        e.start_run(linear_cfg(), t0()).unwrap();
        e.tick(at(600)).unwrap();
        assert_eq!(e.status().active.unwrap().volume_added_ml, None);
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
    fn natural_completion_keeps_the_pump_running_and_holds() {
        let mut e = engine();
        let id = e.start_run(linear_cfg(), t0()).unwrap();
        let o = e.tick(at(3601)).unwrap(); // past the 3600 s duration
        assert!(matches!(o, TickOutcome::Finished { .. }));

        // pump is NOT stopped on natural completion
        assert!(e.pump.transport().running());
        assert!(e.status().active.is_none());

        let h = e.status().holding.expect("holding set after completion");
        assert_eq!(h.run_id, id);
        assert_eq!(h.name, "r");
        assert_eq!(h.control_var, ControlVar::Rpm);
        assert!((h.value - 100.0).abs() < 1e-6); // curve end, quantised
        assert_eq!(h.volume_added_ml, None); // rpm run, no calibrated volume

        assert_eq!(
            e.store().run(id).unwrap().unwrap().status,
            RunStatus::Completed
        );
    }

    #[test]
    fn holding_freezes_the_commanded_volume_at_completion() {
        // Same F(t) = t_min ml/min line as the other commanded-volume tests:
        // exact integral over the full hour is 60^2 / 2 = 1800.
        let mut e = engine();
        let cfg = RunConfig {
            control_var: ControlVar::MlMin,
            curve: CurveSpec::linear(0.0, 60.0, Duration::from_secs(3600)),
            ..linear_cfg()
        };
        e.start_run(cfg, t0()).unwrap();
        e.tick(at(1800)).unwrap();
        assert!(matches!(
            e.tick(at(3600)).unwrap(),
            TickOutcome::Finished { .. }
        ));
        let h = e.status().holding.expect("holding set after completion");
        let v = h.volume_added_ml.expect("ml/min run freezes a volume");
        assert!((v - 1800.0).abs() < 1e-6, "got {v}");
    }

    #[test]
    fn holding_volume_is_not_inflated_by_a_late_finishing_tick() {
        // The control loop hits its 1s deadline "at or after", never exactly
        // on it, so the finishing tick can land a bit past `duration_s` (here
        // 3 s late). 100 -> 200 ml/min over 120 s averages 150 ml/min, exact
        // volume is 300.0 mL; without clamping the last slice to duration_s,
        // the extra 3 s at the (curve-clamped) 200 ml/min end rate would
        // inflate this to 310.0 mL.
        let mut e = engine();
        let cfg = RunConfig {
            control_var: ControlVar::MlMin,
            curve: CurveSpec::linear(100.0, 200.0, Duration::from_secs(120)),
            ..linear_cfg()
        };
        e.start_run(cfg, t0()).unwrap();
        e.tick(at(60)).unwrap();
        assert!(matches!(
            e.tick(at(123)).unwrap(), // 3 s past the 120 s duration
            TickOutcome::Finished { .. }
        ));
        let h = e.status().holding.expect("holding set after completion");
        let v = h.volume_added_ml.expect("ml/min run freezes a volume");
        assert!((v - 300.0).abs() < 1e-6, "got {v}, late tick overshot the true total");
    }

    #[test]
    fn commanded_volume_is_not_contaminated_by_apply_setpoint_between_ticks() {
        // Real runs interleave `apply_setpoint` (~every 150 ms) between
        // journal ticks (1 s), and `apply_setpoint` writes `last_target` too.
        // If the volume trapezoid reused `last_target` as its "previous"
        // corner (instead of its own `vol_target`), a single intervening
        // `apply_setpoint` call would pull in a too-recent value and inflate
        // the slice: on this F(t) = t_min ml/min line, a naive reuse of
        // `last_target` gives 225.0 mL for what must be exactly 200.0
        // (10^2/2 + trapezoid from (10,10) to (20,20) = 50 + 150).
        let mut e = engine();
        let cfg = RunConfig {
            control_var: ControlVar::MlMin,
            curve: CurveSpec::linear(0.0, 60.0, Duration::from_secs(3600)),
            ..linear_cfg()
        };
        e.start_run(cfg, t0()).unwrap();
        e.tick(at(600)).unwrap(); // 10 min in, target = 10 ml/min
        e.apply_setpoint(at(900)).unwrap(); // 15 min in, writes last_target = 15
        e.tick(at(1200)).unwrap(); // 20 min in, target = 20 ml/min
        let v = e.status().active.unwrap().volume_added_ml.unwrap();
        assert!((v - 200.0).abs() < 1e-6, "got {v}, contaminated by the intervening apply_setpoint");
    }

    #[test]
    fn stop_pump_stops_the_pump_and_clears_the_hold() {
        let mut e = engine();
        e.start_run(linear_cfg(), t0()).unwrap();
        e.tick(at(3601)).unwrap();
        assert!(e.pump.transport().running());

        e.stop_pump(at(3602)).unwrap();
        assert!(!e.pump.transport().running());
        assert!(e.status().holding.is_none());
    }

    #[test]
    fn stop_pump_with_nothing_held_is_idle() {
        let mut e = engine();
        assert!(matches!(e.stop_pump(t0()), Err(EngineError::Idle)));
    }

    #[test]
    fn starting_a_run_clears_a_completed_run_hold() {
        let mut e = engine();
        e.start_run(linear_cfg(), t0()).unwrap();
        e.tick(at(3601)).unwrap();
        assert!(e.status().holding.is_some());

        e.start_run(linear_cfg(), at(3602)).unwrap();
        assert!(e.status().holding.is_none());
    }

    #[test]
    fn status_reports_the_transport_and_defaults_to_sim() {
        let e = engine();
        assert_eq!(e.status().transport, "sim");
        assert_eq!(e.transport_kind(), &crate::transport::TransportKind::Sim);
    }

    #[test]
    fn swap_transport_is_refused_while_a_run_is_active() {
        let mut e = engine();
        e.start_run(linear_cfg(), t0()).unwrap();
        let sc = crate::config::SerialConfig {
            path: "sim".into(),
            baud: 9600,
            allow_simulator: false,
        };
        assert!(matches!(e.swap_transport(&sc, 1), Err(EngineError::Busy)));
    }

    #[test]
    fn swap_transport_when_idle_updates_the_recorded_kind() {
        let mut e = engine();
        let sc = crate::config::SerialConfig {
            path: "sim".into(),
            baud: 9600,
            allow_simulator: false,
        };
        assert_eq!(
            e.swap_transport(&sc, 1).unwrap(),
            crate::transport::TransportKind::Sim
        );
        assert_eq!(e.status().transport, "sim");
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
        // pump keeps holding the final setpoint; the run record is Completed
        assert!(e.pump.transport().running());
        assert!(e.status().holding.is_some());
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

        e.pump.transport_mut().drop_next = 2; // write + its retry both dropped
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
    fn write_fails_counter_tracks_the_streak_and_resets() {
        let mut e = engine();
        e.start_run(linear_cfg(), t0()).unwrap();
        assert_eq!(e.write_fails(), 0);

        e.pump.transport_mut().drop_next = 4; // 2 ticks × (write + retry) all dropped
        e.tick(at(10)).unwrap();
        assert_eq!(e.write_fails(), 1);
        e.tick(at(20)).unwrap();
        assert_eq!(e.write_fails(), 2);

        e.tick(at(30)).unwrap(); // drop_next exhausted → the write lands
        assert_eq!(e.write_fails(), 0);
    }

    #[test]
    fn sustained_readback_mismatch_flags_the_pump_then_clears() {
        let mut e = engine();
        let mut cfg = linear_cfg();
        cfg.curve = CurveSpec::linear(50.0, 50.0, Duration::from_secs(100_000)); // flat 50 rpm
        let id = e.start_run(cfg, t0()).unwrap();
        assert!(e.status().pump_confirmed);

        // pump stuck at 5 rpm no matter what we command
        e.pump.transport_mut().frozen_readback = Some(5.0);
        for s in 1..=45 {
            e.tick(at(s)).unwrap(); // readback checks at seq 10/20/30/40
        }
        assert!(!e.status().pump_confirmed);
        assert!(e
            .store()
            .events(Some(id), 100)
            .unwrap()
            .iter()
            .any(|ev| ev.kind == "readback_mismatch"));

        // fault cleared → the pump reads back the commanded value again
        e.pump.transport_mut().frozen_readback = None;
        for s in 46..=70 {
            e.tick(at(s)).unwrap();
        }
        assert!(e.status().pump_confirmed);
    }

    #[test]
    fn recover_serial_is_a_noop_on_the_simulator() {
        let mut e = engine();
        e.set_serial(
            crate::config::SerialConfig {
                path: "sim".into(),
                baud: 9600,
                allow_simulator: false,
            },
            1,
        );
        assert!(!e.serial_lost());
        assert!(e.recover_serial(t0()));
        assert!(!e.serial_lost());
    }

    #[test]
    fn recover_serial_stays_lost_when_the_port_will_not_open() {
        let transport: Box<dyn fermentool_modbus::Transport + Send> = Box::new(SimPump::new(1));
        let mut e = Engine::new(Pump::new(transport, 1), Store::open_in_memory().unwrap(), "test");
        e.set_serial(
            crate::config::SerialConfig {
                path: "NOPE_NOT_A_REAL_PORT_99999".into(),
                baud: 9600,
                allow_simulator: false,
            },
            1,
        );
        assert!(e.serial_lost()); // real port configured but engine is on sim
        assert!(!e.recover_serial(t0())); // swap_strict fails to open
        assert!(e.serial_lost());
        assert!(e
            .store()
            .events(None, 20)
            .unwrap()
            .iter()
            .any(|ev| ev.kind == "serial_lost"));
    }

    #[test]
    fn repeated_recover_serial_records_the_outage_once() {
        // The auto-recovery loop now calls `recover_serial` ~1x/s while the
        // port is down. The event log must capture the outage as a single
        // edge, not one row per attempt.
        let transport: Box<dyn fermentool_modbus::Transport + Send> = Box::new(SimPump::new(1));
        let mut e = Engine::new(Pump::new(transport, 1), Store::open_in_memory().unwrap(), "test");
        e.set_serial(
            crate::config::SerialConfig {
                path: "NOPE_NOT_A_REAL_PORT_99999".into(),
                baud: 9600,
                allow_simulator: false,
            },
            1,
        );
        for _ in 0..5 {
            assert!(!e.recover_serial(t0()));
        }
        let lost = e
            .store()
            .events(None, 50)
            .unwrap()
            .into_iter()
            .filter(|ev| ev.kind == "serial_lost")
            .count();
        assert_eq!(lost, 1, "serial_lost must be edge-triggered, got {lost}");
    }

    #[test]
    fn start_run_translates_a_rejected_ml_min_setpoint() {
        // The LabQ answers a ml/min write with exception 0x03 when it has no
        // head / tubing calibration. That must surface as an actionable error,
        // not a cryptic "pump: MODBUS exception 0x03", and leave no run row.
        let mut e = engine();
        e.pump.transport_mut().reject_setpoint_writes = true;
        let cfg = RunConfig {
            control_var: ControlVar::MlMin,
            curve: CurveSpec::linear(10.0, 40.0, Duration::from_secs(3600)),
            ..linear_cfg()
        };
        match e.start_run(cfg, t0()).unwrap_err() {
            EngineError::PumpRejectedSetpoint {
                code,
                control_var,
                ..
            } => {
                assert_eq!(code, 0x03);
                assert_eq!(control_var, ControlVar::MlMin);
            }
            other => panic!("wrong error variant: {other:?}"),
        }
        let err = EngineError::PumpRejectedSetpoint {
            code: 0x03,
            control_var: ControlVar::MlMin,
            value: 80.0,
        };
        let msg = err.to_string();
        // The one-line message names the value and the exception, and nothing more.
        assert!(msg.contains("80.000 ml/min"), "got: {msg}");
        assert!(msg.contains("illegal data value"), "got: {msg}");
        assert!(!msg.contains("tubing"), "the message must stay one line; got: {msg}");
        assert_eq!(err.code(), "pump_setpoint_rejected");
        // The actionable detail is in the hint.
        let hint = err.hint().expect("a ml/min reject carries a hint");
        assert!(hint.to_lowercase().contains("flow"), "hint should explain the flow range; got: {hint}");
        assert!(hint.contains("rpm"), "hint should offer rpm as a fallback; got: {hint}");
        assert!(e.store().list_runs(10).unwrap().is_empty());
    }

    #[test]
    fn setpoint_error_only_treats_0x03_as_a_rejected_value() {
        // 0x03 (illegal data value) → operator-actionable "the pump refused this
        // value" error.
        assert!(matches!(
            setpoint_error(
                PumpError::Pdu(PduError::Exception(0x03)),
                ControlVar::MlMin,
                12.0
            ),
            EngineError::PumpRejectedSetpoint { code: 0x03, .. }
        ));
        // 0x02 (illegal data address) is a register-map fault, not a value the
        // operator can fix, it must stay a plain pump error.
        assert!(matches!(
            setpoint_error(
                PumpError::Pdu(PduError::Exception(0x02)),
                ControlVar::Rpm,
                50.0
            ),
            EngineError::Pump(_)
        ));
        // Anything else, too.
        assert!(matches!(
            setpoint_error(
                PumpError::Transport(fermentool_modbus::TransportError::Timeout),
                ControlVar::Rpm,
                50.0
            ),
            EngineError::Pump(_)
        ));
    }

    #[test]
    fn confirm_link_latches_a_mute_pump_then_clears_when_it_answers() {
        // A real port that "opens" but whose pump answers nothing must read as
        // NOT connected: runs refused, alarm latched, then cleared once the
        // pump starts responding.
        let mut sim = SimPump::new(1);
        sim.drop_next = 2; // first confirm_link (read + retry) fails; then reads land
        let transport: Box<dyn Transport + Send> = Box::new(sim);
        let mut e = Engine::new(
            Pump::new(transport, 1),
            Store::open_in_memory().unwrap(),
            "test",
        );
        e.set_transport_kind(TransportKind::Serial("COM_TEST".into()));
        e.set_serial(
            crate::config::SerialConfig {
                path: "COM_TEST".into(),
                baud: 9600,
                allow_simulator: false,
            },
            1,
        );
        assert!(!e.serial_lost(), "precondition: not probed yet");

        assert!(!e.confirm_link(t0()));
        assert!(e.serial_lost());
        assert!(matches!(
            e.start_run(linear_cfg(), t0()),
            Err(EngineError::SerialDown)
        ));

        assert!(e.confirm_link(at(1)), "pump answers now");
        assert!(!e.serial_lost());
        assert!(e
            .store()
            .events(None, 20)
            .unwrap()
            .iter()
            .any(|ev| ev.kind == "serial_recovered"));
    }

    #[test]
    fn recover_serial_reopen_alone_does_not_clear_the_alarm() {
        // `swap_strict` "succeeds" on the SimPump engine, but a bare reopen is
        // not proof the pump is there, the alarm must stay latched until a
        // real MODBUS read confirms it.
        let mut e = engine();
        e.set_serial(
            crate::config::SerialConfig {
                path: "COM_NOT_REAL".into(),
                baud: 9600,
                allow_simulator: false,
            },
            1,
        );
        assert!(e.serial_lost());
        assert!(!e.recover_serial(t0()));
        assert!(e.serial_lost(), "a bare reopen must not clear the alarm");
    }

    #[test]
    fn start_run_is_refused_while_the_pump_link_is_down() {
        // Real port configured, engine on the sim fallback (port wouldn't open):
        // a run would drive nothing, so it must be refused up front.
        let transport: Box<dyn fermentool_modbus::Transport + Send> = Box::new(SimPump::new(1));
        let mut e = Engine::new(Pump::new(transport, 1), Store::open_in_memory().unwrap(), "test");
        e.set_serial(
            crate::config::SerialConfig {
                path: "NOPE_NOT_A_REAL_PORT_99999".into(),
                baud: 9600,
                allow_simulator: false,
            },
            1,
        );
        assert!(e.serial_lost());
        assert!(matches!(
            e.start_run(linear_cfg(), t0()),
            Err(EngineError::SerialDown)
        ));
        // No run row was written.
        assert!(e.store().list_runs(10).unwrap().is_empty());
    }

    fn sim_serial(allow_simulator: bool) -> crate::config::SerialConfig {
        crate::config::SerialConfig {
            path: "sim".into(),
            baud: 9600,
            allow_simulator,
        }
    }

    #[test]
    fn start_run_is_refused_on_the_simulator_unless_enabled() {
        let mut e = engine(); // on the simulator
        // The shipped default: sim transport, simulator runs NOT enabled.
        e.set_serial(sim_serial(false), 1);
        assert!(!e.serial_lost(), "sim is not a lost real link");
        assert!(e.status().simulator);
        assert!(!e.status().allow_simulator);
        assert!(matches!(
            e.start_run(linear_cfg(), t0()),
            Err(EngineError::SimulatorNotAllowed)
        ));
        assert!(e.store().list_runs(10).unwrap().is_empty());

        // Opt in → the run starts.
        e.set_serial(sim_serial(true), 1);
        assert!(e.start_run(linear_cfg(), t0()).is_ok());
    }

    #[test]
    fn resume_is_refused_on_the_simulator_unless_enabled() {
        let db = TempDb::new();
        let id = {
            // `Engine::new` is permissive until `set_serial`, so the run is created.
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            let id = a.start_run(linear_cfg(), t0()).unwrap();
            a.tick(at(600)).unwrap();
            id
        };

        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        b.set_serial(sim_serial(false), 1);
        assert!(matches!(
            b.resume(at(1200), grace()),
            Err(EngineError::SimulatorNotAllowed)
        ));

        // The run is still recoverable once simulator runs are enabled.
        b.set_serial(sim_serial(true), 1);
        assert_eq!(b.resume(at(1200), grace()).unwrap(), id);
    }

    #[test]
    fn probe_link_feeds_the_same_write_fail_streak() {
        // Idle-time link probe: a failed read counts toward the write-fail
        // streak exactly like a failed pump write, so the control loop's
        // recovery trips whether or not a run is carrying it.
        let mut sim = SimPump::new(1);
        sim.drop_next = 4; // 2 probes fail (read + retry each), then reads recover
        let transport: Box<dyn fermentool_modbus::Transport + Send> = Box::new(sim);
        let mut e = Engine::new(Pump::new(transport, 1), Store::open_in_memory().unwrap(), "test");

        assert!(!e.probe_link());
        assert_eq!(e.write_fails(), 1);
        assert!(!e.probe_link());
        assert_eq!(e.write_fails(), 2);
        assert!(e.probe_link()); // bus quiet again
        assert_eq!(e.write_fails(), 0);
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
        assert!((quantize(10.008_333, 1e-3) - 10.008).abs() < 1e-9);
        assert!((quantize(10.008_7, 1e-3) - 10.009).abs() < 1e-9);
        assert_eq!(setpoint_grid(ControlVar::Rpm), 0.1);
        assert_eq!(setpoint_grid(ControlVar::MlMin), 1e-3);
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
            // 10 → 46 ml/min over 1 h ⇒ 0.01 ml/min per second, so one 1e-3 grid
            // step every 100 ms.
            curve: CurveSpec::linear(10.0, 46.0, Duration::from_secs(3600)),
            ..linear_cfg()
        };
        e.start_run(cfg, t0()).unwrap();
        // 1250 ms in: raw 10.01250, exactly between two 1e-3 grid points.
        e.apply_setpoint(t0() + SignedDuration::from_millis(1250))
            .unwrap();
        let f = e.pump.transport().flow_ml_min() as f64;
        let on_grid = (f * 1_000.0).round() / 1_000.0;
        assert!((f - on_grid).abs() < 1e-5, "not snapped to the 1e-3 grid: {f}");
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
    fn completed_run_hold_survives_a_restart_and_clears_on_stop() {
        let db = TempDb::new();
        let id = {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            let id = a.start_run(linear_cfg(), t0()).unwrap();
            a.tick(at(3601)).unwrap(); // past the 3600 s duration → Finished
            assert!(a.status().holding.is_some());
            id // drop engine A, the pump keeps physically holding its setpoint
        };

        // Fresh engine on the same DB: the hold must come back so the UI still
        // offers a Stop button instead of showing "idle" while the pump runs.
        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        let h = b.status().holding.expect("hold restored on restart");
        assert_eq!(h.run_id, id);
        assert_eq!(h.control_var, ControlVar::Rpm);
        assert!(b.pending_recovery(at(4000), grace()).unwrap().is_none());

        b.stop_pump(at(4001)).unwrap();
        assert!(b.status().holding.is_none());

        // And a restart after the stop stays clear.
        let c = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        assert!(c.status().holding.is_none());
    }

    #[test]
    fn starting_a_run_clears_a_persisted_hold() {
        let db = TempDb::new();
        {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            a.start_run(linear_cfg(), t0()).unwrap();
            a.tick(at(3601)).unwrap();
        }
        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        assert!(b.status().holding.is_some());
        b.start_run(linear_cfg(), at(4000)).unwrap();
        assert!(b.status().holding.is_none());
        let c = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        assert!(c.status().holding.is_none());
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
    fn commanded_volume_is_reconstructed_after_a_simulated_crash() {
        // Same F(t) = t_min ml/min line as commanded_volume_accumulates_...,
        // so every partial sum below is checkable by hand, and (trapezoid on
        // a straight line is exact for any partition) the reconstructed
        // total across a sparse post-crash tick history stays exact too.
        let db = TempDb::new();
        let ml_cfg = RunConfig {
            control_var: ControlVar::MlMin,
            curve: CurveSpec::linear(0.0, 60.0, Duration::from_secs(3600)),
            ..linear_cfg()
        };
        let id = {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            let id = a.start_run(ml_cfg, t0()).unwrap();
            a.tick(at(600)).unwrap(); // target 10 ml/min
            a.tick(at(1200)).unwrap(); // target 20 ml/min
            id // drop engine A -> "crash", in-memory accumulator lost
        };

        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        assert_eq!(b.resume(at(1800), grace()).unwrap(), id);
        // Reconstructed from the journalled ticks (0,0)->(600,10)->(1200,20),
        // plus the gap to this resume moment at (1800,30): 50 + 150 + 250.
        let v = b.status().active.unwrap().volume_added_ml.unwrap();
        assert!((v - 450.0).abs() < 1e-6, "got {v}");

        // Accumulation continues normally from the resumed anchor.
        b.tick(at(1810)).unwrap();
        let v2 = b.status().active.unwrap().volume_added_ml.unwrap();
        assert!(v2 > v, "volume must keep climbing after resume, got {v2}");

        assert!(matches!(
            b.tick(at(3600)).unwrap(),
            TickOutcome::Finished { .. }
        ));
        // Run finished; the completed run's volume no longer appears on
        // `active` (it is None once the run is holding, see HoldingStatus),
        // recompute it straight from the full journal to check the total
        // matches the curve's exact integral, 60^2 / 2.
        let full = b.store().ticks(id, 0, i64::MAX).unwrap();
        let mut total = 0.0;
        let mut prev = (0.0, 0.0);
        for t in &full {
            total += volume_slice(prev, (t.elapsed_s, t.target));
            prev = (t.elapsed_s, t.target);
        }
        assert!((total - 1800.0).abs() < 0.01, "got {total}");
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
