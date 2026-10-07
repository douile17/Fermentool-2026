//! The control engine: run lifecycle + the tick loop.
//!
//! [`Engine::tick`] does exactly one tick's work, compute the setpoint for the
//! **real elapsed time** (`now - started_at`), write it to the pump, journal it,
//! and finish the run when the curve is done. Time is injected, so a 100 h run
//! is exercised in milliseconds by calling `tick` with synthetic timestamps
//! (`docs/IMPLEMENTATION_PLAN.md` §4.5).

use std::sync::atomic::{AtomicBool, Ordering};
use std::collections::VecDeque;
use std::time::Duration;

use fermentool_curves::CurveSpec;
use fermentool_modbus::{limits, PduError, Pump, PumpError, PumpTransport, Transport, TransportError};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::config::{ScaleConfig, SerialConfig};
use crate::scale;
use crate::store::{
    ControlVar, Direction, EventLevel, EventRow, NewEvent, NewRun, NewTick, RunKind, RunStatus,
    Store, StoreError,
};
use crate::transport::{SwapTransport, TransportKind};
use crate::tracking;
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

    /// A refusal that only says the pump cannot be reached right now: trying
    /// again once it answers may well work.
    pub fn is_link_problem(&self) -> bool {
        matches!(
            self,
            EngineError::SerialDown
                | EngineError::SimulatorNotAllowed
                | EngineError::Pump(PumpError::Transport(_))
        )
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
    /// Who the run belongs to (a `[notify]` person): its notifications go to
    /// them only. Required by the API for a dosing run once people exist.
    #[serde(default)]
    pub responsible: Option<String>,
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
    /// Only resuming is off the table: a calibration burst whose `elapsed`
    /// is past `duration + grace` can only be closed (finish / abort). A
    /// dosing run never is, past its curve it resumes into the hold phase.
    pub past_end: bool,
    /// The curve reached its end (while offline): a resume holds its final
    /// value, still regulated and journalled.
    pub curve_done: bool,
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
    /// A calibration burst reached its end; the run is now `completed` and
    /// the pump stopped. A dosing run never finishes on its own: past its
    /// curve it holds the end value (still `Applied`) until stopped.
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
    /// Whether the balance link itself is up (`false`: unplugged, or its
    /// reads keep failing). Lets the UI tell that apart from a trim alarm on a
    /// reachable balance, both of which make `scale_ok` false. `None` when no
    /// scale is configured.
    pub scale_connected: Option<bool>,
    /// Last balance reading in grams, and whether the balance called it
    /// stable. `None` when no scale is configured, the link is down, or
    /// nothing has been read yet.
    pub scale_weight_g: Option<f64>,
    pub scale_stable: Option<bool>,
    /// `None` when no scale is configured.
    pub scale_state: Option<trim::ScaleState>,
    /// `None` when no scale is configured.
    pub trim_c: Option<f64>,
    /// Diagnostic instantaneous rate from the Theil-Sen estimator, `None`
    /// until at least two samples have been buffered since the last refill.
    pub rate_g_per_min: Option<f64>,
    /// Cumulative feed tracking, `Some` only during a trimmed run.
    pub tracking: Option<TrackingStatus>,
    /// A Stop did not reach the pump (link down, bus error): it may still be
    /// running. The daemon sends the Stop again until it gets through.
    pub stop_pending: bool,
    /// What the start found wrong: a damaged `config.toml` (defaults used),
    /// values put back in range, a journal that fails its check.
    pub warnings: Vec<String>,
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
    pub name: String,
    /// The curve reached its end: the run is in its hold phase, the pump at
    /// the curve's final value, trim and journal still running, until Stop.
    pub curve_done: bool,
}

struct ActiveRun {
    id: i64,
    name: String,
    /// Past `duration_s`, see [`ActiveStatus::curve_done`].
    curve_done: bool,
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

/// The `refill` event's detail. [`parse_refill`] reads it back for the
/// tracking report: keep the two in step.
fn refill_detail(before_g: f64, after_g: f64) -> String {
    format!("bottle refilled: {before_g:.1} g -> {after_g:.1} g ({:+.1} g)", after_g - before_g)
}

/// `(before_g, after_g)` from a [`refill_detail`] string.
fn parse_refill(detail: &str) -> Option<(f64, f64)> {
    let rest = detail.strip_prefix("bottle refilled: ")?;
    let (before, rest) = rest.split_once(" g -> ")?;
    let (after, _) = rest.split_once(" g")?;
    Some((before.trim().parse().ok()?, after.trim().parse().ok()?))
}

/// Up to which elapsed time commanded volume is counted. A calibration burst
/// ends at its duration, so a finishing tick's scheduling overshoot must not
/// add time at the end rate. A dosing run keeps pumping through its hold
/// phase, so its volume keeps counting.
fn volume_horizon_s(kind: RunKind, duration_s: i64) -> f64 {
    match kind {
        RunKind::Calibration => duration_s as f64,
        RunKind::Dosing => f64::INFINITY,
    }
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
    /// How [`recover_scale`](Self::recover_scale) opens the port:
    /// [`open_scale`] for the real balance, a stand-in in tests.
    scale_open: fn(&ScaleConfig) -> Option<Box<dyn Transport + Send>>,
    /// Dimensionless correction multiplied onto every curve setpoint:
    /// `target = curve.value_at(elapsed) * trim_c`. `1.0` is a no-op, the
    /// value while `gravimetric_trim` is off (or unset) on the active run.
    trim_c: f64,
    scale_state: trim::ScaleState,
    scale_read_fails: u32,
    /// `true` from a reopen of the port until the balance answers on it: a
    /// COM port opens whether or not a balance is on the other end, so only
    /// a weight shows the link is back (see [`scale_link_up`](Self::scale_link_up)).
    scale_unconfirmed: bool,
    /// `false` once the tracker alarms (the pump stayed out of what c can
    /// correct for `tracking::ALARM_AFTER_SATURATED_UPDATES` updates).
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
    /// The balance reading (raw, as displayed) just before the current
    /// refill began, journalled with the reading after it in a `refill`
    /// event so a run's bottle weights can be checked end to end.
    refill_before_g: Option<f64>,
    /// `(seconds_since_refill, weight_g)` samples since `refill_at`, feeding
    /// the Theil-Sen rate estimate. Cleared on every new refill reference.
    weight_buffer: Vec<(f64, f64)>,
    last_weight_g: Option<f64>,
    settling_since: Option<Timestamp>,
    /// The last `trim::RECENT_READS_S` of reads, in every balance state, for
    /// the refill tests (a steady rise, a still level). `delivered_g` is the
    /// tracker's count after a `Normal` read, `None` otherwise.
    recent_reads: VecDeque<(trim::Read, Option<f64>)>,
    manual_refill_mode: bool,
    /// When "Refill bottle" was pressed: the request lapses after
    /// `MANUAL_REFILL_TIMEOUT_S` without a pour.
    manual_refill_since: Option<Timestamp>,
    /// One-shot: consumed (reset to `false`) by the next `scale_tick`.
    manual_refill_done_flag: bool,
    /// Diagnostic only (not fed back into `trim_c`, the cumulative mass
    /// balance drives the correction): the instantaneous measured rate.
    last_rate_g_per_min: Option<f64>,
    /// Last balance reading `(grams, stable)`, for display only. `None` until
    /// a read succeeds, and again after any failed read.
    live_weight: Option<(f64, bool)>,
    /// Cumulative feed accounting for the active trimmed run.
    tracker: Tracker,
    /// The balance's display step, from its last reply (sizes the tracking
    /// window). 0.1 g until something has been read.
    scale_resolution_g: f64,
    scale_density_g_per_ml: f64,
    /// Set from a tubing calibration (Task 12) for `control_var: Rpm` runs:
    /// mL/min delivered per commanded rpm, so `integrate_curve_mass` can
    /// convert the curve's rpm output into a volumetric rate. `None` (the
    /// default, and always for `ControlVar::MlMin`, where the curve is
    /// already volumetric) is treated as `1.0`, a no-op factor.
    rpm_to_ml_min: Option<f64>,
    /// An ml/min run driven in rpm: ml/min per rpm from the run's rpm-mode
    /// tubing calibration. The curve, the journal, the volume and the trim
    /// stay in ml/min; only the value written to the pump (and read back)
    /// is rpm, so the pump's own head/tubing table is never involved.
    /// `None`: the pump converts ml/min itself, or the run is in rpm.
    drive_ml_per_rpm: Option<f64>,
    /// An ml/min run driven in rpm whose setpoint is under the motor's
    /// 0.1 rpm minimum: the pump is held stopped until the curve rises.
    drive_paused: bool,
    /// A Stop that did not reach the pump, with the run it ended (if any):
    /// sent again by the control loop until it gets through. Persisted, a
    /// pump left running must not be forgotten across a restart.
    stop_pending: Option<Option<i64>>,
    /// See [`EngineStatus::warnings`].
    warnings: Vec<String>,
}

/// Cap on `weight_buffer`'s length: a gravimetric run with no refill appends
/// one point per second, so over 100 h that vector would grow without bound.
/// Past the cap it is decimated back down to `trim::MAX_SLOPE_POINTS`.
const WEIGHT_BUFFER_CAP: usize = trim::MAX_SLOPE_POINTS * 4;

/// A "Refill bottle" request with no pour after this long is dropped, or a
/// forgotten one would keep c frozen for the rest of the run.
const MANUAL_REFILL_TIMEOUT_S: f64 = 900.0;

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

/// `app_state` key of a Stop not yet delivered: the run id it ended, `0` for
/// none, empty when nothing is pending.
const STOP_PENDING_KEY: &str = "stop_pending";

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedTrim {
    trim_c: f64,
    scale_state: trim::ScaleState,
    refill_weight_g: Option<f64>,
    refill_at: Option<Timestamp>,
    #[serde(default)]
    tracker: Tracker,
}

/// Cumulative feed accounting for the active trimmed run (see
/// `docs/superpowers/plans/2026-09-29-cumulative-feed-tracking.md`). Masses in
/// grams since run start. Persisted with the trim so a crash resume carries on.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Tracker {
    /// Run elapsed time the masses were last integrated to.
    last_t_s: Option<f64>,
    /// What the curve asked for.
    required_g: f64,
    /// What the pump was told (curve times c).
    commanded_g: f64,
    /// What left the bottle.
    delivered_g: f64,
    /// Last `Normal` reading, and the commanded mass at that moment.
    last_weight_g: Option<f64>,
    last_normal_commanded_g: f64,
    /// Set while the balance is out of `Normal` (touch, refill, restart).
    anchor: Option<tracking::Anchor>,
    /// Recent `Normal` reads for the delivery-ratio slope.
    #[serde(skip)]
    points: Vec<tracking::TrackPoint>,
    /// Last delivery-ratio estimate.
    k: Option<f64>,
    last_update_t_s: f64,
    saturated_updates: u32,
    /// The measured delivery ran backwards: the balance weighs the other
    /// side from what Settings says. Freezes the trim (via `scale_ok`).
    wrong_side: bool,
    /// c at run start (1.0, or the calibration's c0): the feed-forward of
    /// [`tracking::next_c_early`] until the delivery ratio is measured.
    /// `None` in a state saved before it existed: the early law then waits.
    c_seed: Option<f64>,
    /// The `trim_start` event (c first moved) was journalled for this run.
    start_logged: bool,
    /// Why the trim stopped regulating, if it did. c is frozen meanwhile.
    alarm: Option<tracking::TrimAlarm>,
    /// Where the feed was last seen leaving the bottle (feed-stop detection).
    flow_ref: Option<tracking::FlowRef>,
    /// Deficit written off when the feed came back after an alarm: what was
    /// missed while it was stopped stays in the totals but is not paid back,
    /// a burst of overfeed being worse for a culture than the gap itself.
    forgiven_g: f64,
    /// `(t_s, c)` at each update over the last hour: an alarm puts c back to
    /// its value from before the problem started, not where it ended up.
    /// Persisted (~360 pairs), so an alarm soon after a crash resume still
    /// finds it.
    c_hist: std::collections::VecDeque<(f64, f64)>,
    /// When the current run of saturated updates began.
    saturated_since_t: Option<f64>,
}

/// Delivered mass below `-max(WRONG_SIDE_MIN_G, WRONG_SIDE_FRACTION *
/// required)` means the weight moved the wrong way for the configured
/// balance position. A touch or a sample can dip it briefly; this much, over
/// an update period, cannot come from the pump.
const WRONG_SIDE_MIN_G: f64 = 2.0;
const WRONG_SIDE_FRACTION: f64 = 0.25;

/// The proof that a run's delivered feed followed its curve, from the journal.
#[derive(Debug, Clone, Serialize)]
pub struct TrackingReport {
    /// `[t_s, required_ml, delivered_ml]`, cumulative, at most 2000 points.
    pub points: Vec<[f64; 3]>,
    /// Of delivered against required.
    pub r_squared: Option<f64>,
    /// Required minus delivered at the last point, positive = behind.
    pub deficit_ml: f64,
    pub deficit_pct: Option<f64>,
    /// Exponential curves only: the µ asked for, and the µ that best fits
    /// the delivered volume (per hour).
    pub mu_requested: Option<f64>,
    pub mu_delivered: Option<f64>,
    /// When the regulation acted, for the chart: `trim_start` (c first
    /// moved) and `trim_ratio` (pump ratio first measured).
    pub markers: Vec<TrackMarker>,
    /// Balance reading (as displayed) at the first and the last journalled
    /// tick, and around each refill: the bottle's weights end to end, to
    /// check the weighed-out total by hand.
    pub weight_start_g: Option<f64>,
    pub weight_end_g: Option<f64>,
    pub refills: Vec<RefillRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefillRecord {
    /// Run elapsed time when the refill settled, seconds.
    pub t_s: f64,
    pub before_g: f64,
    pub after_g: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrackMarker {
    /// Run elapsed time, seconds.
    pub t_s: f64,
    pub kind: String,
}

/// The tracker's view for the status frame, volumes in mL.
#[derive(Debug, Clone, Serialize)]
pub struct TrackingStatus {
    pub required_ml: f64,
    pub delivered_ml: f64,
    /// `100 * (required - delivered) / required`, positive = behind.
    pub deficit_pct: Option<f64>,
    /// The pump's measured delivered / commanded ratio.
    pub delivery_ratio: Option<f64>,
    /// The weight moves the wrong way for the configured balance position
    /// (Settings, Balance): the trim is frozen, the numbers are not meaningful.
    pub wrong_side: bool,
    /// Why the trim stopped regulating, if it did.
    pub alarm: Option<tracking::TrimAlarm>,
    /// Feed missed during past alarms, in the totals but not paid back, mL.
    pub missed_ml: f64,
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
        let stop_pending = store
            .get_state(STOP_PENDING_KEY)
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i64>().ok())
            .map(|id| (id > 0).then_some(id));
        let (trim_c, scale_state, refill_weight_g, refill_at, tracker) = match persisted_trim {
            Some(p) => (p.trim_c, p.scale_state, p.refill_weight_g, p.refill_at, p.tracker),
            None => (1.0, trim::ScaleState::Normal, None, None, Tracker::default()),
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
            scale_unconfirmed: false,
            scale_open: open_scale,
            scale_ok: true,
            refill_weight_g,
            refill_at,
            refill_before_g: None,
            weight_buffer: Vec::new(),
            last_weight_g: None,
            settling_since: None,
            recent_reads: VecDeque::new(),
            manual_refill_since: None,
            manual_refill_mode: false,
            manual_refill_done_flag: false,
            last_rate_g_per_min: None,
            live_weight: None,
            tracker,
            scale_resolution_g: 0.1,
            scale_density_g_per_ml: 1.0,
            rpm_to_ml_min: None,
            drive_ml_per_rpm: None,
            drive_paused: false,
            stop_pending,
            warnings: Vec::new(),
        }
    }

    /// Something the start found wrong, shown until the next start.
    pub fn add_warning(&mut self, warning: String) {
        tracing::error!("{warning}");
        self.warnings.push(warning);
    }

    fn set_stop_pending(&mut self, pending: Option<Option<i64>>) {
        let value = pending.map(|id| id.unwrap_or(0).to_string()).unwrap_or_default();
        let _ = self.store.set_state(STOP_PENDING_KEY, &value);
        self.stop_pending = pending;
    }

    /// Stop the pump. A Stop that does not get through (link down, bus
    /// error) is kept pending and said once: the control loop sends it again
    /// until the pump acknowledges it ([`retry_pending_stop`](Self::retry_pending_stop)).
    /// Returns whether the pump acknowledged it now.
    fn stop_or_keep_pending(&mut self, run_id: Option<i64>, now: Timestamp) -> bool {
        self.drive_paused = false;
        // Over a lost link the stand-in proves nothing: never trust it.
        if !self.serial_lost && self.pump.stop().is_ok() {
            if self.stop_pending.is_some() {
                self.set_stop_pending(None);
            }
            return true;
        }
        if self.stop_pending.is_none() {
            self.set_stop_pending(Some(run_id));
            let _ = self.store.log_event(&NewEvent {
                run_id,
                wall_time: now,
                level: EventLevel::Error,
                kind: "stop_pending".into(),
                detail: Some(
                    "the Stop did not reach the pump (link down?): it may still be running;                      sent again as soon as the pump answers"
                        .into(),
                ),
            });
        }
        false
    }

    /// Whether a Stop is still waiting to reach the pump.
    pub fn stop_pending(&self) -> bool {
        self.stop_pending.is_some()
    }

    /// The control loop's retry of a pending Stop, while no run drives the
    /// pump and the link is up. Returns whether it got through now.
    pub fn retry_pending_stop(&mut self, now: Timestamp) -> bool {
        let Some(run_id) = self.stop_pending else {
            return false;
        };
        if self.serial_lost || self.active.is_some() || self.pump.stop().is_err() {
            return false;
        }
        self.set_stop_pending(None);
        let _ = self.store.log_event(&NewEvent {
            run_id,
            wall_time: now,
            level: EventLevel::Info,
            kind: "pump_stopped".into(),
            detail: Some("the pending Stop reached the pump: it is stopped".into()),
        });
        true
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
            tracker: self.tracker.clone(),
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
                // Once lost, one read decides: each costs a full timeout (two
                // with the pump's own retry) on the control thread.
                let answered = if self.serial_lost {
                    self.confirm_read()
                } else {
                    self.confirm_read() || self.confirm_read()
                };
                if answered {
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
            active: self.active_status(),
            transport: self.transport.label(),
            holding: self.holding.clone(),
            serial_ok: !self.serial_lost,
            write_fails: self.write_fails,
            pump_confirmed: self.pump_confirmed,
            journal_ok: self.journal_fails < JOURNAL_STALL_LIMIT,
            simulator: self.on_simulator(),
            allow_simulator: self.allow_simulator,
            scale_ok: !scale_shown || (self.scale_ok && self.scale_link_up()),
            scale_connected: scale_shown.then(|| self.scale_link_up()),
            scale_weight_g: self.live_weight.filter(|_| self.scale_link_up()).map(|(w, _)| w),
            scale_stable: self.live_weight.filter(|_| self.scale_link_up()).map(|(_, s)| s),
            scale_state: scale_shown.then_some(self.scale_state),
            trim_c: scale_shown.then_some(self.trim_c),
            rate_g_per_min: scale_shown.then_some(self.last_rate_g_per_min).flatten(),
            tracking: self.active.as_ref().filter(|a| a.gravimetric_trim).map(|_| {
                let tr = &self.tracker;
                let ml = |g: f64| g / self.scale_density_g_per_ml;
                TrackingStatus {
                    required_ml: ml(tr.required_g),
                    delivered_ml: ml(tr.delivered_g),
                    deficit_pct: (tr.required_g > 0.0)
                        .then(|| 100.0 * (tr.required_g - tr.delivered_g) / tr.required_g),
                    delivery_ratio: tr.k,
                    wrong_side: tr.wrong_side,
                    alarm: tr.alarm,
                    missed_ml: ml(tr.forgiven_g),
                }
            }),
            stop_pending: self.stop_pending.is_some(),
            warnings: self.warnings.clone(),
        }
    }

    /// The active run's part of [`status`](Self::status) alone: what the
    /// control loop checks several times a pass, without building the rest.
    pub fn active_status(&self) -> Option<ActiveStatus> {
        self.active.as_ref().map(|a| ActiveStatus {
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
            name: a.name.clone(),
            curve_done: a.curve_done,
        })
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
                if cal.archived_at.is_some() {
                    return Err(EngineError::Config(format!(
                        "tubing calibration {cal_id} is archived, restore it first"
                    )));
                }
                let drives_in_rpm = cfg.control_var == ControlVar::MlMin && cal.control_var == ControlVar::Rpm;
                if cal.control_var != cfg.control_var && !drives_in_rpm {
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

        // Everything that can refuse the run is checked before anything
        // changes: a refused start leaves the trim state, a hold left by an
        // older daemon and the pump as they were.
        let (lo, hi) = match cfg.control_var {
            ControlVar::Rpm => (limits::RPM_MIN, limits::RPM_MAX),
            ControlVar::MlMin => (limits::FLOW_MIN, limits::FLOW_MAX),
        };
        let mut spec = cfg.curve.clone();
        spec.clamp_min = spec.clamp_min.max(lo);
        spec.clamp_max = spec.clamp_max.min(hi);
        spec.validate().map_err(EngineError::Config)?;
        // What the tubing calibration brings: c's starting value (ml/min
        // calibration), or an rpm-to-volume conversion (rpm calibration),
        // used to drive an ml/min run in rpm or to count an rpm run's volume.
        let mut seed_c = 1.0;
        let mut drive = None;
        let mut rpm_to_ml = None;
        if let Some(cal) = &calibration {
            match cfg.control_var {
                // ml/min asked, rpm written: at the calibrated speed the
                // conversion is exact, so c starts at 1.0.
                ControlVar::MlMin if cal.control_var == ControlVar::Rpm => {
                    drive = Some(usable_ml_per_rpm(cal)?)
                }
                // Dimensionless: seeds the trim, within its usual bounds.
                ControlVar::MlMin => seed_c = self.trim_bounds().clamp(cal.c0),
                // rpm per (ml/min), not a trim factor: c starts at 1.0.
                ControlVar::Rpm => rpm_to_ml = Some(usable_ml_per_rpm(cal)?),
            }
        }
        if let Some(r) = drive {
            // The tube's ceiling at full speed, without the trim's headroom.
            let peak = spec.peak_value();
            if peak / r > limits::RPM_MAX {
                return Err(EngineError::Config(format!(
                    "the curve asks up to {peak:.2} ml/min; this tube delivers at most {:.2} ml/min at {} rpm",
                    limits::RPM_MAX * r,
                    limits::RPM_MAX
                )));
            }
        }

        let duration_s = spec.duration.as_secs() as i64;
        let c = if cfg.gravimetric_trim { seed_c } else { 1.0 };
        let first = quantize(spec.value_at(Duration::ZERO) * c, drive_grid(cfg.control_var, drive));

        // Pump start sequence (docs/IMPLEMENTATION_PLAN.md §4.3). The pump's
        // head-type / tubing-size registers are deliberately left untouched, see
        // the note in migrations/0001_init.sql.
        self.drive_ml_per_rpm = drive;
        self.drive_paused = false;
        let start_seq = std::time::Instant::now();
        self.pump.set_direction(cfg.direction == Direction::Cw)?;
        drive_write(&mut self.pump, &mut self.drive_paused, cfg.control_var, first, drive)
            .map_err(|e| setpoint_error(e, cfg.control_var, first, drive))?;
        // Driven in rpm under the motor's minimum: the curve asks for no
        // flow yet, the pump starts once it rises.
        if !self.drive_paused {
            self.pump.start()?;
        }
        // A calibration burst's length is a measurement: its clock starts
        // when the pump acknowledged Start, not when the command arrived
        // (the start sequence is three frames, ~0.3 s).
        let started_at = if cfg.kind == RunKind::Calibration { now + whole_ms(start_seq) } else { now };

        let id = match self.store.insert_run(&NewRun {
            name: cfg.name.clone(),
            started_at,
            control_var: cfg.control_var,
            direction: cfg.direction,
            tick_interval_s: TICK_INTERVAL.as_secs() as i64,
            pump_addr: cfg.pump_addr,
            app_version: self.app_version.clone(),
            curve: spec.clone(),
            gravimetric_trim: cfg.gravimetric_trim,
            kind: cfg.kind,
            tubing_calibration_id: cfg.tubing_calibration_id,
            responsible: cfg.responsible.clone(),
            density_g_per_ml: cfg.gravimetric_trim.then_some(self.scale_density_g_per_ml),
        }) {
            Ok(id) => id,
            Err(e) => {
                // The pump turns and no run would drive it: stop it again.
                self.stop_or_keep_pending(None, now);
                return Err(e.into());
            }
        };
        // The run exists from here: a journal line that fails must not undo it.
        let _ = self.store.log_event(&NewEvent {
            run_id: Some(id),
            wall_time: now,
            level: EventLevel::Info,
            kind: "start".into(),
            detail: Some(format!("first setpoint {first:.3}")),
        });

        // A fresh run gets a fresh gravimetric-trim window: the previous run's
        // refill baseline is meaningless against this run's own `started_at`,
        // and reusing it would make the first cumulative check compare this
        // run's theoretical mass against a stale measured one, tripping a
        // spurious alarm. `trim_c` restarts from the calibration's c0 (or 1.0),
        // not from whatever the last run happened to learn.
        self.trim_c = seed_c;
        self.scale_state = trim::ScaleState::Normal;
        self.refill_weight_g = None;
        self.refill_at = None;
        self.refill_before_g = None;
        self.weight_buffer.clear();
        self.last_weight_g = None;
        self.settling_since = None;
        self.recent_reads.clear();
        self.manual_refill_since = None;
        self.scale_read_fails = 0;
        self.scale_ok = true;
        self.manual_refill_mode = false;
        self.manual_refill_done_flag = false;
        self.last_rate_g_per_min = None;
        self.tracker = Tracker { last_t_s: Some(0.0), c_seed: Some(seed_c), ..Tracker::default() };
        self.rpm_to_ml_min = rpm_to_ml;
        // Persist the fresh state now: a crash before anything else changes
        // would otherwise restore the previous run's c and baseline.
        self.save_trim_state();
        // A new run supersedes any completed-run hold, and drives the pump a
        // pending Stop was for.
        self.set_holding(None);
        if self.stop_pending.is_some() {
            self.set_stop_pending(None);
        }

        self.readback_fails = 0;
        self.pump_confirmed = true;
        self.active = Some(ActiveRun {
            id,
            name: cfg.name.clone(),
            curve_done: false,
            started_at,
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
        let trimmed = active.gravimetric_trim;
        let elapsed_s = now.duration_since(active.started_at).as_secs_f64().max(0.0);
        let c = if active.gravimetric_trim { self.trim_c } else { 1.0 };
        let raw = active.spec.value_at(Duration::from_secs_f64(elapsed_s)) * c;
        let target = quantize(raw, drive_grid(control_var, self.drive_ml_per_rpm));
        let seq = active.next_seq;
        active.next_seq += 1;
        if let Some(vol) = active.volume_added_ml {
            // A calibration burst's finishing tick can land a little past
            // `duration_s` (the control loop hits its 1s deadline "at or
            // after", never exactly on it); without clamping this slice's time
            // bound, that overshoot would count as extra time at the final
            // rate. A dosing run keeps pumping past its curve, see
            // `volume_horizon_s`.
            let te = elapsed_s.min(volume_horizon_s(run_kind, duration_s));
            active.volume_added_ml =
                Some(vol + volume_slice((active.vol_elapsed_s, active.vol_target), (te, target)));
        }
        active.vol_elapsed_s = elapsed_s;
        active.vol_target = target;
        active.last_target = Some(target);
        active.last_write_q = Some(target);

        // A burst's end is a measurement too: the pump stops first thing,
        // before any other frame, and the run ends when it acknowledged Stop.
        let ending_burst = run_kind == RunKind::Calibration && elapsed_s >= duration_s as f64;
        let mut ended_at = now;
        // A lost link is not written to: nothing would reach the pump (it
        // holds its last setpoint), and every attempt costs a timeout on this
        // thread. Journalled as not written; the outage itself is one
        // `serial_lost` event, not a `write_fail` a second.
        let link_down = self.serial_lost;
        let write = if link_down {
            Err(PumpError::Transport(TransportError::Io("pump link down".into())))
        } else if ending_burst {
            let t = std::time::Instant::now();
            let stopped = self.pump.stop();
            ended_at = now + whole_ms(t);
            stopped
        } else {
            drive_write(&mut self.pump, &mut self.drive_paused, control_var, target, self.drive_ml_per_rpm)
        };
        let written_ok = write.is_ok();
        if !link_down {
            self.write_fails = if written_ok {
                0
            } else {
                self.write_fails.saturating_add(1)
            };
        }
        if let (Err(e), false) = (&write, link_down) {
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
        // Not while held stopped under the motor's minimum: no speed to compare.
        let readback: Option<f64> = if written_ok
            && !self.drive_paused
            && seq > 0
            && seq % READBACK_EVERY_TICKS == 0
        {
            match read_actual(&mut self.pump, control_var, self.drive_ml_per_rpm) {
                Ok(actual) => {
                    let mut actual = actual as f64;
                    // Driven in rpm, the motor tops out at RPM_MAX: compare with
                    // what was written, not with a target beyond the tube.
                    let written = match (control_var, self.drive_ml_per_rpm) {
                        (ControlVar::MlMin, Some(r)) => drive_rpm(target, r) * r,
                        _ => target,
                    };
                    let tol = (5.0 * drive_grid(control_var, self.drive_ml_per_rpm)).max(0.04 * written.abs());
                    if (actual - written).abs() > tol {
                        // A read answers with no register number: a late reply
                        // to an earlier read can land here. Read once more
                        // before counting a mismatch.
                        if let Ok(again) = read_actual(&mut self.pump, control_var, self.drive_ml_per_rpm) {
                            actual = again as f64;
                        }
                    }
                    if (actual - written).abs() > tol {
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
            weight_g: self.live_weight.map(|(w, _)| w),
            delivered_g: trimmed.then_some(self.tracker.delivered_g),
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

        if elapsed_s >= duration_s as f64 && run_kind == RunKind::Dosing {
            // A dosing run does not end with its curve: it holds the curve's
            // final value (`value_at` is flat past `duration`), still trimmed
            // and journalled every second, until the operator presses Stop,
            // which records it `completed`. Only the transition is logged.
            if let Some(a) = self.active.as_mut().filter(|a| !a.curve_done) {
                a.curve_done = true;
                tracing::info!(run_id = id, target, "curve done; holding, regulation continues");
                let _ = self.store.log_event(&NewEvent {
                    run_id: Some(id),
                    wall_time: now,
                    level: EventLevel::Info,
                    kind: "curve_done".into(),
                    detail: Some(format!(
                        "holding at {target:.3}, regulation and journal continue until Stop"
                    )),
                });
            }
        } else if elapsed_s >= duration_s as f64 {
            if let Err(e) = self.store.finish_run(id, RunStatus::Completed, ended_at) {
                // Couldn't record completion (disk full / unwritable). Don't
                // wedge the engine or drop the completion: keep the burst
                // active and retry on the next tick. The journal-stall streak
                // surfaces the disk problem through `journal_ok`.
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
            // A burst is weighed against its own started_at..ended_at, so the
            // pump stopped above instead of holding: any flow after ended_at
            // would land in the bottle with no time to match it. A Stop that
            // failed there is tried again, then kept pending.
            if !written_ok {
                self.stop_or_keep_pending(Some(id), now);
            }
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
        // The curve's final value belongs to the completing tick; a lost link
        // is not written to (see `tick`).
        if elapsed_s >= active.duration_s as f64 || self.serial_lost {
            return Ok(false);
        }
        let c = if active.gravimetric_trim { self.trim_c } else { 1.0 };
        let raw = active.spec.value_at(Duration::from_secs_f64(elapsed_s)) * c;
        let target = quantize(raw, drive_grid(control_var, self.drive_ml_per_rpm));
        if active.last_write_q == Some(target) {
            return Ok(false);
        }

        match drive_write(&mut self.pump, &mut self.drive_paused, control_var, target, self.drive_ml_per_rpm) {
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

    /// One balance read, shared by [`scale_tick`](Self::scale_tick) and
    /// [`probe_scale`](Self::probe_scale): updates the live weight and the
    /// read-failure streak. `None` on any failure, or without asking at all
    /// once the link is known down: `recover_scale`, on its own backoff, owns
    /// bringing the link back. (The real balance is a [`scale::PolledScale`],
    /// so a read never waits on the device; a test transport may.) A good
    /// read never clears `scale_ok`: that alarm is sticky until `start_run`.
    fn read_scale(&mut self) -> Option<(f64, bool)> {
        if self.scale_link_down() {
            return None;
        }
        match self.ask_scale()? {
            Ok(r) => Some(self.took_scale_reading(r)),
            Err(e) => {
                // Log the first failure of a streak and the one that takes
                // the link down: enough to see why, without a line a second.
                // A just-reopened port that stays silent is the recovery's
                // business, already logged once for the whole outage.
                if self.scale_read_fails == 0 && !self.scale_unconfirmed {
                    tracing::warn!("{e}");
                }
                self.scale_read_fails = self.scale_read_fails.saturating_add(1);
                if self.scale_read_fails == REOPEN_AFTER_WRITE_FAILS && !self.scale_unconfirmed {
                    tracing::warn!("{e}; balance link down after {REOPEN_AFTER_WRITE_FAILS} failed reads");
                }
                self.live_weight = None;
                None
            }
        }
    }

    /// Ask the attached balance for a weight; `None` without one.
    fn ask_scale(&mut self) -> Option<std::result::Result<scale::SicsReading, scale::ScaleError>> {
        Some(
            self.scale
                .as_mut()?
                .transaction(scale::SICS_IMMEDIATE)
                .map_err(scale::ScaleError::from)
                .and_then(|r| scale::parse_sics_weight(&r)),
        )
    }

    /// A weight came back: the link is up (and confirmed, after a reopen).
    fn took_scale_reading(&mut self, r: scale::SicsReading) -> (f64, bool) {
        let reading = (r.weight_g, r.stable);
        self.scale_read_fails = 0;
        self.scale_unconfirmed = false;
        self.scale_resolution_g = r.resolution_g;
        self.live_weight = Some(reading);
        reading
    }

    /// Chart, R² and fitted µ for a run, from its journal. `None` when the
    /// run has no delivered-mass data (no balance, or not trimmed). Volumes
    /// use the configured feed density.
    pub fn tracking_report(&self, run_id: i64) -> Result<Option<TrackingReport>> {
        tracking_report(&self.store, run_id, self.scale_density_g_per_ml)
    }
}

/// The real balance: the configured port behind its poll thread.
fn open_scale(cfg: &ScaleConfig) -> Option<Box<dyn Transport + Send>> {
    scale::open_polled(cfg).map(|s| Box::new(s) as Box<dyn Transport + Send>)
}

/// The events the tracking chart marks.
const MARKER_KINDS: &[&str] = &[
    "trim_start",
    "trim_ratio",
    "alarm_feed_stopped",
    "alarm_saturated",
    "alarm_wrong_side",
    "alarm_cleared",
    "refill",
];

/// [`Engine::tracking_report`] from any connection to the journal: the API
/// builds it on its own, off the control thread. `rho` is the feed density
/// (g/mL) the volumes are counted in.
pub fn tracking_report(store: &Store, run_id: i64, rho: f64) -> Result<Option<TrackingReport>> {
    let Some(run) = store.run(run_id)? else {
        return Ok(None);
    };
    // The density the run counted with, when it was recorded: a change in
    // Settings since must not rewrite the volumes of past runs.
    let rho = run.density_g_per_ml.filter(|d| d.is_finite() && *d > 0.0).unwrap_or(rho);
    let ticks = store.delivery_samples(run_id, 1999)?; // + the latest tick: <= 2000
    if ticks.len() < 2 {
        return Ok(None);
    }
    let ml_per_unit = match (run.control_var, run.tubing_calibration_id) {
        (ControlVar::Rpm, Some(cal_id)) => store
            .calibration(cal_id)?
            .map_or(1.0, |c| calibration_ml_per_rpm(&c)),
        _ => 1.0,
    };
    let sampled = ticks;
    let mut required_ml = Vec::with_capacity(sampled.len());
    let (mut acc, mut prev) = (0.0, 0.0);
    for &(t, _) in &sampled {
        acc += integrate_curve_mass(&run.curve, prev, t, rho, ml_per_unit) / rho;
        required_ml.push(acc);
        prev = t;
    }
    let delivered_ml: Vec<f64> = sampled.iter().map(|&(_, d)| d / rho).collect();
    let req_end = required_ml.last().copied().unwrap_or(0.0);
    let del_end = delivered_ml.last().copied().unwrap_or(0.0);
    let mu_requested = tracking::requested_mu_per_hour(&run.curve);
    let mu_delivered = mu_requested.and_then(|mu| {
        let pts: Vec<(f64, f64)> = sampled
            .iter()
            .zip(&delivered_ml)
            .map(|(&(t, _), &v)| (t / 3600.0, v))
            .collect();
        tracking::fit_exponential_mu(&pts, mu)
    });
    let events = store.events_of_kinds(run_id, MARKER_KINDS)?;
    let at = |e: &EventRow| e.wall_time.duration_since(run.started_at).as_secs_f64().max(0.0);
    let markers = events.iter().map(|e| TrackMarker { t_s: at(e), kind: e.kind.clone() }).collect();
    let mut refills: Vec<RefillRecord> = events
        .iter()
        .filter(|e| e.kind == "refill")
        .filter_map(|e| {
            let (before_g, after_g) = parse_refill(e.detail.as_deref()?)?;
            Some(RefillRecord { t_s: at(e), before_g, after_g })
        })
        .collect();
    refills.sort_by(|a, b| a.t_s.total_cmp(&b.t_s));
    let (weight_start_g, weight_end_g) = store.weight_bounds(run_id)?;
    Ok(Some(TrackingReport {
        markers,
        weight_start_g,
        weight_end_g,
        refills,
        points: sampled
            .iter()
            .zip(required_ml.iter().zip(&delivered_ml))
            .map(|(&(t, _), (&r, &d))| [t, r, d])
            .collect(),
        r_squared: tracking::r_squared(&required_ml, &delivered_ml),
        deficit_ml: req_end - del_end,
        deficit_pct: (req_end > 0.0).then(|| 100.0 * (req_end - del_end) / req_end),
        mu_requested,
        mu_delivered,
    }))
}

impl<T: Transport> Engine<T> {
    /// Balance read, about once a second from the control loop: keeps the
    /// live weight on screen and notices an unplugged balance between runs.
    /// During a calibration burst it also puts the weight in each journal
    /// row, to see the flow across the burst. Does nothing during any other
    /// run: a trimmed one reads the balance through `scale_tick`, a plain one
    /// leaves it alone so its setpoint writes are never held up.
    pub fn probe_scale(&mut self) {
        let calibrating = self.active.as_ref().is_some_and(|a| a.kind == RunKind::Calibration);
        if self.active.is_none() || calibrating {
            let _ = self.read_scale();
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
        let Some((raw_g, _stable)) = self.read_scale() else {
            return;
        };
        // Everything below reasons about a feed bottle whose weight falls as
        // the pump draws. A balance under the receiving vessel sees the
        // mirror image: flip it here, once, and the refill/touch detection,
        // delivered mass, rate and trim all hold unchanged. The live weight
        // on screen and in the journal stays the real reading.
        let weight_g = raw_g * self.scale_cfg.position.sign();

        // Cumulative masses since run start: what the curve asks for, and what
        // the pump was told (c is constant between two reads).
        let Some(active) = self.active.as_ref() else {
            return;
        };
        let spec = active.spec.clone();
        let t = now.duration_since(active.started_at).as_secs_f64().max(0.0);
        let ml_per_unit = self.rpm_to_ml_min.unwrap_or(1.0);
        let density = self.scale_density_g_per_ml;
        let from = self.tracker.last_t_s.unwrap_or(t);
        let required = integrate_curve_mass(&spec, from, t, density, ml_per_unit);
        self.tracker.required_g += required;
        self.tracker.commanded_g += required * self.trim_c;
        self.tracker.last_t_s = Some(t);

        let prev = self.last_weight_g.unwrap_or(weight_g);
        self.last_weight_g = Some(weight_g);
        let seconds_in_settling = self
            .settling_since
            .map(|s| now.duration_since(s).as_secs_f64())
            .unwrap_or(0.0);
        if self.manual_refill_mode && self.manual_refill_since.is_none() {
            self.manual_refill_since = Some(now);
        }
        if self
            .manual_refill_since
            .is_some_and(|s| now.duration_since(s).as_secs_f64() > MANUAL_REFILL_TIMEOUT_S)
        {
            self.manual_refill_mode = false;
            self.manual_refill_since = None;
            if let Some(run_id) = self.active.as_ref().map(|a| a.id) {
                let _ = self.store.log_event(&NewEvent {
                    run_id: Some(run_id),
                    wall_time: now,
                    level: EventLevel::Warn,
                    kind: "refill_cancelled".into(),
                    detail: Some("refill announced 15 min ago, nothing poured: request dropped".into()),
                });
            }
        }
        self.recent_reads.push_back((
            trim::Read { t_s: t, weight_g, commanded_g: self.tracker.commanded_g },
            None,
        ));
        while self.recent_reads.front().is_some_and(|(r, _)| r.t_s < t - trim::RECENT_READS_S) {
            self.recent_reads.pop_front();
        }
        let reads: Vec<trim::Read> = self.recent_reads.iter().map(|(r, _)| *r).collect();
        let k_level = self.tracker.k.unwrap_or(1.0);
        let resolution_g = self.scale_resolution_g;
        let rise = trim::rise_g(&reads, k_level).map_or(0.0, |(g, _)| g);
        // More than before the refill began: the anchor once refilling, the
        // last Normal read on the tick it starts.
        let base_g = self.tracker.anchor.map(|a| a.weight_g).or(self.tracker.last_weight_g);
        let min_gain_g = trim::REFILL_MIN_GAIN_G.max(2.0 * resolution_g);
        let gained = base_g.is_some_and(|b| weight_g > b + min_gain_g);
        let input = trim::StateInput {
            weight_g,
            prev_weight_g: prev,
            steady: trim::steady(&reads, k_level, resolution_g),
            rise_g: rise,
            gained,
            seconds_in_settling,
            manual_refill_mode: self.manual_refill_mode,
            manual_refill_done: self.manual_refill_done_flag,
            expected_step_g: required * self.trim_c,
        };
        self.manual_refill_done_flag = false;
        let next = trim::next_state(self.scale_state, &input);

        if next == trim::ScaleState::RefillSettling && self.scale_state != trim::ScaleState::RefillSettling {
            self.settling_since = Some(now);
        }
        // A refill begins: keep the last reading before the bottle was
        // touched, in the balance's own sign. Out of a perturbation that is
        // its anchor (a pour that starts with a small step is first taken
        // for a touch), otherwise the previous read.
        let refilling = |s| matches!(s, trim::ScaleState::RefillPending | trim::ScaleState::RefillSettling);
        if refilling(next) && !refilling(self.scale_state) {
            let before = self.tracker.anchor.map_or(prev, |a| a.weight_g);
            self.refill_before_g = Some(before * self.scale_cfg.position.sign());
            self.rewind_slow_rise(k_level);
        }
        // Only a real refill completing resets the cumulative reference. A
        // bump up to 50 g (Perturbation -> Normal) is transient: freeze, don't
        // reset, or a routine touch of the bottle would throw away hours of
        // accumulated balance.
        if next == trim::ScaleState::Normal && self.scale_state == trim::ScaleState::RefillSettling {
            // A manual refill request is one-shot: close it here even if the
            // operator never calls refill_done, or the next tick would go
            // straight back to RefillPending and cycle forever.
            self.manual_refill_mode = false;
            self.manual_refill_since = None;
            // The level jumped: these reads would read as a rise.
            self.recent_reads.clear();
            // No more weight than before: the bottle was lifted and put back,
            // what it lost meanwhile is measured, not estimated.
            if !gained {
                if let Some(a) = self.tracker.anchor.as_mut() {
                    a.refill = false;
                }
                self.refill_before_g = None;
            } else {
                self.refill_weight_g = Some(weight_g);
                self.refill_at = Some(now);
                self.weight_buffer.clear();
            }
            // Journal the bottle weights around the refill (raw readings).
            if let (Some(before), Some(run_id)) =
                (self.refill_before_g.take(), self.active.as_ref().map(|a| a.id))
            {
                let _ = self.store.log_event(&NewEvent {
                    run_id: Some(run_id),
                    wall_time: now,
                    level: EventLevel::Info,
                    kind: "refill".into(),
                    detail: Some(refill_detail(before, raw_g)),
                });
            }
        }
        self.scale_state = next;

        if self.scale_state != trim::ScaleState::Normal {
            let refill = matches!(
                self.scale_state,
                trim::ScaleState::RefillPending | trim::ScaleState::RefillSettling
            );
            let before = self.tracker.anchor;
            let anchor = self.tracker.anchor.get_or_insert(tracking::Anchor {
                weight_g: self.tracker.last_weight_g.unwrap_or(weight_g),
                commanded_g: self.tracker.last_normal_commanded_g,
                refill: false,
                resumed: false,
            });
            anchor.refill |= refill;
            // A touch or a refill moves the weight for reasons of its own:
            // feed-stop detection re-anchors once the balance is Normal again.
            self.tracker.flow_ref = None;
            if self.tracker.anchor != before {
                self.save_trim_state();
            }
            return;
        }

        if self.refill_weight_g.is_none() {
            // Reference for the diagnostic rate below, reset by each refill.
            self.refill_weight_g = Some(weight_g);
            self.refill_at = Some(now);
        }
        let t_refill = self.refill_at.unwrap_or(now);
        let elapsed_since_refill = now.duration_since(t_refill).as_secs_f64().max(0.0);
        self.weight_buffer.push((elapsed_since_refill, weight_g));
        if self.weight_buffer.len() > WEIGHT_BUFFER_CAP {
            self.weight_buffer = trim::decimate(&self.weight_buffer, trim::MAX_SLOPE_POINTS);
        }

        // Delivered mass: measured between consecutive Normal reads, settled
        // from the anchor after a touch, a refill or a restart.
        let bounds = self.trim_bounds();
        let tr = &mut self.tracker;
        // Persisted on the first read, after a settled anchor and with each c
        // update (every UPDATE_EVERY_S), not every second: a resume anchors
        // on the saved weight with the masses saved alongside it, so the
        // downtime is measured whatever the save lag.
        let mut save = tr.anchor.is_some() || tr.last_weight_g.is_none();
        if let Some(a) = tr.anchor.take() {
            // The ratio history is kept: across the gap the count grows by
            // k times the commanded mass, a segment of slope k, neutral for
            // the ratio. c resumes on the settled window, not from scratch.
            // Unless the feed had stopped: then the history is the dry
            // bottle, and the pump starts over on what it does now.
            tr.delivered_g += tracking::settle_anchor(&a, weight_g, tr.commanded_g, tr.k, resolution_g);
            if a.refill && tr.alarm.is_some() {
                tr.points.clear();
            }
        } else if let Some(w0) = tr.last_weight_g {
            tr.delivered_g += w0 - weight_g;
        } else {
            // First read of the run: under a second of flow, counted nominal.
            tr.delivered_g += tr.commanded_g;
        }
        tr.last_weight_g = Some(weight_g);
        tr.last_normal_commanded_g = tr.commanded_g;
        if let Some(last) = self.recent_reads.back_mut() {
            last.1 = Some(tr.delivered_g);
        }
        tr.points.push(tracking::TrackPoint {
            t_s: t,
            commanded_g: tr.commanded_g,
            delivered_g: tr.delivered_g,
        });
        if tr.points.len() > WEIGHT_BUFFER_CAP {
            tr.points = tracking::thin(&tr.points, WEIGHT_BUFFER_CAP);
        }
        tr.flow_ref = Some(tracking::track_flow(tr.flow_ref, t, weight_g, tr.commanded_g, resolution_g));

        if t - tr.last_update_t_s >= tracking::UPDATE_EVERY_S {
            save = true;
            tr.last_update_t_s = t;
            // Diagnostic rate for the status: an O(n^2) slope, so on the
            // update cadence rather than every read.
            self.last_rate_g_per_min = trim::theil_sen_slope(&trim::decimate(
                &self.weight_buffer,
                trim::MAX_SLOPE_POINTS,
            ))
            .map(|s| s * 60.0);
        } else {
            // Not an update tick: nothing below runs.
            if save {
                self.save_trim_state();
            }
            return;
        }
        tr.c_hist.push_back((t, self.trim_c));
        while tr.c_hist.front().is_some_and(|(t0, _)| *t0 < t - tracking::C_HISTORY_S) {
            tr.c_hist.pop_front();
        }
        // An alarm to raise (kind, onset time, detail), applied once the
        // tracker borrow ends: it restores c, so it needs the whole engine.
        let mut raise: Option<(tracking::TrimAlarm, f64, String)> = None;
        if tr.alarm.is_none()
            && tr.delivered_g < -(WRONG_SIDE_FRACTION * tr.required_g).max(WRONG_SIDE_MIN_G)
        {
            // Correcting against a mirrored measurement would drive c to its
            // bound (+25 %). Freeze c, keep accounting, say why.
            tracing::warn!(
                required_g = tr.required_g,
                delivered_g = tr.delivered_g,
                "balance weight moves the wrong way for its configured position; trim frozen"
            );
            tr.wrong_side = true;
            raise = Some((
                tracking::TrimAlarm::WrongSide,
                0.0,
                "balance weight moves the wrong way for its configured position".into(),
            ));
        }
        if raise.is_none()
            && tr.alarm.is_none()
            && tracking::feed_stopped(tr.flow_ref, t, tr.commanded_g, resolution_g)
        {
            let r = tr.flow_ref.expect("feed_stopped implies a reference");
            raise = Some((
                tracking::TrimAlarm::FeedStopped,
                r.t_s,
                format!(
                    "feed stopped: bottle empty or line blocked ({:.1} g commanded in {:.0} s, \
                     weight did not move)",
                    tr.commanded_g - r.commanded_g,
                    t - r.t_s
                ),
            ));
        }
        let rate = spec.value_at(Duration::from_secs_f64(t)) * ml_per_unit * density / 60.0;
        // The two moments the tracking chart marks: c first moves, and the
        // pump ratio is first measured. Journalled once each, as events, so
        // they survive a restart and show in History.
        let mut trim_events: Vec<(&str, String)> = Vec::new();
        let c_before = self.trim_c;
        // An alarm that clears by itself: the pump delivers within bounds
        // again (bottle refilled, tube put back). Judged on the recent window
        // only, the history is still full of the stopped stretch.
        let mut cleared = false;
        if raise.is_none()
            && matches!(tr.alarm, Some(tracking::TrimAlarm::FeedStopped | tracking::TrimAlarm::Saturated))
        {
            if let Some(k) = tracking::recent_ratio(&tr.points, resolution_g) {
                if !tracking::needs_alarm(k, bounds) {
                    let missed = tr.required_g - tr.delivered_g - tr.forgiven_g;
                    tr.forgiven_g += missed;
                    tr.alarm = None;
                    tracking::keep_recent(&mut tr.points, resolution_g);
                    tr.k = Some(k);
                    tr.saturated_updates = 0;
                    tr.saturated_since_t = None;
                    tr.flow_ref = None;
                    cleared = true;
                    trim_events.push((
                        "alarm_cleared",
                        format!(
                            "feed flowing again (pump ratio {k:.3}), regulation resumed at x{:.3}; \
                             {missed:.1} g missed meanwhile are not caught up",
                            self.trim_c
                        ),
                    ));
                }
            }
        }
        if cleared {
            self.scale_ok = true;
        } else if self.scale_ok && raise.is_none() && tr.alarm.is_none() {
            let ratio = tracking::delivery_ratio(&tr.points, resolution_g);
            if ratio.is_none() && tr.k.is_none() {
                // No ratio yet (the first few g of a slow feed). A deficit well
                // past start-up noise means the pump is really off (tube moved,
                // wrong calibration): estimate the ratio from what there is and
                // correct at full speed, as the ratio path would. Otherwise
                // nudge around the starting c on the cumulative deficit.
                let deficit = tr.required_g - tr.delivered_g - tr.forgiven_g;
                let early_k = tracking::deficit_is_significant(deficit, tr.required_g, resolution_g)
                    .then(|| tracking::early_ratio(&tr.points, resolution_g))
                    .flatten();
                if let Some(k) = early_k {
                    self.trim_c = tracking::next_c(self.trim_c, k, deficit, rate, bounds);
                } else if let Some(seed) = tr.c_seed {
                    self.trim_c =
                        tracking::next_c_early(self.trim_c, seed, deficit, rate, resolution_g, bounds);
                }
            } else if let Some(r) = ratio {
                let k = r.k;
                if tr.k.is_none() {
                    trim_events.push((
                        "trim_ratio",
                        format!("pump ratio measured: {k:.3}, regulating on it"),
                    ));
                }
                tr.k = Some(k);
                if tracking::needs_alarm(k, bounds) {
                    tr.saturated_updates += 1;
                    tr.saturated_since_t.get_or_insert(t);
                } else {
                    tr.saturated_updates = 0;
                    tr.saturated_since_t = None;
                }
                if tr.saturated_updates >= tracking::ALARM_AFTER_SATURATED_UPDATES {
                    // The pump can't be brought on track within the c bounds:
                    // c goes back to before it started chasing, frozen.
                    // Accounting goes on for the record.
                    raise = Some((
                        tracking::TrimAlarm::Saturated,
                        tr.saturated_since_t.unwrap_or(t),
                        format!(
                            "pump delivers {:.0} % of its setpoint, beyond the correction limit",
                            100.0 * k
                        ),
                    ));
                } else {
                    let deficit = tr.required_g - tr.delivered_g - tr.forgiven_g;
                    self.trim_c = tracking::next_c_for(self.trim_c, r, deficit, rate, bounds);
                }
            }
        }
        if !tr.start_logged && (self.trim_c - c_before).abs() > 1e-9 {
            tr.start_logged = true;
            trim_events.insert(
                0,
                (
                    "trim_start",
                    format!(
                        "correction starts: x{:.3} ({:+.2} g vs requested)",
                        self.trim_c,
                        tr.delivered_g - tr.required_g
                    ),
                ),
            );
        }
        if let Some(run_id) = self.active.as_ref().map(|a| a.id) {
            for (kind, detail) in trim_events {
                let _ = self.store.log_event(&NewEvent {
                    run_id: Some(run_id),
                    wall_time: now,
                    level: EventLevel::Info,
                    kind: kind.into(),
                    detail: Some(detail),
                });
            }
        }
        if let Some((kind, onset_t, detail)) = raise {
            self.raise_trim_alarm(kind, onset_t, now, detail);
        }
        if save {
            self.save_trim_state();
        }
    }

    /// Stop regulating, say why, and put c back to its value from before the
    /// problem began (`onset_t`), not the one it reached chasing it: a run
    /// left on that alarm, or whose bottle is refilled, then delivers its
    /// last known-good flow instead of overfeeding at the bound. The ratio
    /// history is dropped so the recovery check sees only what follows.
    fn raise_trim_alarm(&mut self, kind: tracking::TrimAlarm, onset_t: f64, now: Timestamp, detail: String) {
        let tr = &mut self.tracker;
        let restored = match kind {
            // Chasing a mirrored deficit since the start: back to the seed.
            tracking::TrimAlarm::WrongSide => tr.c_seed,
            _ => tracking::c_at(&tr.c_hist, onset_t).or(tr.c_seed),
        };
        let before = self.trim_c;
        if let Some(c) = restored {
            self.trim_c = c;
        }
        tr.alarm = Some(kind);
        tr.points.clear();
        tr.k = None;
        tr.saturated_updates = 0;
        tr.saturated_since_t = None;
        self.scale_ok = false;
        tracing::warn!(?kind, before, after = self.trim_c, "trim alarm: {detail}");
        if let Some(run_id) = self.active.as_ref().map(|a| a.id) {
            let _ = self.store.log_event(&NewEvent {
                run_id: Some(run_id),
                wall_time: now,
                level: EventLevel::Error,
                kind: kind.event_kind().into(),
                detail: Some(format!("{detail}; correction frozen at x{:.3} (was x{before:.3})", self.trim_c)),
            });
        }
        self.save_trim_state();
    }

    /// Operator-declared "I'm about to change the bottle": forces the state
    /// machine into `RefillPending` on the next `scale_tick`, ahead of the
    /// automatic weight-jump threshold.
    pub fn trigger_refill_mode(&mut self) {
        self.manual_refill_mode = true;
    }

    /// A refill seen as a rise (a transfer pump, a slow pour) began before
    /// it was recognised: reads under the perturbation limit counted the
    /// gain as negative delivery. Take the count, the ratio history and c
    /// back to the lowest `Normal` read of the rise window, and anchor there.
    fn rewind_slow_rise(&mut self, k: f64) {
        let level = |r: &trim::Read| r.weight_g + k * r.commanded_g;
        let window_from = self.recent_reads.back().map_or(0.0, |(r, _)| r.t_s) - trim::REFILL_RISE_WINDOW_S;
        let normal = self
            .recent_reads
            .iter()
            .filter(|(r, d)| d.is_some() && r.t_s >= window_from);
        let Some(&(low, Some(delivered))) = normal.clone().min_by(|a, b| level(&a.0).total_cmp(&level(&b.0)))
        else {
            return;
        };
        let Some(&(last, _)) = normal.last() else {
            return;
        };
        if level(&last) - level(&low) <= trim::PERTURBATION_MIN_G {
            return; // a sudden pour: the last Normal read is the right anchor
        }
        let tr = &mut self.tracker;
        tr.delivered_g = delivered;
        tr.last_weight_g = Some(low.weight_g);
        tr.last_normal_commanded_g = low.commanded_g;
        tr.points.retain(|p| p.t_s <= low.t_s);
        tr.anchor = None;
        if let Some(c) = tracking::c_at(&tr.c_hist, low.t_s) {
            self.trim_c = c;
        }
        self.refill_before_g = Some(low.weight_g * self.scale_cfg.position.sign());
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

    /// How far the trim may move c, from `[scale] trim_limit_pct`.
    fn trim_bounds(&self) -> tracking::Bounds {
        tracking::Bounds::from_limit_pct(self.scale_cfg.trim_limit_pct)
    }

    /// Change the correction limit under a running trimmed run. c is pulled
    /// into the new bounds at once. An alarm raised because c sat on the old
    /// bound is lifted: the wider limit may cover the pump, and if it does not
    /// the same 5 min of saturation raise it again. A wrong-side alarm stays,
    /// no limit fixes a mirrored measurement.
    fn set_trim_limit_live(&mut self, pct: f64) {
        let old = self.scale_cfg.trim_limit_pct;
        if pct == old {
            return;
        }
        self.scale_cfg.trim_limit_pct = pct;
        self.trim_c = self.trim_bounds().clamp(self.trim_c);
        // Only a saturation alarm is about the limit; a stopped feed or a
        // mirrored balance is not fixed by allowing more correction.
        let lifted = self.tracker.alarm == Some(tracking::TrimAlarm::Saturated);
        let mut missed = 0.0;
        if lifted {
            // Same as a recovery: what the pump fell short by while out of
            // bounds is written off, not paid back in a burst.
            let tr = &mut self.tracker;
            missed = tr.required_g - tr.delivered_g - tr.forgiven_g;
            tr.forgiven_g += missed;
            tr.alarm = None;
            tr.saturated_updates = 0;
            tr.saturated_since_t = None;
            self.scale_ok = true;
        }
        if let Some(run_id) = self.active.as_ref().map(|a| a.id) {
            let _ = self.store.log_event(&NewEvent {
                run_id: Some(run_id),
                wall_time: Timestamp::now(),
                level: EventLevel::Info,
                kind: "trim_limit".into(),
                detail: Some(format!(
                    "correction limit ±{old}% -> ±{pct}%{}",
                    if lifted {
                        format!(", saturation alarm lifted ({missed:.1} g missed meanwhile are not caught up)")
                    } else {
                        String::new()
                    }
                )),
            });
        }
        self.save_trim_state();
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

    /// The balance is attached and answering: not down, and not a reopened
    /// port still waiting for its first weight. What the status frame shows
    /// as connected, and what ends an outage for the control loop.
    pub fn scale_link_up(&self) -> bool {
        !self.scale_link_down() && !self.scale_unconfirmed
    }

    /// Whether the control loop should try to reopen the scale now: it is
    /// configured, its link is down, and either no run is active or the
    /// active run wants the trim. Idle retries matter because a trimmed run
    /// refuses to start without a scale; a plain dosing run is left alone so
    /// reopen attempts never slow its setpoint writes.
    pub fn scale_recovery_wanted(&self) -> bool {
        self.scale_wanted()
            && self.scale_link_down()
            && self.active.as_ref().is_none_or(|a| a.gravimetric_trim)
    }

    /// The scale's counterpart to [`recover_serial`](Self::recover_serial).
    /// First asks the link already held: a balance switched back on answers
    /// there, and one that is merely silent keeps that link (`false`). Only a
    /// broken link (I/O error, stuck poll thread) or none at all is reopened
    /// from scratch. No simulator fallback. Returns `true` when a weight came
    /// back or the port reopened, `false` otherwise; either way the link counts as up only
    /// once a weight arrives ([`scale_link_up`](Self::scale_link_up)): a COM
    /// port opens whether or not a balance answers on it, and treating that
    /// as a recovery made a switched-off balance flap "lost"/"recovered"
    /// every few seconds. Returns `true` without doing anything when no scale
    /// is configured or the link is not down.
    pub fn recover_scale(&mut self) -> bool {
        if !self.scale_wanted() || !self.scale_link_down() {
            return true;
        }
        match self.ask_scale() {
            Some(Ok(r)) => {
                self.took_scale_reading(r);
                return true;
            }
            // A silent balance on a port that works (switched off, its own
            // cable out): the poll thread keeps asking and will see it back.
            // Reopening would only fight that thread for the COM port.
            Some(Err(scale::ScaleError::Transport(fermentool_modbus::TransportError::Timeout))) => {
                return false;
            }
            // No link, a broken port, a stuck poll thread, garbage: reopen.
            _ => {}
        }
        // Let go of the stale link first: a COM port is exclusive on Windows,
        // so opening it again while the old poll thread still holds it fails.
        // The thread closes it asynchronously; if this attempt is too early,
        // the next retry gets the port.
        self.scale = None;
        self.live_weight = None;
        let Some(link) = (self.scale_open)(&self.scale_cfg) else {
            return false;
        };
        self.scale = Some(link);
        // One read decides: a weight confirms the link, a miss puts it back
        // down (without a new five-read streak) for the next retry.
        self.scale_read_fails = REOPEN_AFTER_WRITE_FAILS - 1;
        self.scale_unconfirmed = true;
        true
    }

    /// Apply a `[scale]` section saved from Settings, without a restart. A
    /// density-only change keeps the open link; a new port or baud closes the
    /// old link and opens the new one (an empty port removes the balance).
    /// Refused while a trimmed run is active: its cumulative accounting is
    /// tied to this balance and this density. Returns whether the balance is
    /// connected afterwards; a port that does not open yet is left to the
    /// background retry.
    pub fn reconfigure_scale(&mut self, cfg: ScaleConfig) -> std::result::Result<bool, String> {
        if self.active.as_ref().is_some_and(|a| a.gravimetric_trim) {
            // The port, baud, density and position are what the run's
            // delivered mass is counted with; only the correction limit can
            // change under it.
            let same_accounting = cfg.path.trim() == self.scale_cfg.path.trim()
                && cfg.baud == self.scale_cfg.baud
                && cfg.density_g_per_ml == self.scale_cfg.density_g_per_ml
                && cfg.position == self.scale_cfg.position;
            if !same_accounting {
                return Err("stop the gravimetric run before changing the balance port, baud, \
                            density or position (the correction limit can change during a run)"
                    .into());
            }
            self.set_trim_limit_live(cfg.trim_limit_pct);
            return Ok(!self.scale_link_down());
        }
        let same_link = cfg.path.trim() == self.scale_cfg.path.trim() && cfg.baud == self.scale_cfg.baud;
        if same_link && !self.scale_link_down() {
            self.scale_density_g_per_ml = cfg.density_g_per_ml;
            self.scale_cfg = cfg;
            return Ok(true);
        }
        self.scale = None;
        self.scale_read_fails = 0;
        self.scale_unconfirmed = false;
        self.live_weight = None;
        self.attach_scale(None, cfg);
        if !self.scale_wanted() {
            return Ok(false);
        }
        // No waiting here, on the control thread that also writes the pump
        // (a plain dosing run may be going): the link opens now if it can,
        // the background retry takes it from there, and the balance shows as
        // connected once it answers. The old poll thread lets go of its COM
        // port on its own; an attempt made before that is simply retried.
        self.recover_scale();
        Ok(self.scale_link_up())
    }

    #[cfg(test)]
    pub(crate) fn attach_scale_for_test(&mut self, t: Box<dyn Transport + Send>) {
        self.scale = Some(t);
    }

    #[cfg(test)]
    pub(crate) fn set_scale_opener_for_test(&mut self, open: fn(&ScaleConfig) -> Option<Box<dyn Transport + Send>>) {
        self.scale_open = open;
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

    /// Graceful stop: stop the pump, mark the run `stopped`, or `completed`
    /// when its curve had reached the end (Stop closes the hold phase).
    pub fn stop_run(&mut self, now: Timestamp) -> Result<()> {
        let status = if self.active.as_ref().is_some_and(|a| a.curve_done) {
            RunStatus::Completed
        } else {
            RunStatus::Stopped
        };
        self.end_run(now, status, "stop")
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
        self.stop_or_keep_pending(Some(h.run_id), now);
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
        // Recorded as the operator asked, stopped or not: a Stop that did not
        // get through stays pending, retried and shown.
        self.stop_or_keep_pending(Some(active.id), now);
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
            past_end: run.kind == RunKind::Calibration
                && elapsed_s > run.duration_s as f64 + grace.as_secs_f64(),
            curve_done: elapsed_s >= run.duration_s as f64,
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
        // A dosing run past its curve resumes into its hold phase, like it
        // would have been had the daemon stayed up. A calibration burst past
        // its end is over: its weighing window is gone.
        if run.kind == RunKind::Calibration && elapsed_s > run.duration_s as f64 + grace.as_secs_f64() {
            return Err(EngineError::Config(
                "calibration burst is past its end plus grace; finish or abort instead".into(),
            ));
        }
        // `trim_c` was restored from `app_state` by `Engine::new`; the rpm
        // conversion is not persisted there, rebuild it from the run's own
        // calibration. A calibration deleted since is no reason to refuse a
        // crash resume: the pump keeps its curve, the trim alarms and freezes.
        let mut rpm_to_ml = None;
        let mut drive = None;
        if let Some(cal) = match run.tubing_calibration_id {
            Some(cal_id) => self.store.calibration(cal_id)?,
            None => None,
        } {
            match (run.control_var, cal.control_var) {
                (ControlVar::Rpm, _) => rpm_to_ml = Some(usable_ml_per_rpm(&cal)?),
                (ControlVar::MlMin, ControlVar::Rpm) => drive = Some(usable_ml_per_rpm(&cal)?),
                (ControlVar::MlMin, ControlVar::MlMin) => {}
            }
        }
        self.rpm_to_ml_min = rpm_to_ml;
        self.drive_ml_per_rpm = drive;
        self.drive_paused = false;
        // An alarm saved before the crash still stands: the trim stays frozen
        // until the feed is seen flowing within bounds again.
        if run.gravimetric_trim && self.tracker.alarm.is_some() {
            self.scale_ok = false;
        }
        // The tracker was restored with the trim. Anchor it on the last weight
        // seen before the crash, so the downtime (the pump kept going) is
        // measured by the weight difference at the first read.
        if run.gravimetric_trim && self.tracker.anchor.is_none() {
            if let Some(w) = self.tracker.last_weight_g {
                self.tracker.anchor = Some(tracking::Anchor {
                    weight_g: w,
                    commanded_g: self.tracker.last_normal_commanded_g,
                    refill: false,
                    resumed: true,
                });
            }
        }
        // `trim_c` only applies if this run opted in, same gate as
        // `tick`/`apply_setpoint`.
        let c = if run.gravimetric_trim { self.trim_c } else { 1.0 };
        let target = quantize(
            run.curve.value_at(Duration::from_secs_f64(elapsed_s)) * c,
            drive_grid(run.control_var, self.drive_ml_per_rpm),
        );

        self.log_crash_detected(run.id, now, elapsed_s, run.duration_s)?;

        self.pump.set_direction(run.direction == Direction::Cw)?;
        drive_write(&mut self.pump, &mut self.drive_paused, run.control_var, target, drive)
            .map_err(|e| setpoint_error(e, run.control_var, target, drive))?;
        if !self.drive_paused {
            self.pump.start()?;
        }
        // The resumed run drives the pump a pending Stop was for.
        if self.stop_pending.is_some() {
            self.set_stop_pending(None);
        }

        let next_seq = self.store.last_tick(run.id)?.map_or(0, |t| t.seq + 1);
        let volume_added_ml = if run.control_var == ControlVar::MlMin {
            let start = quantize(
                run.curve.value_at(Duration::ZERO),
                drive_grid(run.control_var, self.drive_ml_per_rpm),
            );
            let horizon = volume_horizon_s(run.kind, run.duration_s);
            Some(self.reconstruct_volume_ml(run.id, start, horizon, elapsed_s, target)?)
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
            name: run.name.clone(),
            curve_done: run.kind == RunKind::Dosing && elapsed_s >= run.duration_s as f64,
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

    /// `[resume] prompt = false`: resume the interrupted dosing run without
    /// asking, once the pump answers a read. `None` while there is nothing to
    /// try (no interrupted run, a run already going, the link down); a
    /// calibration burst is left to the operator, its weighing needs them.
    pub fn try_auto_resume(&mut self, now: Timestamp, grace: Duration) -> Option<Result<i64>> {
        if self.active.is_some() || self.serial_lost || (self.on_simulator() && !self.allow_simulator) {
            return None;
        }
        let run = self.store.running_run().ok().flatten()?;
        if run.kind != RunKind::Dosing {
            return None;
        }
        // An open port is not a live pump: one read first, so a silent pump
        // is waited for instead of failing the resume (and journalling a
        // crash_detected) every few seconds.
        if !self.on_simulator() && self.pump.read_speed_rpm().is_err() {
            return Some(Err(EngineError::SerialDown));
        }
        let resumed = self.resume(now, grace);
        if let Ok(id) = &resumed {
            let _ = self.store.log_event(&NewEvent {
                run_id: Some(*id),
                wall_time: now,
                level: EventLevel::Info,
                kind: "auto_resume".into(),
                detail: Some("resumed without asking (Settings, Crash resume)".into()),
            });
        }
        Some(resumed)
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
        horizon_s: f64,
        resume_elapsed_s: f64,
        resume_target: f64,
    ) -> Result<f64> {
        // Clamp every time bound to `horizon_s` (`volume_horizon_s`), same
        // reasoning as `tick`'s: a calibration burst ends at its duration; a
        // dosing run keeps pumping through its hold phase (infinite horizon).
        let d = horizon_s;
        // Streamed row by row: a 100 h run journals ~360k ticks, loading them
        // all at once cost tens of MB on the control thread.
        let mut total = 0.0;
        let mut prev = (0.0, start);
        self.store.for_each_target(run_id, |elapsed_s, target| {
            let te = elapsed_s.min(d);
            total += volume_slice(prev, (te, target));
            prev = (te, target);
        })?;
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
        self.stop_or_keep_pending(Some(run.id), now);
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

/// Write `value` (in the run's unit) to the pump. `drive`: ml/min per rpm
/// when an ml/min run is driven in rpm (see `Engine::drive_ml_per_rpm`).
/// Driven in rpm, a value under the motor's 0.1 rpm minimum (the pump refuses
/// such a speed, and the curve asks for next to no flow) holds the pump
/// stopped, `paused`, and it starts again once the value rises: a curve that
/// steps down to 0 really stops the feed instead of leaving the pump on its
/// last speed.
fn drive_write<T: Transport>(
    pump: &mut Pump<T>,
    paused: &mut bool,
    control_var: ControlVar,
    value: f64,
    drive: Option<f64>,
) -> std::result::Result<(), PumpError> {
    match (control_var, drive) {
        (ControlVar::MlMin, Some(r)) => {
            let rpm = drive_rpm(value, r);
            if rpm < limits::RPM_MIN - 1e-9 {
                if !*paused {
                    pump.stop()?;
                    *paused = true;
                }
                return Ok(());
            }
            pump.set_speed_rpm(rpm as f32)?;
            if *paused {
                pump.start()?;
                *paused = false;
            }
            Ok(())
        }
        (ControlVar::MlMin, None) => pump.set_flow_ml_min(value as f32),
        (ControlVar::Rpm, _) => pump.set_speed_rpm(value as f32),
    }
}

/// The rpm that delivers `ml_min` at `ml_per_rpm`, on the motor's 0.1 rpm
/// step and within its range.
fn drive_rpm(ml_min: f64, ml_per_rpm: f64) -> f64 {
    quantize(ml_min / ml_per_rpm, setpoint_grid(ControlVar::Rpm)).clamp(0.0, limits::RPM_MAX)
}

/// Classify a failed setpoint write. Only MODBUS `0x03` (illegal *data value*)
/// means the pump received the frame and refused the *value*, the operator
/// actionable case (see [`EngineError::PumpRejectedSetpoint`]). `0x02` (illegal
/// data *address*) is a register-map / firmware mismatch, not something the
/// operator fixes by changing the setpoint, so it stays a plain
/// [`EngineError::Pump`] along with every other failure.
///
/// An ml/min run driven in rpm wrote an rpm: the refusal is reported as such,
/// never pointing at the head/tubing table that mode does not use.
fn setpoint_error(e: PumpError, control_var: ControlVar, value: f64, drive: Option<f64>) -> EngineError {
    let (control_var, value) = match (control_var, drive) {
        (ControlVar::MlMin, Some(r)) => (ControlVar::Rpm, drive_rpm(value, r)),
        _ => (control_var, value),
    };
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
/// In the run's unit: a driven ml/min run reads the speed and converts it.
fn read_actual<T: Transport>(
    pump: &mut Pump<T>,
    control_var: ControlVar,
    drive: Option<f64>,
) -> std::result::Result<f32, PumpError> {
    match (control_var, drive) {
        (ControlVar::MlMin, Some(r)) => pump.read_speed_rpm().map(|v| v * r as f32),
        (ControlVar::MlMin, None) => pump.read_flow_ml_min(),
        (ControlVar::Rpm, _) => pump.read_speed_rpm(),
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

/// The setpoint step in the run's unit: a driven ml/min run moves by whole
/// 0.1 rpm motor steps, so a new value is written only when the speed changes.
fn drive_grid(control_var: ControlVar, drive: Option<f64>) -> f64 {
    match (control_var, drive) {
        (ControlVar::MlMin, Some(r)) => setpoint_grid(ControlVar::Rpm) * r,
        _ => setpoint_grid(control_var),
    }
}

/// ml/min delivered per commanded rpm, from an rpm-mode tubing calibration.
fn calibration_ml_per_rpm(cal: &crate::store::CalibrationRow) -> f64 {
    cal.mean_measured_ml_min / cal.setpoint
}

/// [`calibration_ml_per_rpm`], refused when it is not a positive number (a
/// record edited by hand): it would turn every setpoint into 0 rpm or NaN.
fn usable_ml_per_rpm(cal: &crate::store::CalibrationRow) -> Result<f64> {
    let r = calibration_ml_per_rpm(cal);
    if r.is_finite() && r > 0.0 {
        Ok(r)
    } else {
        Err(EngineError::Config(format!(
            "tubing calibration {} gives no usable ml/min per rpm ({r})",
            cal.id
        )))
    }
}

/// Time since `t`, in whole milliseconds (a simulated pump answers in
/// microseconds: 0, so test timestamps stay exact).
fn whole_ms(t: std::time::Instant) -> SignedDuration {
    SignedDuration::from_millis(t.elapsed().as_millis() as i64)
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
            responsible: None,
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
    /// real closed loop (`scale_tick` -> tracker -> `tracking::next_c`)
    /// rather than a pre-scripted sequence. Returns the highest
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
            position: Default::default(),
            trim_limit_pct: 25.0,
        }
    }

    #[test]
    fn a_routine_perturbation_does_not_reset_the_cumulative_baseline() {
        // "Transient: freeze, don't reset": a bump under 50 g that settles
        // one tick later must not wipe the buffer or the baseline, only a real
        // RefillSettling -> Normal completion may.
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[
            1000.0, 999.0, 998.0, // 3 stable readings, buffer at 3
            1020.0, // +22 g -> Perturbation, buffer untouched
            1020.5, // settles (within the perturbation limit) -> Normal, buffer at 4
        ])));
        e.start_run(trim_cfg(), t0()).unwrap();
        for i in 0..5 {
            e.scale_tick(at(i));
        }
        assert_eq!(e.status().scale_state, Some(trim::ScaleState::Normal));
        assert_eq!(e.weight_buffer_len(), 4);
    }

    #[test]
    fn a_refill_announced_but_never_poured_lapses_after_15_min() {
        // "Refill bottle" pressed, nothing poured, "Done" never pressed: the
        // balance waits for the pour, then gives up, and never cycles
        // Normal -> RefillPending -> Normal, wiping the baseline every lap.
        let spec = CurveSpec::linear(9.0, 9.0, Duration::from_secs(36_000));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let id = e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        e.trigger_refill_mode();
        let mut w = 1_000.0;
        for i in 0..1000 {
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            w -= spec.value_at(Duration::from_secs(i as u64)) / 60.0 * e.trim_c();
            let state = e.status().scale_state;
            if (1..900).contains(&i) {
                assert_eq!(state, Some(trim::ScaleState::RefillPending), "tick {i}");
            } else if i >= 940 {
                assert_eq!(state, Some(trim::ScaleState::Normal), "still cycling at tick {i}");
            }
        }
        assert_eq!(event_times(&e, id, "refill_cancelled").len(), 1);
        assert!(event_times(&e, id, "refill").is_empty(), "nothing was poured");
    }

    #[test]
    fn a_refill_by_transfer_pump_is_seen_and_c_rides_through_it() {
        // 60 ml/min fed by a pump delivering 90 %, c settled; a transfer pump
        // adds 1.5 L at 5 g/s. Its ~4 g net per read is under the 5 g
        // perturbation limit at this flow: without the rise test it reads as
        // flow running backwards.
        let spec = CurveSpec::linear(60.0, 60.0, Duration::from_secs(36_000));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let id = e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        let k = 0.9;
        let mut w = 2_000.0;
        let mut truth = 0.0;
        let mut cs = Vec::new();
        for i in 0..4_000i64 {
            if (1_800..2_100).contains(&i) {
                w += 5.0;
            }
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            cs.push(e.trim_c());
            let out = k * spec.value_at(Duration::from_secs(i as u64)) / 60.0 * e.trim_c();
            w -= out;
            truth += out;
        }
        let refills: Vec<(f64, f64)> = e
            .store()
            .events(Some(id), 10_000)
            .unwrap()
            .into_iter()
            .filter(|ev| ev.kind == "refill")
            .filter_map(|ev| parse_refill(ev.detail.as_deref()?))
            .collect();
        assert_eq!(refills.len(), 1, "{refills:?}");
        // 1.5 L poured, ~350 g drawn by the feed (1 g/s) while it lasted
        // and settled.
        let gain = refills[0].1 - refills[0].0;
        assert!((gain - 1_150.0).abs() < 40.0, "{refills:?}");
        let before = cs[1_790];
        let swing = cs[1_790..].iter().map(|c| (c - before).abs()).fold(0.0, f64::max);
        assert!(swing < 0.01, "c moved {swing} around the refill (was {before})");
        let st = e.status();
        let tr = st.tracking.unwrap();
        assert!(st.scale_ok && tr.alarm.is_none(), "{tr:?}");
        assert!((tr.delivered_ml - truth).abs() < 0.01 * truth, "{} vs {truth}", tr.delivered_ml);
    }

    #[test]
    fn a_bottle_lifted_and_put_back_is_not_a_refill() {
        let spec = CurveSpec::linear(9.0, 9.0, Duration::from_secs(36_000));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let id = e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        let mut w = 1_000.0;
        let mut truth = 0.0;
        for i in 0..1_200i64 {
            let lifted = (600..640).contains(&i);
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[if lifted { 0.0 } else { w }])));
            e.scale_tick(at(i));
            let out = 0.9 * spec.value_at(Duration::from_secs(i as u64)) / 60.0 * e.trim_c();
            w -= out;
            truth += out;
        }
        assert!(event_times(&e, id, "refill").is_empty(), "a lift is not a refill");
        let tr = e.status().tracking.unwrap();
        // Measured across the lift, not estimated.
        assert!((tr.delivered_ml - truth).abs() < 0.5, "{} vs {truth}", tr.delivered_ml);
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
        assert_eq!(st.scale_connected, Some(false));
        assert!(st.scale_state.is_some());
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
    fn an_idle_probe_reports_the_live_weight() {
        let mut e = engine();
        e.attach_scale(
            Some(Box::new(ScriptedScale::new(&[742.5]))),
            scale_cfg("sim-scale"),
        );
        assert_eq!(e.status().scale_weight_g, None, "nothing read yet");
        e.probe_scale();
        let st = e.status();
        assert_eq!(st.scale_weight_g, Some(742.5));
        assert_eq!(st.scale_stable, Some(true));
    }

    #[test]
    fn idle_probes_notice_an_unplugged_balance() {
        let mut e = engine();
        e.attach_scale(
            Some(Box::new(ScriptedScale::new(&[742.5]))), // then times out
            scale_cfg("sim-scale"),
        );
        e.probe_scale();
        for _ in 0..REOPEN_AFTER_WRITE_FAILS {
            e.probe_scale();
        }
        let st = e.status();
        assert_eq!(st.scale_connected, Some(false));
        assert_eq!(st.scale_weight_g, None, "a stale weight must not look live");
    }

    #[test]
    fn the_idle_probe_does_nothing_while_a_run_is_active() {
        // Mid-run the balance is read by scale_tick (trimmed runs) or not at
        // all (plain runs), never by the idle probe.
        let mut e = engine();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        e.attach_scale(
            Some(Box::new(CountingFailingScale { calls: calls.clone() })),
            scale_cfg("sim-scale"),
        );
        e.start_run(linear_cfg(), t0()).unwrap();
        e.probe_scale();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn a_calibration_burst_journals_the_balance_every_second() {
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[700.0, 699.5, 699.0])));
        let cfg = RunConfig {
            kind: RunKind::Calibration,
            curve: CurveSpec::linear(50.0, 50.0, Duration::from_secs(300)),
            ..linear_cfg()
        };
        let id = e.start_run(cfg, t0()).unwrap();
        for i in 1..=3 {
            e.probe_scale();
            e.tick(at(i)).unwrap();
        }
        let w: Vec<Option<f64>> = e.store().ticks(id, 0, 10).unwrap().iter().map(|t| t.weight_g).collect();
        assert_eq!(w, [Some(700.0), Some(699.5), Some(699.0)]);
    }

    #[test]
    fn a_trimmed_run_keeps_the_live_weight_current() {
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[1000.0, 999.0])));
        e.start_run(flat_60_ml_min_cfg(), t0()).unwrap();
        e.scale_tick(t0());
        e.scale_tick(at(1));
        assert_eq!(e.status().scale_weight_g, Some(999.0));
    }

    #[test]
    fn a_scale_down_at_boot_is_retried_while_idle_not_only_during_a_trim_run() {
        // A trimmed run can't start without a scale, so if recovery only ran
        // during one, a scale that missed boot would never come back.
        let mut e = engine();
        assert!(!e.scale_recovery_wanted(), "nothing configured, nothing to retry");
        e.attach_scale(None, scale_cfg("NOPE_NOT_A_REAL_PORT_99999"));
        assert!(e.scale_recovery_wanted(), "idle with a configured scale down");
        // A plain dosing run doesn't need the scale: don't block its control
        // thread on reopen attempts.
        e.start_run(linear_cfg(), t0()).unwrap();
        assert!(!e.scale_recovery_wanted());
        e.stop_run(at(1)).unwrap();
        // A healthy link needs nothing.
        e.attach_scale(Some(Box::new(ScriptedScale::new(&[]))), scale_cfg("sim-scale"));
        assert!(!e.scale_recovery_wanted());
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

    /// Always fails with an I/O error (the adapter is gone); flags when the
    /// engine lets go of it (the poll thread closing its COM port, for real).
    struct DropFlagScale(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl Transport for DropFlagScale {
        fn transaction(&mut self, _: &[u8]) -> std::result::Result<Vec<u8>, fermentool_modbus::TransportError> {
            Err(fermentool_modbus::TransportError::Io("device removed".into()))
        }
    }
    impl Drop for DropFlagScale {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn recover_scale_releases_a_broken_link_before_reopening_the_port() {
        // A COM port is exclusive on Windows: reopening it while the stale
        // link still holds it fails every time, so an idle balance that
        // missed a few reads stayed "disconnected" for good.
        let mut e = engine();
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        e.attach_scale(
            Some(Box::new(DropFlagScale(dropped.clone()))),
            scale_cfg("NOPE_NOT_A_REAL_PORT_99999"),
        );
        for _ in 0..REOPEN_AFTER_WRITE_FAILS {
            e.probe_scale();
        }
        assert!(e.scale_link_down());
        assert!(!e.recover_scale());
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst), "stale link still holds the port");
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

    /// A balance switched off: its COM port (the USB adapter) still opens.
    fn open_silent(_: &ScaleConfig) -> Option<Box<dyn Transport + Send>> {
        Some(Box::new(ScriptedScale::new(&[])))
    }

    fn open_answering(_: &ScaleConfig) -> Option<Box<dyn Transport + Send>> {
        Some(Box::new(ScriptedScale::new(&[640.0, 640.0, 640.0])))
    }

    fn open_never(_: &ScaleConfig) -> Option<Box<dyn Transport + Send>> {
        panic!("nothing to reopen here")
    }

    /// A link whose port broke (adapter pulled).
    fn broken() -> Box<dyn Transport + Send> {
        Box::new(DropFlagScale(Default::default()))
    }

    fn take_down(e: &mut Engine<SimPump>) {
        for _ in 0..REOPEN_AFTER_WRITE_FAILS {
            e.probe_scale();
        }
        assert!(e.scale_link_down());
    }

    #[test]
    fn a_reopened_port_is_no_balance_until_a_weight_comes_back() {
        // The bug: a reopen counted as a recovery, so a switched-off balance
        // read "connected", then failed five fresh reads, every few seconds.
        let mut e = engine();
        e.attach_scale(Some(broken()), scale_cfg("COM-silent"));
        e.set_scale_opener_for_test(open_silent);
        take_down(&mut e);
        assert!(e.recover_scale(), "the port reopened");
        assert!(!e.scale_link_down(), "one read is allowed on the new link");
        assert!(!e.scale_link_up());
        assert_eq!(e.status().scale_connected, Some(false), "not shown connected on an open port");
        e.probe_scale();
        assert!(e.scale_link_down(), "a single miss puts it back down, no new five-read streak");
        assert!(e.scale_recovery_wanted());
        // Still silent: the link is kept for its poll thread, not reopened.
        e.set_scale_opener_for_test(open_never);
        assert!(!e.recover_scale());
    }

    #[test]
    fn a_silent_balance_keeps_its_link() {
        // Switched off: the port is fine. Reopening it would fight the poll
        // thread (still holding it) for the COM port on every retry.
        let mut e = engine();
        e.attach_scale(Some(Box::new(ScriptedScale::new(&[]))), scale_cfg("COM-off"));
        e.set_scale_opener_for_test(open_never);
        take_down(&mut e);
        assert!(!e.recover_scale());
        assert!(e.scale_link_down());
        assert_eq!(e.status().scale_connected, Some(false));
    }

    #[test]
    fn a_reopened_port_that_answers_is_the_balance_back() {
        let mut e = engine();
        e.attach_scale(Some(broken()), scale_cfg("COM-back"));
        e.set_scale_opener_for_test(open_answering);
        take_down(&mut e);
        assert!(e.recover_scale());
        e.probe_scale();
        assert!(e.scale_link_up());
        let st = e.status();
        assert_eq!(st.scale_connected, Some(true));
        assert_eq!(st.scale_weight_g, Some(640.0));
    }

    #[test]
    fn a_balance_back_on_the_held_link_needs_no_reopen() {
        // Switched off, then on again: the link held all along answers.
        fn never_called(_: &ScaleConfig) -> Option<Box<dyn Transport + Send>> {
            panic!("the held link answered, nothing to reopen")
        }
        let mut e = engine();
        let mut replies: std::collections::VecDeque<_> =
            (0..REOPEN_AFTER_WRITE_FAILS).map(|_| Err(fermentool_modbus::TransportError::Timeout)).collect();
        replies.push_back(Ok(b"S S      512.0 g\r\n".to_vec()));
        e.attach_scale(Some(Box::new(ScriptedScale { replies })), scale_cfg("COM-held"));
        e.set_scale_opener_for_test(never_called);
        take_down(&mut e);
        assert!(e.recover_scale());
        assert!(e.scale_link_up());
        assert_eq!(e.status().scale_weight_g, Some(512.0));
    }

    // ---- scale settings from the UI ----

    #[test]
    fn configuring_a_scale_from_settings_shows_it_without_a_restart() {
        let mut e = engine();
        assert_eq!(e.status().scale_state, None, "fresh install: no balance");
        let connected = e.reconfigure_scale(scale_cfg("NOPE_NOT_A_REAL_PORT_99999")).unwrap();
        assert!(!connected);
        let st = e.status();
        assert_eq!(st.scale_connected, Some(false));
        assert!(st.scale_state.is_some());
        assert!(e.scale_recovery_wanted(), "the background retry takes over");
    }

    #[test]
    fn clearing_the_scale_port_removes_the_balance() {
        let mut e = engine();
        e.attach_scale(Some(Box::new(ScriptedScale::new(&[1000.0]))), scale_cfg("sim-scale"));
        e.reconfigure_scale(ScaleConfig::default()).unwrap();
        let st = e.status();
        assert_eq!(st.scale_state, None);
        assert_eq!(st.scale_connected, None);
        assert!(!e.scale_recovery_wanted());
    }

    #[test]
    fn a_density_change_keeps_the_open_link() {
        let mut e = engine();
        e.attach_scale(Some(Box::new(ScriptedScale::new(&[742.5]))), scale_cfg("sim-scale"));
        let mut cfg = scale_cfg("sim-scale");
        cfg.density_g_per_ml = 1.18;
        assert!(e.reconfigure_scale(cfg).unwrap());
        e.probe_scale();
        assert_eq!(e.status().scale_weight_g, Some(742.5), "same scripted link, not reopened");
    }

    #[test]
    fn the_scale_cannot_be_changed_during_a_trimmed_run() {
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[1000.0; 10])));
        e.start_run(flat_60_ml_min_cfg(), t0()).unwrap();
        // Another port (or density, or position) would change how the run's
        // delivered mass is counted: refused.
        assert!(e.reconfigure_scale(scale_cfg("COM99")).is_err());
        // Only the correction limit may change under a running trim.
        let limit_only = ScaleConfig { trim_limit_pct: 40.0, ..ScaleConfig::default() };
        assert!(e.reconfigure_scale(limit_only).is_ok());
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
                    responsible: None,
                    density_g_per_ml: None,
                })
                .unwrap();
            store
                .finish_run(*run, RunStatus::Stopped, started + SignedDuration::from_mins(5))
                .unwrap();
        }
        store
            .insert_calibration(&crate::store::NewCalibration {
                tubing_lot_id: "LOT".into(),
                internal_ref: None,
                inner_diameter_mm: 1.6,
                outer_diameter_mm: 4.8,
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
            responsible: None,
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
        assert_eq!(e.trim_c(), trim::TRIM_MAX); // the default ±25 % limit
    }

    /// Simulate `secs` one-second balance reads of a bottle drained by a pump
    /// that delivers `k(t)` times what it is commanded, the command being the
    /// run's own curve times the engine's current c. The balance reports 0.1 g
    /// steps (ScriptedScale's format). Returns the true delivered grams.
    fn simulate(
        e: &mut Engine<SimPump>,
        spec: &CurveSpec,
        k: impl Fn(f64) -> f64,
        start_g: f64,
        from_s: i64,
        secs: i64,
    ) -> f64 {
        let mut w = start_g;
        let mut delivered = 0.0;
        for i in from_s..from_s + secs {
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            let rate = spec.value_at(Duration::from_secs(i as u64)) / 60.0; // g/s at 1 g/mL
            let out = k(i as f64) * rate * e.trim_c();
            w -= out;
            delivered += out;
        }
        delivered
    }

    #[test]
    fn delivered_mass_tracks_an_exponential_curve_to_r2_one() {
        // 1 ml/min, mu = 0.15/h, 6 h; the pump starts at 85 % and drifts to 92 %.
        let spec = CurveSpec::exponential_physio(1.0, 0.15, Duration::from_secs(6 * 3600));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let cfg = RunConfig { curve: spec.clone(), ..trim_cfg() };
        e.start_run(cfg, t0()).unwrap();
        let k = |t: f64| 0.85 + 0.07 * t / (6.0 * 3600.0);
        let truth = simulate(&mut e, &spec, k, 5_000.0, 0, 6 * 3600);
        let st = e.status();
        let tr = st.tracking.expect("tracking status during a trimmed run");
        assert!(st.scale_ok, "no alarm expected");
        // The engine's own count agrees with the truth to the balance step.
        assert!((tr.delivered_ml - truth).abs() < 0.5, "{} vs {truth}", tr.delivered_ml);
        // And the cumulative delivered volume matches the curve.
        let pct = tr.deficit_pct.unwrap();
        assert!(pct.abs() < 1.0, "final deficit {pct} %");
        // c sits on the pump's final inverse ratio.
        assert!((e.trim_c() - 1.0 / 0.92).abs() < 0.03, "c = {}", e.trim_c());
    }

    #[test]
    fn a_linear_curve_from_zero_is_tracked_too() {
        let spec = CurveSpec::linear(0.0, 10.0, Duration::from_secs(3 * 3600));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        simulate(&mut e, &spec, |_| 0.9, 5_000.0, 0, 3 * 3600);
        let pct = e.status().tracking.unwrap().deficit_pct.unwrap();
        assert!(pct.abs() < 1.0, "final deficit {pct} %");
    }

    #[test]
    fn a_refill_keeps_the_cumulative_count() {
        let spec = CurveSpec::linear(60.0, 60.0, Duration::from_secs(36_000));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        let mut truth = simulate(&mut e, &spec, |_| 0.9, 2_000.0, 0, 1800);
        // Bottle swapped: +1500 g, then the run goes on from the new weight.
        let w_after = 2_000.0 - truth + 1_500.0;
        truth += simulate(&mut e, &spec, |_| 0.9, w_after, 1800, 1800);
        let tr = e.status().tracking.unwrap();
        // The refill itself is estimated from k, so allow a few grams.
        assert!((tr.delivered_ml - truth).abs() < 5.0, "{} vs {truth}", tr.delivered_ml);
        assert!(tr.deficit_pct.unwrap().abs() < 1.0);
    }

    #[test]
    fn a_slow_run_is_corrected_before_its_ratio_window_is_full() {
        // 2 ml/min on a pump delivering 90 %: the ratio needs 5 g (~2.5 min),
        // the deficit term must not wait for it.
        let spec = CurveSpec::linear(2.0, 2.0, Duration::from_secs(3600));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        let d1 = simulate(&mut e, &spec, |_| 0.9, 470.0, 0, 140);
        assert!(e.status().tracking.unwrap().delivery_ratio.is_none(), "window not full yet");
        assert!(e.trim_c() > 1.003, "already correcting: c = {}", e.trim_c());
        simulate(&mut e, &spec, |_| 0.9, 470.0 - d1, 140, 1060);
        let tr = e.status().tracking.unwrap();
        assert!(tr.deficit_pct.unwrap().abs() < 2.0, "{tr:?}");
        assert!((e.trim_c() - 1.0 / 0.9).abs() < 0.03, "c = {}", e.trim_c());
    }

    /// Run 39 replayed: the tube moved in the head, the pump delivers 78 % of
    /// its setpoint from the start, 9 ml/min, no calibration (c starts at 1).
    fn moved_tube_run(limit_pct: f64) -> (Engine<SimPump>, CurveSpec) {
        let spec = CurveSpec::linear(9.0, 9.0, Duration::from_secs(3600));
        let mut e = engine();
        e.attach_scale(
            Some(Box::new(ScriptedScale::new(&[]))),
            ScaleConfig { trim_limit_pct: limit_pct, ..scale_cfg("sim-scale") },
        );
        e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        (e, spec)
    }

    /// Run 48 replayed: 0.9 ml/min on a pump delivering 92 %; the bottle
    /// runs dry at `dry_at` s, is refilled (+300 g) at `refill_at` s, and the
    /// run goes on to `end` s. Returns (engine, run id, c every second).
    fn bottle_runs_dry(dry_at: i64, refill_at: i64, end: i64) -> (Engine<SimPump>, i64, Vec<f64>) {
        let spec = CurveSpec::linear(0.9, 0.9, Duration::from_secs(36_000));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let id = e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        let mut w = 450.0;
        let mut cs = Vec::new();
        for i in 0..end {
            if i == refill_at {
                w += 300.0;
            }
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            cs.push(e.trim_c());
            if !(dry_at..refill_at).contains(&i) {
                w -= 0.92 * spec.value_at(Duration::from_secs(i as u64)) / 60.0 * e.trim_c();
            }
        }
        (e, id, cs)
    }

    fn event_times(e: &Engine<SimPump>, id: i64, kind: &str) -> Vec<i64> {
        let mut v: Vec<i64> = e
            .store()
            .events(Some(id), 10_000)
            .unwrap()
            .into_iter()
            .filter(|ev| ev.kind == kind)
            .map(|ev| ev.wall_time.duration_since(t0()).as_secs())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn an_empty_bottle_is_called_out_fast_and_c_goes_back_to_before_it() {
        let (dry_at, refill_at) = (1500, 2700);
        let (e, id, cs) = bottle_runs_dry(dry_at, refill_at, refill_at);
        let alarm = event_times(&e, id, "alarm_feed_stopped");
        assert_eq!(alarm.len(), 1, "one alarm");
        // Run 48 took 9 min to alarm, as a saturation; now ~3 min, by name.
        assert!(alarm[0] - dry_at <= 240, "alarm {} s after the bottle ran dry", alarm[0] - dry_at);
        let st = e.status();
        assert_eq!(st.tracking.as_ref().unwrap().alarm, Some(tracking::TrimAlarm::FeedStopped));
        assert!(!st.scale_ok);
        // c is back to where it was when the feed last flowed (not where the
        // chase took it), and stays there while the alarm stands.
        let before = cs[dry_at as usize];
        let chased = cs[dry_at as usize..alarm[0] as usize].iter().cloned().fold(0.0, f64::max);
        let frozen = cs[(alarm[0] + 1) as usize];
        assert!((frozen - before).abs() < 0.011, "frozen at {frozen}, was {before} when the bottle ran dry");
        assert!(frozen < chased, "restored below the chase ({chased})");
        assert!(cs[(alarm[0] + 1) as usize..].iter().all(|&c| c == frozen), "c moved during the alarm");
        assert!((before - 1.0 / 0.92).abs() < 0.03, "c before = {before}");
    }

    #[test]
    fn a_refill_is_journalled_with_the_bottle_weights_around_it() {
        // The dry bottle stood at 450 - ~22.4 g; refilled +300 g at 2700 s.
        let (dry_at, refill_at, end) = (1500, 2700, 3000);
        let (mut e, id, _) = bottle_runs_dry(dry_at, refill_at, end);
        for i in 0..3 {
            e.tick(at(end + i)).unwrap(); // a few journal rows for the report
        }
        let report = e.tracking_report(id).unwrap().expect("a trimmed run has a report");
        assert_eq!(report.refills.len(), 1, "{:?}", report.refills);
        let r = &report.refills[0];
        // "After" is read once the balance has settled (20 s still, then
        // 10 s), the pump drawing meanwhile: about 0.5 g under the 300 poured.
        assert!((r.after_g - r.before_g - 300.0).abs() < 1.0, "{r:?}");
        assert!(r.before_g < 430.0 && r.before_g > 420.0, "{r:?}");
        assert!(report.markers.iter().any(|m| m.kind == "refill"));
        assert_eq!(parse_refill(&refill_detail(413.5, 753.5)), Some((413.5, 753.5)));
    }

    #[test]
    fn a_pour_that_starts_with_a_small_step_journals_the_weight_before_it() {
        // Run 65: 564.9 g, then +11 g and +28 g (a touch, by size), then the
        // rest of the pour. The refill's "before" is 564.9, not 603.8.
        let spec = CurveSpec::linear(9.0, 9.0, Duration::from_secs(36_000));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let id = e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        let mut w = 600.0;
        let mut before = 0.0;
        for i in 0..200i64 {
            match i {
                60 => {
                    before = w;
                    w += 11.0;
                }
                61 => w += 28.0,
                62..=66 => w += 25.0,
                _ => {}
            }
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            w -= 0.15 * e.trim_c();
        }
        let refills: Vec<(f64, f64)> = e
            .store()
            .events(Some(id), 100)
            .unwrap()
            .into_iter()
            .filter(|ev| ev.kind == "refill")
            .filter_map(|ev| parse_refill(ev.detail.as_deref()?))
            .collect();
        assert_eq!(refills.len(), 1, "{refills:?}");
        assert!((refills[0].0 - before).abs() < 0.5, "before {} vs {before}", refills[0].0);
    }

    #[test]
    fn after_a_refill_the_regulation_resumes_without_a_catch_up_burst() {
        let (dry_at, refill_at, end) = (1500, 2700, 4500);
        let (e, id, cs) = bottle_runs_dry(dry_at, refill_at, end);
        let cleared = event_times(&e, id, "alarm_cleared");
        assert_eq!(cleared.len(), 1, "cleared once");
        assert!(cleared[0] > refill_at && cleared[0] - refill_at <= 600, "cleared {} s after the refill", cleared[0] - refill_at);
        let st = e.status();
        let tr = st.tracking.unwrap();
        assert!(st.scale_ok && tr.alarm.is_none());
        // The ~18 g missed while dry are written off, not paid back: c never
        // shoots up after the refill.
        let missed = 0.9 * (refill_at - dry_at) as f64 / 60.0;
        assert!((tr.missed_ml - missed).abs() < 4.0, "missed {} vs ~{missed}", tr.missed_ml);
        let peak = cs[refill_at as usize..].iter().cloned().fold(0.0, f64::max);
        assert!(peak < 1.0 / 0.92 + 0.04, "c peaked at {peak} after the refill");
        assert!((e.trim_c() - 1.0 / 0.92).abs() < 0.03, "c = {}", e.trim_c());
    }

    #[test]
    fn a_feed_stop_alarm_survives_a_restart() {
        let db = TempDb::new();
        let spec = CurveSpec::linear(0.9, 0.9, Duration::from_secs(36_000));
        {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            a.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
            a.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
            let mut w = 450.0;
            for i in 0..900i64 {
                a.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
                a.scale_tick(at(i));
                a.tick(at(i)).unwrap();
                if i < 300 {
                    w -= 0.9 / 60.0 * a.trim_c();
                }
            }
            assert_eq!(a.status().tracking.unwrap().alarm, Some(tracking::TrimAlarm::FeedStopped));
        }
        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        b.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        b.resume(at(960), grace()).unwrap();
        let st = b.status();
        assert!(!st.scale_ok, "still frozen after the restart");
        assert_eq!(st.tracking.unwrap().alarm, Some(tracking::TrimAlarm::FeedStopped));
    }

    #[test]
    fn a_large_offset_is_corrected_at_full_speed_before_the_ratio_window() {
        let (mut e, spec) = moved_tube_run(25.0);
        simulate(&mut e, &spec, |_| 0.78, 780.0, 0, 115);
        assert!(e.status().tracking.unwrap().delivery_ratio.is_none(), "window not full yet");
        // The early nudge alone reached ~1.07 by now (run 39: 1.071 at 2 min).
        assert!(e.trim_c() > 1.12, "c = {}", e.trim_c());
    }

    #[test]
    fn a_wider_limit_lets_the_trim_catch_a_tube_the_default_cannot() {
        // c needs 1/0.78 = 1.28: past the default 1.25, inside ±40 %.
        let (mut e, spec) = moved_tube_run(40.0);
        simulate(&mut e, &spec, |_| 0.78, 780.0, 0, 900);
        let st = e.status();
        assert!(st.scale_ok, "no alarm inside ±40 %");
        assert!((e.trim_c() - 1.0 / 0.78).abs() < 0.03, "c = {}", e.trim_c());
        assert!(st.tracking.unwrap().deficit_pct.unwrap().abs() < 2.0);

        // The default ±25 % cannot cover it: after 5 min out of bounds it
        // alarms, with c put back to where it stood when that began, not
        // left pinned at 1.25 to overfeed once the tube is fixed.
        let (mut e, spec) = moved_tube_run(25.0);
        simulate(&mut e, &spec, |_| 0.78, 780.0, 0, 900);
        let st = e.status();
        assert!(!st.scale_ok);
        assert_eq!(st.tracking.unwrap().alarm, Some(tracking::TrimAlarm::Saturated));
        assert!(e.trim_c() < 1.25, "c = {}", e.trim_c());
    }

    #[test]
    fn the_limit_can_be_widened_mid_run_and_lifts_a_saturation_alarm() {
        let (mut e, spec) = moved_tube_run(25.0);
        let d1 = simulate(&mut e, &spec, |_| 0.78, 780.0, 0, 900);
        assert!(!e.status().scale_ok, "pinned at 1.25, alarmed");

        // Anything that changes the accounting is still refused mid-run...
        let cfg = |pct: f64, density: f64| ScaleConfig {
            trim_limit_pct: pct,
            density_g_per_ml: density,
            ..scale_cfg("sim-scale")
        };
        assert!(e.reconfigure_scale(cfg(25.0, 1.1)).is_err());
        assert!(!e.status().scale_ok);
        // ...the limit is not.
        assert!(e.reconfigure_scale(cfg(40.0, 1.0)).is_ok());
        assert!(e.status().scale_ok, "the wider limit lifts the saturation alarm");

        simulate(&mut e, &spec, |_| 0.78, 780.0 - d1, 900, 900);
        let st = e.status();
        assert!(st.scale_ok, "covered by ±40 %, no new alarm");
        assert!((e.trim_c() - 1.0 / 0.78).abs() < 0.03, "c = {}", e.trim_c());

        // Narrowing pulls c in at once.
        assert!(e.reconfigure_scale(cfg(10.0, 1.0)).is_ok());
        assert!((e.trim_c() - 1.10).abs() < 1e-9, "c = {}", e.trim_c());
    }

    #[test]
    fn the_start_up_lag_does_not_trigger_the_full_speed_early_path() {
        // 60 ml/min, an exact pump, but nothing arrives for the first 2 s:
        // 2 g behind is "significant" by size, yet the pump is on spec, and
        // c must not run off on it.
        let spec = CurveSpec::linear(60.0, 60.0, Duration::from_secs(3600));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        let mut w = 2_000.0;
        for i in 0..120i64 {
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            if i >= 2 {
                w -= 1.0 * e.trim_c();
            }
            assert!((e.trim_c() - 1.0).abs() < 0.02, "t = {i}: c = {}", e.trim_c());
        }
    }

    #[test]
    fn the_trim_settles_instead_of_hunting_on_a_noisy_slow_feed() {
        // Run 19's conditions: 2 ml/min, a 0.1 g balance, a pump at ~90 %
        // whose per-second output pulses ±60 % (rollers, drops). The
        // cumulative was always fine; what must hold now is the instantaneous
        // flow: with a 5 g ratio window for the whole run, c swung ±10 % in
        // minutes and the real flow per 2 min ranged 75..140 %.
        let spec = CurveSpec::linear(2.0, 2.0, Duration::from_secs(3600));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        let (mut w, mut truth): (f64, f64) = (470.0, 0.0);
        // The real balance sticks, then jumps 0.3..0.5 g at once (runs 16
        // and 19 journals): it only updates its display once the load has
        // moved by `STICK` from what it shows.
        const STICK: f64 = 0.35;
        let mut shown = w;
        let mut c_late = Vec::new();
        let mut out_per_s = Vec::new();
        for i in 0..2400i64 {
            if (shown - w).abs() >= STICK {
                shown = w;
            }
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[shown])));
            e.scale_tick(at(i));
            let pulse = 1.0 + 0.6 * (i as f64 * 2.399_963).sin(); // golden-angle pseudo-noise
            let out = 0.9 * pulse * (2.0 / 60.0) * e.trim_c();
            w -= out;
            truth += out;
            out_per_s.push(out);
            if i >= 900 {
                c_late.push(e.trim_c());
            }
        }
        let (lo, hi) = c_late.iter().fold((f64::MAX, f64::MIN), |(l, h), &c| (l.min(c), h.max(c)));
        assert!(hi - lo < 0.04, "c still hunting after 15 min: {lo:.3}..{hi:.3}");
        assert!((e.trim_c() - 1.0 / 0.9).abs() < 0.03, "c = {}", e.trim_c());
        // Real flow per 2 min, after the first 15: within ±4 % of 2 ml/min.
        for (n, chunk) in out_per_s[900..].chunks_exact(120).enumerate() {
            let flow = chunk.iter().sum::<f64>() / 2.0;
            assert!((flow / 2.0 - 1.0).abs() < 0.04, "2-min slice {n}: {flow:.3} ml/min");
        }
        let tr = e.status().tracking.unwrap();
        assert!((tr.delivered_ml - truth).abs() < 0.5);
        assert!(tr.deficit_pct.unwrap().abs() < 1.0, "{tr:?}");
    }

    #[test]
    fn the_trim_and_the_journal_keep_going_after_the_curve() {
        // A 20 min flat 10 ml/min curve on a pump delivering 90 %, then 40 min
        // of hold. Before the hold phase, the run closed at 20 min: no more
        // trim, no more journal. Now both carry on at the end value.
        let spec = CurveSpec::linear(10.0, 10.0, Duration::from_secs(1200));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let id = e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        let mut w = 2_000.0;
        let mut truth = 0.0;
        for i in 0..3600 {
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            e.tick(at(i)).unwrap();
            let out = 0.9 * spec.value_at(Duration::from_secs(i as u64)) / 60.0 * e.trim_c();
            w -= out;
            truth += out;
        }
        let st = e.status();
        assert!(st.active.as_ref().unwrap().curve_done);
        let tr = st.tracking.unwrap();
        assert!(st.scale_ok);
        assert!((tr.delivered_ml - truth).abs() < 0.5, "{} vs {truth}", tr.delivered_ml);
        assert!(tr.deficit_pct.unwrap().abs() < 1.0, "{tr:?}");
        assert!((e.trim_c() - 1.0 / 0.9).abs() < 0.03, "c = {}", e.trim_c());
        // Every second of the hold is in the journal, with the weight.
        assert_eq!(e.store().tick_count(id).unwrap(), 3600);
        let last = e.store().last_tick(id).unwrap().unwrap();
        assert!(last.weight_g.is_some() && last.delivered_g.is_some());
        let report = e.tracking_report(id).unwrap().unwrap();
        assert!(report.points.last().unwrap()[0] > 3500.0, "the report covers the hold");
        // One marker each: c first moving, then the ratio measured (>= 2 min).
        let at_of = |kind: &str| {
            let m: Vec<f64> = report.markers.iter().filter(|m| m.kind == kind).map(|m| m.t_s).collect();
            assert_eq!(m.len(), 1, "{kind}: {:?}", report.markers);
            m[0]
        };
        let (start, ratio) = (at_of("trim_start"), at_of("trim_ratio"));
        assert!(start <= ratio && ratio >= 120.0, "start {start} s, ratio {ratio} s");
    }

    #[test]
    fn a_small_bump_on_the_pan_at_low_flow_is_not_chased_by_the_trim() {
        // Run 16 replayed: 2 ml/min, an exact pump, and 20 min in the pan
        // gains 3.4 g for good. It used to count as 3.4 g un-delivered, and
        // the trim pushed c to 1.25 for the rest of the run to "repay" it.
        let spec = CurveSpec::linear(2.0, 2.0, Duration::from_secs(1800));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        let mut truth = simulate(&mut e, &spec, |_| 1.0, 474.2, 0, 1200);
        let w_after = 474.2 - truth + 3.4;
        truth += simulate(&mut e, &spec, |_| 1.0, w_after, 1200, 600);
        let st = e.status();
        let tr = st.tracking.unwrap();
        assert!(st.scale_ok, "a bump is not an alarm");
        assert!((tr.delivered_ml - truth).abs() < 0.5, "{} vs {truth}", tr.delivered_ml);
        assert!(tr.deficit_pct.unwrap().abs() < 2.0, "{tr:?}");
        assert!((e.trim_c() - 1.0).abs() < 0.05, "c chased the bump: {}", e.trim_c());
    }

    #[test]
    fn a_blocked_line_is_called_a_stopped_feed_after_three_minutes() {
        // Nothing leaves the bottle from the start. It used to take the
        // saturation path (5 min of c pinned at its bound); it is now named
        // for what it is, after FEED_STOP_MIN_S, with c back at its start.
        let spec = CurveSpec::linear(60.0, 60.0, Duration::from_secs(36_000));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        simulate(&mut e, &spec, |_| 0.0, 2_000.0, 0, 170);
        assert!(e.status().scale_ok, "not before 3 min");
        simulate(&mut e, &spec, |_| 0.0, 2_000.0, 170, 30);
        let st = e.status();
        assert!(!st.scale_ok, "a line delivering nothing must alarm");
        assert_eq!(st.tracking.unwrap().alarm, Some(tracking::TrimAlarm::FeedStopped));
        assert_eq!(e.trim_c(), 1.0, "back to the starting c");
        simulate(&mut e, &spec, |_| 0.0, 2_000.0, 200, 120);
        assert_eq!(e.trim_c(), 1.0, "c frozen by the alarm");
    }

    #[test]
    fn recover_scale_does_not_treat_a_trim_alarm_as_a_down_link() {
        // A blocked line alarms while every balance read succeeds; recovery
        // must leave that alarm for start_run to clear.
        let spec = CurveSpec::linear(60.0, 60.0, Duration::from_secs(36_000));
        let mut e = engine();
        e.attach_scale(Some(Box::new(ScriptedScale::new(&[]))), scale_cfg("sim-scale"));
        e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        simulate(&mut e, &spec, |_| 0.0, 2_000.0, 0, 600);
        assert!(!e.status().scale_ok, "must have alarmed");
        assert!(!e.scale_link_down());
        assert!(e.recover_scale());
        assert!(!e.status().scale_ok, "recover_scale must not clear an alarm it didn't cause");
        assert_eq!(e.status().scale_connected, Some(true));
    }

    #[test]
    fn a_very_low_flow_never_false_alarms() {
        // 0.05 ml/min on a 0.1 g balance: 3 g/h, the window needs 20 g.
        let spec = CurveSpec::linear(0.05, 0.05, Duration::from_secs(10 * 3600));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        simulate(&mut e, &spec, |_| 0.95, 500.0, 0, 10 * 3600);
        assert!(e.status().scale_ok);
        assert!(e.status().tracking.unwrap().deficit_pct.unwrap().abs() < 3.0);
    }

    #[test]
    fn tracking_uses_the_configured_density() {
        // 2 g/mL: a flat 60 ml/min curve is 2 g/s. A bottle losing exactly
        // that is on track: c stays at 1 and required == delivered in mL.
        let spec = CurveSpec::linear(60.0, 60.0, Duration::from_secs(36_000));
        let mut e = engine();
        e.attach_scale(
            Some(Box::new(ScriptedScale::new(&[]))),
            ScaleConfig { density_g_per_ml: 2.0, ..scale_cfg("sim-scale") },
        );
        e.start_run(RunConfig { curve: spec, ..trim_cfg() }, t0()).unwrap();
        let mut w = 10_000.0;
        for i in 0..900 {
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            w -= 2.0 * e.trim_c();
        }
        let tr = e.status().tracking.unwrap();
        assert!(e.status().scale_ok);
        assert!((e.trim_c() - 1.0).abs() < 0.01, "c = {}", e.trim_c());
        assert!((tr.required_ml - tr.delivered_ml).abs() < 1.0, "{tr:?}");
    }

    /// A balance whose weight *rises* by what the pump delivers (it weighs
    /// the receiving vessel), `secs` one-second reads from `start_g`.
    fn fill_receiver(e: &mut Engine<SimPump>, g_per_s: f64, start_g: f64, from_s: i64, secs: i64) {
        let mut w = start_g;
        for i in from_s..from_s + secs {
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            w += g_per_s * e.trim_c();
        }
    }

    #[test]
    fn a_balance_under_the_receiver_is_tracked_when_configured_so() {
        // Flat 60 ml/min at 1 g/mL, pump exact: the vessel gains 1 g/s.
        let spec = CurveSpec::linear(60.0, 60.0, Duration::from_secs(36_000));
        let mut e = engine();
        e.attach_scale(
            Some(Box::new(ScriptedScale::new(&[]))),
            ScaleConfig { position: crate::config::ScalePosition::Receiver, ..scale_cfg("sim-scale") },
        );
        e.start_run(RunConfig { curve: spec, ..trim_cfg() }, t0()).unwrap();
        fill_receiver(&mut e, 1.0, 300.0, 0, 900);
        let st = e.status();
        let tr = st.tracking.unwrap();
        assert!(st.scale_ok && !tr.wrong_side, "{tr:?}");
        assert!((e.trim_c() - 1.0).abs() < 0.01, "c = {}", e.trim_c());
        assert!((tr.required_ml - tr.delivered_ml).abs() < 1.0, "{tr:?}");
        assert!(tr.deficit_pct.unwrap().abs() < 1.0, "{tr:?}");
        // What is shown is still the real reading, not the flipped one.
        assert!(st.scale_weight_g.unwrap() > 300.0);
    }

    #[test]
    fn a_balance_on_the_other_side_freezes_the_trim_instead_of_chasing_it() {
        // Configured under the feed bottle, but the weight rises: before this
        // guard, c climbed toward TRIM_MAX (+25 %) on a mirrored deficit.
        let spec = CurveSpec::linear(60.0, 60.0, Duration::from_secs(36_000));
        let mut e = engine();
        e.attach_scale(Some(Box::new(ScriptedScale::new(&[]))), scale_cfg("sim-scale"));
        e.start_run(RunConfig { curve: spec, ..trim_cfg() }, t0()).unwrap();
        fill_receiver(&mut e, 1.0, 300.0, 0, 600);
        let st = e.status();
        let tr = st.tracking.unwrap();
        assert!(tr.wrong_side, "{tr:?}");
        assert!(!st.scale_ok, "the trim alarm must be raised");
        assert!((e.trim_c() - 1.0).abs() < 0.03, "c must not run away: {}", e.trim_c());
        let c = e.trim_c();
        fill_receiver(&mut e, 1.0, 900.0, 600, 120);
        assert_eq!(e.trim_c(), c, "c frozen");
    }

    #[test]
    fn an_rpm_calibration_converts_the_curve_for_tracking() {
        // Calibrated at 50 rpm -> 600 g in 5 min = 120 ml/min, 2.4 ml/min
        // per rpm; a flat 50 rpm run delivering exactly that keeps c at 1.
        let mut e = engine();
        let cal = seed_calibration(&e.store, ControlVar::Rpm, 50.0, 600.0);
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let spec = CurveSpec::linear(50.0, 50.0, Duration::from_secs(36_000));
        let cfg = RunConfig {
            control_var: ControlVar::Rpm,
            gravimetric_trim: true,
            tubing_calibration_id: Some(cal),
            curve: spec.clone(),
            ..linear_cfg()
        };
        e.start_run(cfg, t0()).unwrap();
        let mut w = 10_000.0;
        for i in 0..900 {
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            w -= 2.0 * e.trim_c();
        }
        assert!(e.status().scale_ok);
        assert!((e.trim_c() - 1.0).abs() < 0.01, "c = {}", e.trim_c());
    }

    /// A run in ml/min on an rpm calibration (2.4 ml/min per rpm, from
    /// `seed_calibration`'s 600 g in 5 min at 50 rpm).
    fn driven_cfg(cal: i64, spec: CurveSpec) -> RunConfig {
        RunConfig {
            control_var: ControlVar::MlMin,
            tubing_calibration_id: Some(cal),
            curve: spec,
            ..linear_cfg()
        }
    }

    #[test]
    fn an_ml_min_run_on_an_rpm_calibration_is_driven_in_rpm() {
        let mut e = engine();
        let cal = seed_calibration(&e.store, ControlVar::Rpm, 50.0, 600.0);
        let spec = CurveSpec::linear(24.0, 48.0, Duration::from_secs(3600));
        e.start_run(driven_cfg(cal, spec), t0()).unwrap();
        assert!((e.pump.transport().speed_rpm() as f64 - 10.0).abs() < 1e-3, "24 ml/min = 10 rpm");
        for i in 1..=1800 {
            e.apply_setpoint(at(i)).unwrap();
            e.tick(at(i)).unwrap();
        }
        // 36 ml/min half way: 15 rpm, and the pump's own ml/min never used.
        assert!((e.pump.transport().speed_rpm() as f64 - 15.0).abs() < 0.11, "{}", e.pump.transport().speed_rpm());
        assert_eq!(e.pump.transport().flow_ml_min(), 0.0);
        let st = e.status();
        assert!(st.pump_confirmed, "readback compares in ml/min");
        assert!((st.active.unwrap().last_target.unwrap() - 36.0).abs() < 0.25, "journalled in ml/min");
    }

    #[test]
    fn a_trim_past_full_speed_is_not_a_pump_fault() {
        // 820 ml/min fits (840 at 350 rpm); c = 1.06 asks 869. The pump sits
        // at 350 rpm, as written: the readback must not call it a fault.
        let mut e = engine();
        let cal = seed_calibration(&e.store, ControlVar::Rpm, 50.0, 600.0);
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let cfg = RunConfig {
            gravimetric_trim: true,
            ..driven_cfg(cal, CurveSpec::linear(820.0, 820.0, Duration::from_secs(3600)))
        };
        e.start_run(cfg, t0()).unwrap();
        e.set_trim_c_for_test(1.06);
        for i in 1..=40 {
            e.tick(at(i)).unwrap();
        }
        assert!((e.pump.transport().speed_rpm() as f64 - limits::RPM_MAX).abs() < 1e-3);
        assert!(e.status().pump_confirmed);
    }

    #[test]
    fn a_driven_run_beyond_the_tube_at_full_speed_is_refused() {
        let mut e = engine();
        let cal = seed_calibration(&e.store, ControlVar::Rpm, 50.0, 600.0);
        // 350 rpm x 2.4 = 840 ml/min at most.
        let spec = CurveSpec::linear(100.0, 900.0, Duration::from_secs(3600));
        let err = e.start_run(driven_cfg(cal, spec), t0()).unwrap_err();
        assert!(matches!(&err, EngineError::Config(m) if m.contains("840")), "{err:?}");
        assert!(e.status().active.is_none());
    }

    #[test]
    fn a_driven_run_resumes_in_rpm() {
        let db = TempDb::new();
        let spec = CurveSpec::linear(24.0, 48.0, Duration::from_secs(3600));
        {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            let cal = seed_calibration(&a.store, ControlVar::Rpm, 50.0, 600.0);
            a.start_run(driven_cfg(cal, spec), t0()).unwrap();
        }
        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        b.resume(at(1800), grace()).unwrap();
        assert!((b.pump.transport().speed_rpm() as f64 - 15.0).abs() < 0.11, "{}", b.pump.transport().speed_rpm());
        assert_eq!(b.pump.transport().flow_ml_min(), 0.0);
    }

    #[test]
    fn tracking_survives_a_crash_resume() {
        let db = TempDb::new();
        let spec = CurveSpec::linear(60.0, 60.0, Duration::from_secs(36_000));
        let before = {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            a.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
            a.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
            // 601 reads so the last one (t = 600) is also a saved update.
            simulate(&mut a, &spec, |_| 1.0, 5_000.0, 0, 601);
            a.status().tracking.unwrap()
        };
        // 60 s down, the pump kept delivering 1 g/s: the bottle is 60 g lighter.
        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        b.resume(at(660), grace()).unwrap();
        let w = 5_000.0 - before.delivered_ml - 60.0;
        b.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
        b.scale_tick(at(660));
        let after = b.status().tracking.unwrap();
        assert!(
            (after.delivered_ml - (before.delivered_ml + 60.0)).abs() < 0.5,
            "{} vs {}",
            after.delivered_ml,
            before.delivered_ml + 60.0
        );
        assert!((after.required_ml - (before.required_ml + 60.0)).abs() < 0.5);
    }

    #[test]
    fn a_past_run_is_reported_with_its_own_density() {
        // Counted at 2 g/mL; Settings says 1 g/mL by the time it is read.
        let spec = CurveSpec::linear(60.0, 60.0, Duration::from_secs(36_000));
        let mut e = engine();
        e.attach_scale(
            Some(Box::new(ScriptedScale::new(&[]))),
            ScaleConfig { density_g_per_ml: 2.0, ..scale_cfg("sim-scale") },
        );
        let id = e.start_run(RunConfig { curve: spec, ..trim_cfg() }, t0()).unwrap();
        let mut w = 10_000.0;
        for i in 0..300 {
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            e.tick(at(i)).unwrap();
            w -= 2.0 * e.trim_c();
        }
        let report = tracking_report(e.store(), id, 1.0).unwrap().unwrap();
        let last = report.points.last().unwrap();
        // 60 ml/min for ~5 min: ~300 mL, not twice that.
        assert!((last[2] - last[1]).abs() < 0.03 * last[1], "{last:?}");
        assert!((last[1] - 300.0).abs() < 10.0, "{last:?}");
    }

    #[test]
    fn the_journal_records_the_weight_during_a_trimmed_run() {
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[1000.0])));
        let id = e.start_run(flat_60_ml_min_cfg(), t0()).unwrap();
        e.scale_tick(t0());
        e.tick(at(1)).unwrap();
        let t = e.store.last_tick(id).unwrap().unwrap();
        assert_eq!(t.weight_g, Some(1000.0));
        assert!(t.delivered_g.is_some());
    }

    #[test]
    fn the_tracking_report_proves_an_exponential_run() {
        let spec = CurveSpec::exponential_physio(1.0, 0.15, Duration::from_secs(4 * 3600));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let id = e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        let mut w = 5_000.0;
        for i in 0..4 * 3600 {
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            e.tick(at(i)).unwrap();
            w -= 0.9 * spec.value_at(Duration::from_secs(i as u64)) / 60.0 * e.trim_c();
        }
        let r = e.tracking_report(id).unwrap().expect("report");
        assert!(r.r_squared.unwrap() > 0.999, "R² {:?}", r.r_squared);
        assert!((r.mu_requested.unwrap() - 0.15).abs() < 1e-9);
        assert!((r.mu_delivered.unwrap() - 0.15).abs() < 0.01, "µ {:?}", r.mu_delivered);
        assert!(r.points.len() <= 2000);
    }

    #[test]
    fn no_report_for_a_run_without_a_balance() {
        let mut e = engine();
        let id = e.start_run(linear_cfg(), t0()).unwrap();
        e.tick(at(1)).unwrap();
        assert!(e.tracking_report(id).unwrap().is_none());
    }

    #[test]
    fn an_ml_min_calibration_on_an_rpm_run_or_an_unknown_id_is_refused() {
        // The other way round, an rpm calibration on an ml/min run, drives
        // the pump in rpm (`an_ml_min_run_on_an_rpm_calibration_is_driven_in_rpm`).
        let mut e = engine();
        let ml_cal = seed_calibration(&e.store, ControlVar::MlMin, 10.0, 45.0);
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let cfg = RunConfig {
            control_var: ControlVar::Rpm,
            tubing_calibration_id: Some(ml_cal),
            ..trim_cfg()
        };
        assert!(matches!(e.start_run(cfg, t0()), Err(EngineError::Config(_))));
        let cfg = RunConfig { tubing_calibration_id: Some(9999), ..trim_cfg() };
        assert!(matches!(e.start_run(cfg, t0()), Err(EngineError::Config(_))));
        assert!(e.status().active.is_none());
    }

    #[test]
    fn an_archived_calibration_is_refused_until_restored() {
        let mut e = engine();
        let cal = seed_calibration(&e.store, trim_cfg().control_var, 10.0, 50.0);
        e.store.set_calibration_archived(cal, true).unwrap();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let cfg = RunConfig { tubing_calibration_id: Some(cal), ..trim_cfg() };
        assert!(matches!(e.start_run(cfg.clone(), t0()), Err(EngineError::Config(_))));
        assert!(e.status().active.is_none());

        e.store.set_calibration_archived(cal, false).unwrap();
        assert!(e.start_run(cfg, t0()).is_ok());
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
        for i in 10..910 {
            b.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            b.scale_tick(at(i));
            w -= 2.0 * b.trim_c();
        }
        assert!((b.trim_c() - 1.0).abs() < 0.01, "resume lost the rpm-to-volume conversion: c = {}", b.trim_c());
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
    fn a_finished_curve_holds_its_end_value_and_keeps_running_the_run() {
        let mut e = engine();
        let id = e.start_run(linear_cfg(), t0()).unwrap();
        assert!(!e.status().active.unwrap().curve_done);
        let o = e.tick(at(3601)).unwrap(); // past the 3600 s duration
        assert!(matches!(o, TickOutcome::Applied { .. }), "{o:?}");

        // The run stays active, the pump on the curve's end value.
        let a = e.status().active.expect("still active past the curve");
        assert!(a.curve_done);
        assert_eq!(a.name, "r");
        assert!((a.last_target.unwrap() - 100.0).abs() < 1e-6);
        assert!(e.pump.transport().running());
        assert!(e.status().holding.is_none(), "no separate hold any more");
        assert_eq!(e.store().run(id).unwrap().unwrap().status, RunStatus::Running);

        // And it keeps journalling, logging the end of the curve only once.
        e.tick(at(3602)).unwrap();
        e.tick(at(7200)).unwrap();
        assert_eq!(e.store().tick_count(id).unwrap(), 3);
        let done = e.store().events(Some(id), 50).unwrap().into_iter().filter(|x| x.kind == "curve_done").count();
        assert_eq!(done, 1);
    }

    #[test]
    fn stopping_in_the_hold_phase_records_the_run_completed() {
        let mut e = engine();
        let id = e.start_run(linear_cfg(), t0()).unwrap();
        e.tick(at(3601)).unwrap();
        e.stop_run(at(4000)).unwrap();
        assert!(!e.pump.transport().running());
        assert!(e.status().active.is_none());
        assert_eq!(e.store().run(id).unwrap().unwrap().status, RunStatus::Completed);
        // Stopped before its curve ends, a run is still `stopped`.
        let id = e.start_run(linear_cfg(), at(5000)).unwrap();
        e.stop_run(at(5100)).unwrap();
        assert_eq!(e.store().run(id).unwrap().unwrap().status, RunStatus::Stopped);
    }

    #[test]
    fn commanded_volume_keeps_counting_through_the_hold() {
        // Same F(t) = t_min ml/min line as the other commanded-volume tests:
        // 1800 mL over the curve's hour, then 60 ml/min held for 10 min.
        let mut e = engine();
        let cfg = RunConfig {
            control_var: ControlVar::MlMin,
            curve: CurveSpec::linear(0.0, 60.0, Duration::from_secs(3600)),
            ..linear_cfg()
        };
        e.start_run(cfg, t0()).unwrap();
        e.tick(at(1800)).unwrap();
        e.tick(at(3600)).unwrap();
        let v = e.status().active.unwrap().volume_added_ml.unwrap();
        assert!((v - 1800.0).abs() < 1e-6, "got {v}");
        e.tick(at(4200)).unwrap();
        let v = e.status().active.unwrap().volume_added_ml.unwrap();
        assert!((v - 2400.0).abs() < 1e-6, "got {v}");
    }

    #[test]
    fn only_a_calibration_burst_stops_counting_volume_at_its_duration() {
        assert_eq!(volume_horizon_s(RunKind::Calibration, 300), 300.0);
        assert_eq!(volume_horizon_s(RunKind::Dosing, 300), f64::INFINITY);
    }

    /// A hold left behind by a daemon from before the hold phase (the pump
    /// kept its speed across the update): it must still be shown and stoppable.
    fn seed_legacy_hold<T: Transport>(e: &mut Engine<T>, run_id: i64) {
        e.set_holding(Some(HoldingStatus {
            run_id,
            name: "r".into(),
            control_var: ControlVar::Rpm,
            value: 100.0,
            finished_at: at(3600),
            volume_added_ml: None,
        }));
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
        let id = e.start_run(linear_cfg(), t0()).unwrap();
        e.stop_run(at(60)).unwrap();
        e.pump.start().unwrap(); // still spinning after an update, as it was
        seed_legacy_hold(&mut e, id);
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
        seed_legacy_hold(&mut e, 1);
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
    fn tick_at_duration_enters_the_hold_phase() {
        let mut e = engine();
        let id = e.start_run(linear_cfg(), t0()).unwrap();

        let o = e.tick(at(3600)).unwrap();
        assert!(matches!(o, TickOutcome::Applied { .. }));
        assert!(e.pump.transport().running());
        assert!(e.status().active.unwrap().curve_done);
        assert_eq!(e.store().running_run().unwrap().map(|r| r.id), Some(id));
        assert!(matches!(e.tick(at(3610)).unwrap(), TickOutcome::Applied { .. }));
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
                12.0,
                None
            ),
            EngineError::PumpRejectedSetpoint { code: 0x03, .. }
        ));
        // Driven in rpm (2.4 ml/min per rpm), the refused value is the rpm
        // written, reported in rpm: 12 ml/min = 5 rpm.
        assert!(matches!(
            setpoint_error(
                PumpError::Pdu(PduError::Exception(0x03)),
                ControlVar::MlMin,
                12.0,
                Some(2.4)
            ),
            EngineError::PumpRejectedSetpoint { code: 0x03, control_var: ControlVar::Rpm, value }
                if (value - 5.0).abs() < 1e-9
        ));
        // 0x02 (illegal data address) is a register-map fault, not a value the
        // operator can fix, it must stay a plain pump error.
        assert!(matches!(
            setpoint_error(
                PumpError::Pdu(PduError::Exception(0x02)),
                ControlVar::Rpm,
                50.0,
                None
            ),
            EngineError::Pump(_)
        ));
        // Anything else, too.
        assert!(matches!(
            setpoint_error(
                PumpError::Transport(fermentool_modbus::TransportError::Timeout),
                ControlVar::Rpm,
                50.0,
                None
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
    fn a_stop_that_does_not_get_through_stays_pending_until_it_does() {
        let mut e = engine();
        let id = e.start_run(linear_cfg(), t0()).unwrap();
        e.pump.transport_mut().drop_next = 2; // the Stop and its retry are lost
        e.stop_run(at(60)).unwrap();
        // Recorded as asked, but the pump is still running and it is said.
        assert_eq!(e.store().run(id).unwrap().unwrap().status, RunStatus::Stopped);
        assert!(e.pump.transport().running());
        assert!(e.status().stop_pending);
        assert_eq!(event_times(&e, id, "stop_pending").len(), 1);
        // The control loop sends it again: the pump stops, the alarm clears.
        assert!(e.retry_pending_stop(at(61)));
        assert!(!e.pump.transport().running());
        assert!(!e.status().stop_pending);
        assert_eq!(event_times(&e, id, "pump_stopped").len(), 1);
    }

    #[test]
    fn a_stop_over_a_lost_link_is_not_trusted() {
        // The cable is out: whatever stands in for the port proves nothing.
        let mut e = engine();
        e.start_run(linear_cfg(), t0()).unwrap();
        e.serial_lost = true;
        e.stop_run(at(60)).unwrap();
        assert!(e.pump.transport().running(), "nothing could have reached the pump");
        assert!(e.status().stop_pending);
        assert!(!e.retry_pending_stop(at(61)), "not before the link is back");
        e.serial_lost = false;
        assert!(e.retry_pending_stop(at(62)));
        assert!(!e.pump.transport().running());
    }

    #[test]
    fn a_pending_stop_survives_a_restart() {
        let db = TempDb::new();
        {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            a.start_run(linear_cfg(), t0()).unwrap();
            a.pump.transport_mut().drop_next = 2;
            a.stop_run(at(60)).unwrap();
            assert!(a.status().stop_pending);
        }
        let b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        assert!(b.status().stop_pending, "a pump left running is not forgotten");
    }

    #[test]
    fn a_lost_link_is_not_written_to_every_tick() {
        let mut e = engine();
        let id = e.start_run(linear_cfg(), t0()).unwrap();
        e.serial_lost = true;
        let before = e.pump.transport().transactions;
        for i in 1..=20 {
            e.apply_setpoint(at(i)).unwrap();
            assert!(matches!(e.tick(at(i)).unwrap(), TickOutcome::Applied { written_ok: false, .. }));
        }
        assert_eq!(e.pump.transport().transactions, before, "no frame sent over a lost link");
        assert!(event_times(&e, id, "write_fail").is_empty(), "the outage is one event, not one a second");
        assert_eq!(e.store().tick_count(id).unwrap(), 20, "the journal goes on");
    }

    #[test]
    fn a_start_that_cannot_record_its_run_stops_the_pump_again() {
        // A run still marked running (a crash recovery not resolved) makes
        // the insert fail after the pump was started.
        let db = TempDb::new();
        {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            a.start_run(linear_cfg(), t0()).unwrap();
        }
        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        assert!(b.start_run(linear_cfg(), at(100)).is_err());
        assert!(!b.pump.transport().running(), "the pump must not turn with no run");
        assert!(b.status().active.is_none());
    }

    #[test]
    fn a_refused_start_leaves_the_learned_state_alone() {
        let mut e = engine();
        e.set_trim_c_for_test(1.12);
        seed_legacy_hold(&mut e, 1);
        let bad = RunConfig { curve: CurveSpec::linear(1.0, 2.0, Duration::ZERO), ..linear_cfg() };
        assert!(e.start_run(bad, t0()).is_err());
        assert_eq!(e.trim_c(), 1.12);
        assert!(e.status().holding.is_some(), "a refused start must not drop the hold");
    }

    #[test]
    fn a_driven_run_under_the_motor_minimum_holds_the_pump_stopped() {
        // 2.4 ml/min per rpm: 0 ml/min is no speed the pump accepts. The
        // run starts with the pump stopped and starts it once the curve rises.
        let mut e = engine();
        let cal = seed_calibration(&e.store, ControlVar::Rpm, 50.0, 600.0);
        let up = CurveSpec::linear(0.0, 24.0, Duration::from_secs(3600));
        let id = e.start_run(driven_cfg(cal, up), t0()).unwrap();
        assert!(!e.pump.transport().running(), "nothing to deliver yet");
        e.tick(at(1800)).unwrap(); // 12 ml/min = 5 rpm
        assert!(e.pump.transport().running());
        assert!((e.pump.transport().speed_rpm() as f64 - 5.0).abs() < 0.11);
        e.stop_run(at(1801)).unwrap();
        assert!(event_times(&e, id, "write_fail").is_empty());

        // And a curve that steps down to 0 really stops the feed.
        let down = CurveSpec::linear(24.0, 0.0, Duration::from_secs(3600));
        e.start_run(driven_cfg(cal, down), at(4000)).unwrap();
        assert!(e.pump.transport().running());
        e.tick(at(4000 + 3600)).unwrap();
        assert!(!e.pump.transport().running(), "0 ml/min must stop the pump, not keep its last speed");
        assert!(e.status().pump_confirmed);
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
            a.stop_run(at(60)).unwrap();
            seed_legacy_hold(&mut a, id);
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
            let id = a.start_run(linear_cfg(), t0()).unwrap();
            a.stop_run(at(60)).unwrap();
            seed_legacy_hold(&mut a, id);
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
        b.tick(at(3600)).unwrap();
        assert!(b.status().active.unwrap().curve_done);
        b.stop_run(at(3700)).unwrap();
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

        b.tick(at(3600)).unwrap();
        // Curve done: recompute the volume straight from the full journal to
        // check the total matches the curve's exact integral, 60^2 / 2.
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
    fn a_dosing_run_long_past_its_curve_resumes_into_its_hold() {
        let db = TempDb::new();
        let id = {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            let mut cfg = linear_cfg();
            cfg.curve = CurveSpec::linear(1.0, 9.0, Duration::from_secs(100));
            let id = a.start_run(cfg, t0()).unwrap();
            a.tick(at(10)).unwrap();
            id
        };
        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        let g = Duration::from_secs(60);
        let info = b.pending_recovery(at(1000), g).unwrap().unwrap();
        assert!(!info.past_end, "a dosing run can always resume");
        assert!(info.curve_done);
        assert!((info.resume_target - 9.0).abs() < 1e-6);
        assert_eq!(b.resume(at(1000), g).unwrap(), id);
        assert!(b.status().active.unwrap().curve_done);
        assert!((b.pump.transport().speed_rpm() - 9.0).abs() < 0.5);
    }

    #[test]
    fn a_calibration_burst_is_refused_past_the_grace_window() {
        let db = TempDb::new();
        {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            let mut cfg = linear_cfg();
            cfg.curve = CurveSpec::linear(1.0, 9.0, Duration::from_secs(100));
            cfg.kind = RunKind::Calibration;
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
    fn resume_within_grace_applies_the_end_value_and_holds() {
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
        assert!(matches!(b.tick(at(140)).unwrap(), TickOutcome::Applied { .. }));
        assert!(b.status().active.unwrap().curve_done);
    }

    #[test]
    fn an_interrupted_dosing_run_resumes_by_itself_when_asked_to() {
        let db = TempDb::new();
        let id = {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            let id = a.start_run(linear_cfg(), t0()).unwrap();
            a.tick(at(600)).unwrap();
            id
        };
        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        assert_eq!(b.try_auto_resume(at(1800), grace()).unwrap().unwrap(), id);
        assert_eq!(b.status().active.unwrap().run_id, id);
        assert!(b.pump.transport().running());
        assert_eq!(event_times(&b, id, "auto_resume").len(), 1);
        // Nothing more to do once it runs.
        assert!(b.try_auto_resume(at(1810), grace()).is_none());
    }

    #[test]
    fn an_automatic_resume_waits_for_a_pump_that_answers() {
        let db = TempDb::new();
        {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            a.start_run(linear_cfg(), t0()).unwrap();
        }
        // The link is down: nothing is tried.
        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        b.serial_lost = true;
        assert!(b.try_auto_resume(at(1800), grace()).is_none());
        // A calibration burst is never resumed without the operator.
        let db2 = TempDb::new();
        {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db2.store(), "test");
            let cfg = RunConfig { kind: RunKind::Calibration, ..linear_cfg() };
            a.start_run(cfg, t0()).unwrap();
        }
        let mut c = Engine::new(Pump::new(SimPump::new(1), 1), db2.store(), "test");
        assert!(c.try_auto_resume(at(30), grace()).is_none());
    }

    #[test]
    fn resume_is_busy_when_a_run_is_active() {
        let mut e = engine();
        e.start_run(linear_cfg(), t0()).unwrap();
        assert!(matches!(e.resume(t0(), grace()), Err(EngineError::Busy)));
    }
}
