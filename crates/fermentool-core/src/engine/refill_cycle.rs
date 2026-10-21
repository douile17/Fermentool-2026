//! The automatic refill cycle (step 3 of
//! `docs/superpowers/specs/2026-10-02-auto-refill-design.md`): below a low
//! level the refill pump tops the weighed bottle up to the weight it had when
//! the run started, then stops.
//!
//! The regulation rides through it on the balance's own refill handling
//! (`trim::next_state`): while the pump fills, the state machine is held in
//! `RefillPending` as by a "Refill bottle" press, so c is frozen; once the
//! pump is stopped and the level holds still it settles, journals the
//! `refill` event and resumes on the kept ratio history.
//!
//! Evaluated on every balance read of a run that drives the refill pump
//! ([`Engine::scale_tick`]).
//!
//! No flow is assumed: the refill pump is not calibrated, and a new tube or
//! another speed changes what it delivers (run 192: 100 mL/min at 145 rpm
//! against the 285 expected at 350, cut as "no flow" while it filled fine).
//! A fill is judged by the balance alone: the bottle must gain, so the
//! refill pump brings more than the feed pump draws. When it does not, the
//! fill goes on (the line may only be priming) and an alarm is notified;
//! only the high level, and the overfill and Stop guards, stop it.

use std::collections::VecDeque;
use std::time::Instant;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{Engine, EngineError, Result};
use crate::store::{EventLevel, NewEvent};
use crate::trim;
use fermentool_modbus::{PumpTransport, Transport};

/// `app_state` key of the running cycle's [`Plan`], so a resume after a
/// crash keeps the levels the run started with.
const PLAN_KEY: &str = "refill_plan";

/// A run must use at least this much before a refill, or a refill would
/// hardly be worth starting the pump for.
pub const MIN_SPAN_G: f64 = 100.0;
/// The Stop goes out this long of measured rise before the high level: one
/// read and the Stop's own round trip.
const OVERSHOOT_S: f64 = 1.5;
/// Past the high level by this much: the pump did not stop.
pub const OVERFILL_G: f64 = 50.0;
/// The bottle must have risen over this trailing window of a fill (the
/// first one leaves the line time to prime and the balance its ~2 s lag)...
pub const RISE_WINDOW_S: f64 = 30.0;
/// ... by at least this many balance steps, and never less than
/// [`RISE_MIN_G`]: past the balance's noise, and past a feed that the
/// refill only matches.
const RISE_MIN_STEPS: f64 = 10.0;
const RISE_MIN_G: f64 = 2.0;
/// The measured rise is trusted once it spans this long.
const RATE_MIN_SPAN_S: f64 = 10.0;
/// Stops not acknowledged before the refill pump is called out of control.
const STOP_FAILS_ALARM: u32 = 3;
/// After a fill, the level must settle (the balance back to `Normal`) within
/// this long, or the cycle goes on regardless.
const SETTLE_MAX_S: f64 = 300.0;

/// The levels a run refills between, fixed at its start. Persisted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct Plan {
    pub run_id: i64,
    /// A refill starts under this weight: the start weight less what New
    /// run says the bottle gives before a refill ("Refill after using").
    pub low_g: f64,
    /// ... and fills back up to this one (the weight at the run's start).
    pub high_g: f64,
    /// No automatic refill until "Refill now": a guard tripped, or the
    /// operator pressed "Stop refill".
    pub halted: bool,
    /// The overfill alarm is raised (said once, whatever else halted it).
    #[serde(default)]
    pub overfilled: bool,
}

/// How long a fill goes on without a balance reading before it is cut,
/// seconds. At the refill pump's top speed (~4 g/s into a bottle) that is a
/// dozen grams past the level at worst, far from overflowing it.
pub(super) const REFILL_BLIND_S: f64 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) enum Phase {
    #[default]
    Idle,
    /// The refill pump runs since `since_t` (run time, s), from `start_g`.
    Filling { since_t: f64, start_g: f64 },
    /// A Stop was sent and did not get through yet.
    Stopping { since_t: f64, start_g: f64 },
    /// Stopped; waiting for the balance to settle back to `Normal`.
    Settling { since_t: f64 },
}

/// What the balance saw of the fill under way: its trailing
/// [`RISE_WINDOW_S`] of reads, and whether "not rising" was said.
#[derive(Debug, Default)]
pub(super) struct FillTrack {
    window: VecDeque<(f64, f64)>,
    /// The bottle's rise over the window, g/s, once it spans
    /// [`RATE_MIN_SPAN_S`].
    rate_g_s: Option<f64>,
    not_rising: bool,
}

impl FillTrack {
    /// Add the read `weight_g` at `t`: the rise over the trailing window,
    /// `(g, over s)`.
    fn push(&mut self, t: f64, weight_g: f64) -> (f64, f64) {
        self.window.push_back((t, weight_g));
        // Keep the newest read at least a window old as the window's start.
        while self.window.len() > 2 && t - self.window[1].0 >= RISE_WINDOW_S {
            self.window.pop_front();
        }
        let (t0, w0) = self.window[0];
        let span = t - t0;
        self.rate_g_s = (span >= RATE_MIN_SPAN_S).then(|| (weight_g - w0) / span);
        (weight_g - w0, span)
    }
}

/// Why the automatic refill stopped itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefillAlarm {
    Overfill,
    StopFailed,
}

impl RefillAlarm {
    pub fn kind(self) -> &'static str {
        match self {
            RefillAlarm::Overfill => "alarm_refill_overfill",
            RefillAlarm::StopFailed => "alarm_refill_stop_failed",
        }
    }
}

/// What the status frame shows of the cycle.
#[derive(Debug, Clone, Serialize)]
pub struct CycleStatus {
    /// `idle`, `filling`, `stopping` or `settling`.
    pub phase: &'static str,
    pub halted: bool,
    /// A refill starts under this weight, g...
    pub low_g: f64,
    /// ... and fills back up to this one (the weight at the run's start).
    pub high_g: f64,
    /// The balance now, g (`None` before a read).
    pub weight_g: Option<f64>,
    /// During a fill: how fast the bottle rises, g/min, net of the feed
    /// (`None` before 10 s of reads).
    pub rise_g_per_min: Option<f64>,
    /// During a fill: the bottle has not risen over the last 30 s; the
    /// refill pump goes on regardless.
    pub not_rising: bool,
}

/// The levels for a run that refills once the bottle has given `draw_g`
/// and starts with `weight_g` on the balance (the full bottle): `(low,
/// high)` in grams, or why not. It fills back up to where it started; no
/// bottle size or empty weight is needed.
pub fn plan_for(draw_g: f64, weight_g: f64) -> std::result::Result<(f64, f64), String> {
    if !(draw_g > 0.0) {
        return Err("enter how much the bottle gives before a refill".into());
    }
    if draw_g < MIN_SPAN_G {
        return Err(format!("refill after using at least {MIN_SPAN_G:.0} g"));
    }
    if weight_g <= draw_g {
        return Err(format!(
            "the balance reads {:.2} kg, less than the {:.2} kg to use before a refill: fill the bottle, or use less",
            weight_g / 1000.0,
            draw_g / 1000.0
        ));
    }
    Ok((weight_g - draw_g, weight_g))
}

impl<T: Transport> Engine<T> {
    fn save_refill_plan(&self) {
        let json = self.refill.plan.as_ref().and_then(|p| serde_json::to_string(p).ok()).unwrap_or_default();
        let _ = self.store.set_state(PLAN_KEY, &json);
    }

    fn load_refill_plan(&self, run_id: i64) -> Option<Plan> {
        self.store
            .get_state(PLAN_KEY)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<Plan>(&s).ok())
            .filter(|p| p.run_id == run_id)
    }

    /// A run that drives the refill pump is starting with `weight_g` in the
    /// bottle: its levels, or why not.
    pub(super) fn refill_plan_check(&self, low_g: Option<f64>) -> Result<Plan> {
        let weight_g = self
            .live_weight
            .map(|(w, _)| w)
            .ok_or_else(|| EngineError::Config("automatic refill needs a balance reading: check the balance".into()))?;
        let (low_g, high_g) = plan_for(low_g.unwrap_or(0.0), weight_g)
            .map_err(|e| EngineError::Config(format!("automatic refill: {e}")))?;
        Ok(Plan {
            run_id: 0,
            low_g,
            high_g,
            halted: false,
            overfilled: false,
        })
    }

    pub(super) fn refill_plan_start(&mut self, mut plan: Plan, run_id: i64) {
        plan.run_id = run_id;
        self.refill.plan = Some(plan);
        self.refill.phase = Phase::Idle;
        self.refill.resume_fill = false;
        self.save_refill_plan();
    }

    /// A resumed run takes its levels back. Without them (a crash before
    /// they were saved) the automatic refill stays off, said once.
    pub(super) fn refill_plan_resume(&mut self, run_id: i64, now: Timestamp) {
        self.refill.phase = Phase::Idle;
        self.refill.resume_fill = false;
        self.refill.plan = self.load_refill_plan(run_id);
        if self.refill.plan.is_none() {
            let _ = self.store.log_event(&NewEvent {
                run_id: Some(run_id),
                wall_time: now,
                level: EventLevel::Warn,
                kind: "refill_off".into(),
                detail: Some("the refill levels were lost with the crash: automatic refill off for this run".into()),
            });
        }
    }

    pub(super) fn refill_plan_end(&mut self) {
        self.refill.plan = None;
        self.refill.phase = Phase::Idle;
        self.refill.resume_fill = false;
        self.save_refill_plan();
    }

    /// The balance regulation must treat the bottle as being refilled.
    pub(super) fn refill_filling(&self) -> bool {
        matches!(self.refill.phase, Phase::Filling { .. } | Phase::Stopping { .. })
    }

    pub fn refill_cycle_status(&self) -> Option<CycleStatus> {
        let p = self.refill.plan.as_ref()?;
        Some(CycleStatus {
            phase: match self.refill.phase {
                Phase::Idle => "idle",
                Phase::Filling { .. } => "filling",
                Phase::Stopping { .. } => "stopping",
                Phase::Settling { .. } => "settling",
            },
            halted: p.halted,
            low_g: p.low_g,
            high_g: p.high_g,
            weight_g: self.live_weight.map(|(w, _)| w),
            rise_g_per_min: if self.refill_filling() { self.refill.fill.rate_g_s.map(|r| r * 60.0) } else { None },
            not_rising: self.refill_filling() && self.refill.fill.not_rising,
        })
    }

    fn refill_event(&self, now: Timestamp, level: EventLevel, kind: &str, detail: String) {
        let _ = self.store.log_event(&NewEvent {
            run_id: self.active.as_ref().map(|a| a.id),
            wall_time: now,
            level,
            kind: kind.into(),
            detail: Some(detail),
        });
    }

    /// Start the refill pump: speed, direction, Start.
    fn refill_start_pump(&mut self, t: f64, weight_g: f64, now: Timestamp, why: &str) -> bool {
        let cfg = self.refill.cfg.clone();
        let res = self.pump.with_address(cfg.pump_address, |p| {
            p.set_speed_rpm(cfg.speed_rpm as f32)?;
            p.set_direction(cfg.direction.clockwise())?;
            p.start()
        });
        if let Err(e) = res {
            // A Start whose answer was lost may have reached it.
            self.refill.stop_owed = true;
            self.refill.next_stop = Some(Instant::now());
            self.refill_event(now, EventLevel::Warn, "refill_start_failed", format!("the refill pump did not start ({e}); tried again later"));
            return false;
        }
        self.refill.phase = Phase::Filling { since_t: t, start_g: weight_g };
        self.refill.fill = FillTrack::default();
        self.refill.fill.push(t, weight_g);
        let high_g = self.refill.plan.as_ref().expect("a cycle has a plan").high_g;
        self.refill_event(
            now,
            EventLevel::Info,
            "refill_start",
            format!("{why}: {weight_g:.0} g on the balance, filling to {high_g:.0} g at {} rpm", cfg.speed_rpm),
        );
        true
    }

    /// A new speed from Settings while the refill pump fills: written to it
    /// at once (it is otherwise written at each fill's start). Never starts
    /// it; a direction change waits for the next fill (reversed mid-fill, it
    /// would pump the bottle back into the reservoir).
    pub(super) fn refill_speed_now(&mut self, rpm: f64) {
        if !matches!(self.refill.phase, Phase::Filling { .. }) {
            return;
        }
        let res = self.pump.with_address(self.refill.cfg.pump_address, |p| p.set_speed_rpm(rpm as f32));
        let (level, detail) = match res {
            Ok(()) => (EventLevel::Info, format!("refill pump set to {rpm} rpm during the fill")),
            Err(e) => (EventLevel::Warn, format!("the refill pump did not take {rpm} rpm ({e}); the next fill starts at it")),
        };
        self.refill_event(Timestamp::now(), level, "refill_speed", detail);
    }

    /// Stop the refill pump at the end of a fill (or on a guard).
    fn refill_stop_fill(&mut self, t: f64, start_g: f64) {
        if self.refill_stop(Instant::now()) {
            self.refill.phase = Phase::Settling { since_t: t };
        } else {
            // Sent again by the control loop every second, and here on
            // every read until it gets through.
            self.refill.stop_owed = true;
            self.refill.next_stop = Some(Instant::now());
            self.refill.phase = Phase::Stopping { since_t: t, start_g };
        }
    }

    fn refill_alarm(&mut self, alarm: RefillAlarm, now: Timestamp, detail: String) {
        tracing::warn!("{}: {detail}", alarm.kind());
        if let Some(p) = self.refill.plan.as_mut() {
            p.halted = true;
            p.overfilled |= alarm == RefillAlarm::Overfill;
        }
        self.save_refill_plan();
        self.refill_event(now, EventLevel::Error, alarm.kind(), detail);
    }

    /// A balance read failed during a fill: no filling blind for long. A
    /// read or two missed (a PC busy for a moment, the balance's USB link
    /// slow to take a request) is ridden through; the fill is cut once no
    /// reading has come for `REFILL_BLIND_S`. Run 210 lost one read to a PC
    /// compiling, and its fill stopped 42 g short.
    pub(super) fn refill_cycle_read_failed(&mut self, now: Timestamp) {
        if let Phase::Filling { since_t, start_g } = self.refill.phase {
            let since = *self.refill.blind_since.get_or_insert(now);
            if now.duration_since(since).as_secs_f64() < REFILL_BLIND_S {
                return;
            }
            self.refill.blind_since = None;
            self.refill_stop_fill(since_t, start_g);
            if matches!(self.refill.phase, Phase::Settling { .. }) {
                self.refill.phase = Phase::Idle;
            }
            self.refill.resume_fill = true;
            self.refill_event(now, EventLevel::Warn, "refill_interrupted", "no balance reading: refill pump stopped, the refill starts again once the balance answers".into());
        }
    }

    /// One balance read of a run that drives the refill pump: `weight_g` in
    /// the bottle at run time `t`.
    pub(super) fn refill_cycle_step(&mut self, weight_g: f64, t: f64, now: Timestamp) {
        // A reading: the balance answers, a gap in a fill is over.
        self.refill.blind_since = None;
        let Some(plan) = self.refill.plan.clone() else {
            return;
        };
        match self.refill.phase {
            Phase::Idle => {
                if plan.halted {
                    self.refill.resume_fill = false;
                    return;
                }
                if self.scale_state != trim::ScaleState::Normal || self.refill.stop_owed {
                    return;
                }
                if self.refill.resume_fill {
                    if weight_g >= plan.high_g {
                        self.refill.resume_fill = false;
                    } else if self.refill_line_up()
                        && self.refill_start_pump(t, weight_g, now, "resuming the interrupted refill")
                    {
                        self.refill.resume_fill = false;
                    }
                    return;
                }
                if weight_g < plan.low_g && self.refill_line_up() {
                    self.refill_start_pump(t, weight_g, now, "under the refill level");
                }
            }
            Phase::Filling { since_t, start_g } => {
                let dur = t - since_t;
                let gained = weight_g - start_g;
                let (rise, span) = self.refill.fill.push(t, weight_g);
                let lead = OVERSHOOT_S * self.refill.fill.rate_g_s.unwrap_or(0.0).max(0.0);
                if weight_g >= plan.high_g - lead {
                    self.refill_stop_fill(t, start_g);
                    self.refill_event(
                        now,
                        EventLevel::Info,
                        "refill_stop",
                        format!(
                            "filled {gained:.0} g in {dur:.0} s at {} rpm ({:+.0} g/min net of the feed)",
                            self.refill.cfg.speed_rpm,
                            gained / dur.max(1.0) * 60.0
                        ),
                    );
                    return;
                }
                if span < RISE_WINDOW_S {
                    return;
                }
                let rise_min = (RISE_MIN_STEPS * self.scale_resolution_g).max(RISE_MIN_G);
                if rise < rise_min && !self.refill.fill.not_rising {
                    // Said, not acted on: the line may only be priming, and a
                    // refill is wanted even a poor one.
                    self.refill.fill.not_rising = true;
                    tracing::warn!("alarm_refill_no_rise: {rise:+.1} g over {span:.0} s");
                    self.refill_event(
                        now,
                        EventLevel::Error,
                        "alarm_refill_no_rise",
                        format!(
                            "refill pump on for {dur:.0} s at {} rpm, the bottle moved {rise:+.1} g over the last {span:.0} s: it keeps pumping in case the line is priming. Check the reservoir, the line and the refill pump",
                            self.refill.cfg.speed_rpm
                        ),
                    );
                } else if rise >= rise_min && self.refill.fill.not_rising {
                    self.refill.fill.not_rising = false;
                    self.refill_event(
                        now,
                        EventLevel::Info,
                        "refill_rising",
                        format!("the bottle fills again: {:+.0} g/min net of the feed", rise / span * 60.0),
                    );
                }
            }
            Phase::Stopping { since_t, start_g } => {
                if !self.refill.stop_owed {
                    self.refill.phase = Phase::Settling { since_t: t };
                    return;
                }
                // The control loop resends it every second; here as well.
                if self.refill_stop(Instant::now()) {
                    self.refill.phase = Phase::Settling { since_t: t };
                    return;
                }
                self.refill.stop_owed = true;
                if self.refill.stop_fails == STOP_FAILS_ALARM {
                    self.refill_alarm(
                        RefillAlarm::StopFailed,
                        now,
                        "the refill pump does not acknowledge Stop: it may still be filling. Stop it by hand; the Stop is sent again every second".into(),
                    );
                }
                if weight_g > plan.high_g + OVERFILL_G && !plan.overfilled {
                    self.refill_alarm(
                        RefillAlarm::Overfill,
                        now,
                        format!("the bottle is {:.0} g over its high level: the refill pump did not stop", weight_g - plan.high_g),
                    );
                }
                let _ = (since_t, start_g);
            }
            Phase::Settling { since_t } => {
                if weight_g > plan.high_g + OVERFILL_G && !plan.overfilled {
                    self.refill_alarm(
                        RefillAlarm::Overfill,
                        now,
                        format!("the bottle is {:.0} g over its high level after the refill", weight_g - plan.high_g),
                    );
                }
                if self.scale_state == trim::ScaleState::Normal || t - since_t > SETTLE_MAX_S {
                    self.refill.phase = Phase::Idle;
                }
            }
        }
    }

    /// "Refill now": start a fill whatever the level, and lift a halt.
    pub fn refill_now(&mut self, now: Timestamp) -> std::result::Result<(), String> {
        let Some(plan) = self.refill.plan.clone() else {
            return Err("No run with automatic refill.".into());
        };
        if self.refill.phase != Phase::Idle {
            return Err("A refill is already going.".into());
        }
        if !self.refill_line_up() {
            return Err("The pump line is not connected.".into());
        }
        let Some((weight_g, _)) = self.live_weight.filter(|_| self.scale_link_up()) else {
            return Err("No balance reading: a refill is not run blind.".into());
        };
        if weight_g >= plan.high_g {
            if plan.halted {
                self.refill_resume_auto(now);
                return Ok(());
            }
            return Err("The bottle is already at its high level.".into());
        }
        let t = self.run_t(now);
        if plan.halted {
            self.refill_resume_auto(now);
        }
        if self.refill_start_pump(t, weight_g, now, "Refill now") {
            Ok(())
        } else {
            Err("The refill pump did not start: check it (Settings, Refill pump, Test).".into())
        }
    }

    fn refill_resume_auto(&mut self, now: Timestamp) {
        if let Some(p) = self.refill.plan.as_mut() {
            p.halted = false;
            p.overfilled = false;
        }
        self.save_refill_plan();
        self.refill_event(now, EventLevel::Info, "refill_restarted", "automatic refill on again".into());
    }

    /// "Stop refill": stop a fill now and hold the automatic refill until
    /// "Refill now".
    pub fn refill_pause(&mut self, now: Timestamp) -> std::result::Result<(), String> {
        let Some(plan) = self.refill.plan.as_ref() else {
            return Err("No run with automatic refill.".into());
        };
        if plan.halted && !self.refill_filling() {
            return Err("Automatic refill is already stopped.".into());
        }
        let t = self.run_t(now);
        if let Phase::Filling { start_g, .. } = self.refill.phase {
            self.refill_stop_fill(t, start_g);
        }
        if let Some(p) = self.refill.plan.as_mut() {
            p.halted = true;
        }
        self.save_refill_plan();
        self.refill_event(now, EventLevel::Info, "refill_paused", "automatic refill stopped by hand, until Refill now".into());
        Ok(())
    }

    fn run_t(&self, now: Timestamp) -> f64 {
        self.active
            .as_ref()
            .map_or(0.0, |a| now.duration_since(a.started_at).as_secs_f64().max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_refills_once_the_bottle_gave_its_draw_back_to_where_it_started() {
        // A full 4.98 kg bottle, refilled every 300 g used.
        assert_eq!(plan_for(300.0, 4980.0).unwrap(), (4680.0, 4980.0));
        assert_eq!(plan_for(1600.0, 9600.0).unwrap(), (8000.0, 9600.0));
    }

    #[test]
    fn the_plan_needs_a_draw_the_bottle_can_give() {
        assert!(plan_for(0.0, 9600.0).unwrap_err().contains("before a refill"));
        assert!(plan_for(50.0, 9600.0).unwrap_err().contains("at least 100 g"));
        assert!(plan_for(5000.0, 4980.0).unwrap_err().contains("fill the bottle"));
    }

    // --- the cycle on a simulated bench -------------------------------------

    use super::super::tests::TempDb;
    use crate::config::RefillConfig;
    use crate::engine::RunConfig;
    use crate::store::{ControlVar, RunKind, Store};
    use fermentool_curves::CurveSpec;
    use fermentool_modbus::{Pump, SimBus};
    use std::time::Duration;

    /// One balance reply.
    struct Reading(f64);
    impl Transport for Reading {
        fn transaction(&mut self, _: &[u8]) -> std::result::Result<Vec<u8>, fermentool_modbus::TransportError> {
            Ok(format!("S S {:>10.1} g\r\n", self.0).into_bytes())
        }
    }

    fn t0() -> Timestamp {
        "2026-10-14T09:00:00Z".parse().unwrap()
    }

    /// A 1 L bottle (300 g empty, overflow line fitted) starting with 800 mL,
    /// fed at 60 mL/min, refilled at 285 mL/min from address 2 unless a test
    /// says otherwise.
    struct Bench {
        e: Engine<SimBus>,
        w: f64,
        /// The reservoir has run dry: the refill pump turns, nothing comes.
        dry: bool,
        /// What the refill pump really delivers, mL/min.
        refill_ml_min: f64,
        run_id: i64,
    }

    impl Bench {
        fn new(store: Store) -> Self {
            let mut e = Engine::new(Pump::new(SimBus::new(), 1), store, "test");
            e.pump_addr = 1;
            e.set_refill(
                RefillConfig { pump_address: 2, ..RefillConfig::default() },
                Instant::now(),
            );
            Bench { e, w: 1100.0, dry: false, refill_ml_min: 285.0, run_id: 0 }
        }

        fn start(&mut self) {
            self.e.attach_scale_for_test(Box::new(Reading(self.w)));
            self.e.live_weight = Some((self.w, true));
            let cfg = RunConfig {
                name: "refill".into(),
                control_var: ControlVar::MlMin,
                direction: crate::store::Direction::Cw,
                pump_addr: 1,
                curve: CurveSpec::linear(60.0, 60.0, Duration::from_secs(36_000)),
                gravimetric_trim: true,
                kind: RunKind::Dosing,
                tubing_calibration_id: None,
                responsible: None,
                auto_refill: true,
                refill_draw_g: Some(550.0), // 1100 g at the start: refills under 550 g
            };
            self.run_id = self.e.start_run(cfg, t0()).unwrap();
        }

        fn refill_running(&self) -> bool {
            self.e.pump.transport().get(2).is_some_and(|p| p.running())
        }

        /// One second: a balance read, then what both pumps moved.
        fn second(&mut self, i: i64) {
            let now = t0() + jiff::SignedDuration::from_secs(i);
            self.e.attach_scale_for_test(Box::new(Reading(self.w)));
            self.e.scale_tick(now);
            self.e.tick(now).unwrap();
            self.w -= 60.0 / 60.0 * self.e.trim_c();
            if self.refill_running() && !self.dry {
                self.w += self.refill_ml_min / 60.0;
            }
        }

        fn kinds(&self) -> Vec<String> {
            let mut v: Vec<_> = self.e.store.events(Some(self.run_id), 10_000).unwrap();
            v.sort_by_key(|e| e.id);
            v.into_iter().map(|e| e.kind).collect()
        }
    }

    #[test]
    fn a_speed_changed_during_a_fill_reaches_the_refill_pump_at_once() {
        // Run 210: the speed went from 55 to 350 rpm in Settings mid-fill;
        // the refill pump went on at 55 until the next fill.
        let mut b = Bench::new(Store::open_in_memory().unwrap());
        b.start();
        let mut i = 1;
        while !b.refill_running() {
            b.second(i);
            i += 1;
            assert!(i < 3000, "no fill started");
        }
        assert!((b.e.pump.transport().get(2).unwrap().speed_rpm() - 350.0).abs() < 1e-3);
        b.e.set_refill(RefillConfig { pump_address: 2, speed_rpm: 120.0, ..RefillConfig::default() }, Instant::now());
        let p = b.e.pump.transport().get(2).unwrap();
        assert!(p.running(), "the fill goes on");
        assert!((p.speed_rpm() - 120.0).abs() < 1e-3, "still at {} rpm", p.speed_rpm());
        assert!(b.kinds().iter().any(|k| k == "refill_speed"), "{:?}", b.kinds());
    }

    /// Fill until the refill pump runs, then `blind` seconds without a
    /// balance reading (the reads fail), then readings again.
    fn fill_with_a_gap(blind: i64) -> Bench {
        let mut b = Bench::new(Store::open_in_memory().unwrap());
        b.start();
        let mut i = 1;
        while !b.refill_running() {
            b.second(i);
            i += 1;
            assert!(i < 3000, "no fill started");
        }
        for k in 0..blind {
            b.e.refill_cycle_read_failed(t0() + jiff::SignedDuration::from_secs(i + k));
        }
        b
    }

    #[test]
    fn a_read_or_two_lost_mid_fill_does_not_stop_it() {
        // Run 210: one failed write to the balance (the PC busy compiling)
        // stopped the fill 42 g short, and it was finished as a second refill.
        let b = fill_with_a_gap(2);
        assert!(b.refill_running(), "the fill goes on through a 2 s gap");
        assert!(!b.kinds().iter().any(|k| k == "refill_interrupted"), "{:?}", b.kinds());
    }

    #[test]
    fn a_balance_silent_past_the_grace_still_stops_the_fill() {
        let b = fill_with_a_gap(REFILL_BLIND_S as i64 + 2);
        assert!(!b.refill_running(), "no filling blind for long");
        assert!(b.kinds().iter().any(|k| k == "refill_interrupted"), "{:?}", b.kinds());
    }

    #[test]
    fn a_reading_back_in_time_restarts_the_grace() {
        // Two short gaps, a good read between them: neither stops the fill.
        let mut b = fill_with_a_gap(2);
        let now = t0() + jiff::SignedDuration::from_secs(5000);
        b.e.attach_scale_for_test(Box::new(Reading(b.w)));
        b.e.scale_tick(now);
        for k in 1..3 {
            b.e.refill_cycle_read_failed(now + jiff::SignedDuration::from_secs(k));
        }
        assert!(b.refill_running());
    }

    #[test]
    fn a_speed_changed_between_fills_waits_for_the_next_one() {
        let mut b = Bench::new(Store::open_in_memory().unwrap());
        b.start();
        b.second(1);
        assert!(!b.refill_running());
        b.e.set_refill(RefillConfig { pump_address: 2, speed_rpm: 120.0, ..RefillConfig::default() }, Instant::now());
        assert!(!b.e.pump.transport().get(2).is_some_and(|p| p.running()), "a speed change never starts the pump");
        assert!(!b.kinds().iter().any(|k| k == "refill_speed"));
    }

    #[test]
    fn the_bottle_is_refilled_twice_and_the_regulation_rides_through() {
        let mut b = Bench::new(Store::open_in_memory().unwrap());
        b.start();
        let mut high_seen: f64 = 0.0;
        let mut lowest: f64 = f64::MAX;
        let mut cs = Vec::new();
        for i in 1..2400 {
            b.second(i);
            high_seen = high_seen.max(b.w);
            lowest = lowest.min(b.w);
            cs.push(b.e.trim_c());
        }
        let kinds = b.kinds();
        let starts = kinds.iter().filter(|k| *k == "refill_start").count();
        assert!(starts >= 2, "{kinds:?}");
        assert_eq!(kinds.iter().filter(|k| *k == "refill_stop").count(), starts);
        assert!(kinds.iter().filter(|k| *k == "refill").count() >= 2, "the balance journals each refill: {kinds:?}");
        assert!(!kinds.iter().any(|k| k.starts_with("alarm_")), "{kinds:?}");
        // Never past the high level (1100 g) by more than a read or two of flow.
        assert!(high_seen < 1100.0 + 15.0, "{high_seen}");
        // Never under the low level (550 g) by more than a read.
        assert!(lowest > 550.0 - 5.0, "{lowest}");
        // c holds through the fills: the pump delivers what it is told.
        let c_last = *cs.last().unwrap();
        assert!((c_last - 1.0).abs() < 0.02, "c = {c_last}");
        assert!(b.e.status().scale_ok);

        // The journal's delivered count neither dives nor jumps across the
        // fills: estimated meanwhile, and said so.
        let d = b.e.store.delivery_samples(b.run_id, 10_000).unwrap();
        for w in d.windows(2) {
            let step = w[1].1 - w[0].1;
            assert!((-1.0..5.0).contains(&step), "delivered moved {step:.1} g at {:.0} s", w[1].0);
        }
        let spans = b.e.store.estimated_spans(b.run_id).unwrap();
        assert!(spans.len() >= starts, "{spans:?}");
        assert!(spans.iter().all(|s| s[1] - s[0] >= 30.0), "{spans:?}");
    }

    /// Runs the bench until the refill pump starts; returns the second.
    fn until_refill(b: &mut Bench) -> i64 {
        let mut i = 1;
        while !b.refill_running() {
            b.second(i);
            i += 1;
            assert!(i < 1200, "no refill started");
        }
        i
    }

    #[test]
    fn a_slow_refill_pump_fills_to_the_top_without_an_alarm() {
        // Run 192: 100 mL/min at 145 rpm, cut as "no flow" against the 285
        // expected at 350. Nothing is expected now: it rises, it fills.
        let mut b = Bench::new(Store::open_in_memory().unwrap());
        b.refill_ml_min = 100.0;
        b.start();
        let mut high_seen: f64 = 0.0;
        for i in 1..2400 {
            b.second(i);
            high_seen = high_seen.max(b.w);
        }
        let kinds = b.kinds();
        assert!(kinds.iter().filter(|k| *k == "refill_stop").count() >= 1, "{kinds:?}");
        assert!(!kinds.iter().any(|k| k.starts_with("alarm_")), "{kinds:?}");
        assert!(high_seen > 1100.0 - 15.0 && high_seen < 1100.0 + 15.0, "{high_seen}");
        let stop = b.e.store.events(Some(b.run_id), 10_000).unwrap().into_iter().find(|e| e.kind == "refill_stop").unwrap();
        let detail = stop.detail.unwrap();
        assert!(detail.contains("rpm") && detail.contains("g/min"), "the journal says how fast: {detail}");
    }

    #[test]
    fn an_empty_reservoir_is_notified_and_the_refill_pump_keeps_going() {
        let mut b = Bench::new(Store::open_in_memory().unwrap());
        b.start();
        let start = until_refill(&mut b);
        b.dry = true;
        for i in start..start + 600 {
            b.second(i);
        }
        let kinds = b.kinds();
        assert_eq!(kinds.iter().filter(|k| *k == "alarm_refill_no_rise").count(), 1, "said once: {kinds:?}");
        assert!(b.refill_running(), "never cut: the line may only be priming");
        let st = b.e.refill_cycle_status().unwrap();
        assert!(!st.halted);
        assert!(st.not_rising);
        assert_eq!(st.phase, "filling");
    }

    #[test]
    fn a_line_that_primes_late_is_said_then_cleared_and_fills_on() {
        let mut b = Bench::new(Store::open_in_memory().unwrap());
        b.start();
        let start = until_refill(&mut b);
        b.dry = true;
        let mut i = start;
        while i < start + 45 {
            b.second(i);
            i += 1;
        }
        b.dry = false;
        let mut high_seen: f64 = 0.0;
        for _ in 0..400 {
            b.second(i);
            i += 1;
            high_seen = high_seen.max(b.w);
        }
        let kinds = b.kinds();
        let alarm = kinds.iter().position(|k| k == "alarm_refill_no_rise").expect("said");
        let rising = kinds.iter().position(|k| k == "refill_rising").expect("cleared");
        let stop = kinds.iter().position(|k| k == "refill_stop").expect("filled to the top");
        assert!(alarm < rising && rising < stop, "{kinds:?}");
        assert!(high_seen > 1100.0 - 15.0, "{high_seen}");
        assert!(!b.e.refill_cycle_status().unwrap().not_rising);
    }

    #[test]
    fn a_refill_pump_slower_than_the_feed_is_said_the_bottle_does_not_rise() {
        // 40 mL/min in, 60 mL/min out: the bottle falls while it "refills".
        let mut b = Bench::new(Store::open_in_memory().unwrap());
        b.refill_ml_min = 40.0;
        b.start();
        let start = until_refill(&mut b);
        for i in start..start + 120 {
            b.second(i);
        }
        assert!(b.kinds().contains(&"alarm_refill_no_rise".to_string()));
        assert!(b.refill_running());
    }

    #[test]
    fn the_rise_is_shown_while_filling() {
        let mut b = Bench::new(Store::open_in_memory().unwrap());
        b.refill_ml_min = 100.0;
        b.start();
        let start = until_refill(&mut b);
        for i in start..start + 20 {
            b.second(i);
        }
        let r = b.e.refill_cycle_status().unwrap().rise_g_per_min.expect("measured after 10 s");
        // 100 in, 60 out: about +40 g/min net.
        assert!((r - 40.0).abs() < 5.0, "{r}");
    }

    #[test]
    fn a_refill_pump_that_ignores_stop_is_called_out() {
        let mut b = Bench::new(Store::open_in_memory().unwrap());
        b.start();
        for i in 1..1200 {
            if b.refill_running() && b.w > 1050.0 {
                // From here its Stops go unanswered: it fills on (the
                // simulated pump keeps its running state).
                b.e.pump.transport_mut().set_plugged(2, false);
            }
            b.second(i);
            if b.w > 1300.0 {
                break;
            }
        }
        let kinds = b.kinds();
        assert!(kinds.contains(&"alarm_refill_stop_failed".to_string()), "{kinds:?}");
        assert!(kinds.contains(&"alarm_refill_overfill".to_string()), "{kinds:?}");
    }

    /// A balance read lost on the wire (run 183: "failed to write whole
    /// buffer" on its port).
    struct LostRead;
    impl Transport for LostRead {
        fn transaction(&mut self, _: &[u8]) -> std::result::Result<Vec<u8>, fermentool_modbus::TransportError> {
            Err(fermentool_modbus::TransportError::Io("serial I/O error: failed to write whole buffer".into()))
        }
    }

    #[test]
    fn a_fill_cut_by_a_lost_balance_read_goes_on_to_the_high_level() {
        // Run 183: cut at 4114 g of the 4155 g, then left there.
        let mut b = Bench::new(Store::open_in_memory().unwrap());
        b.start();
        let mut i = 1;
        while !b.refill_running() {
            b.second(i);
            i += 1;
            assert!(i < 1200, "no refill started");
        }
        // Cut near the top, as in run 183: far over the low level, so only a
        // resumed fill brings it up, not the next one.
        while b.w < 1000.0 {
            b.second(i);
            i += 1;
        }
        // The balance silent past the grace (REFILL_BLIND_S): the pump fills
        // on blind meanwhile, then stops.
        for _ in 0..=(REFILL_BLIND_S as i64) {
            let now = t0() + jiff::SignedDuration::from_secs(i);
            b.e.attach_scale_for_test(Box::new(LostRead));
            b.e.scale_tick(now);
            if b.refill_running() {
                b.w += b.refill_ml_min / 60.0;
            }
            i += 1;
        }
        let cut_at = b.w;
        assert!(!b.refill_running(), "no filling blind for long");
        let mut high_seen: f64 = 0.0;
        for _ in 0..120 {
            b.second(i);
            i += 1;
            high_seen = high_seen.max(b.w);
        }
        assert!(cut_at < 1050.0, "cut under the high level: {cut_at}");
        assert!(high_seen > 1100.0 - 15.0, "filled on to the high level: {high_seen}");
        assert!(high_seen < 1100.0 + 15.0, "{high_seen}");
        let kinds = b.kinds();
        assert!(kinds.contains(&"refill_interrupted".to_string()), "{kinds:?}");
        assert!(!kinds.iter().any(|k| k.starts_with("alarm_")), "{kinds:?}");
    }

    #[test]
    fn refill_now_and_stop_refill() {
        let mut b = Bench::new(Store::open_in_memory().unwrap());
        b.start();
        for i in 1..60 {
            b.second(i);
        }
        assert!(!b.refill_running(), "above the low level");
        b.e.refill_now(t0() + jiff::SignedDuration::from_secs(60)).unwrap();
        assert!(b.refill_running());
        b.e.refill_pause(t0() + jiff::SignedDuration::from_secs(61)).unwrap();
        assert!(!b.refill_running());
        for i in 61..900 {
            b.second(i);
        }
        assert!(!b.refill_running(), "stopped by hand: no automatic refill");
        assert!(b.w < 550.0);
        b.e.refill_now(t0() + jiff::SignedDuration::from_secs(900)).unwrap();
        assert!(b.refill_running());
        assert!(b.kinds().contains(&"refill_restarted".to_string()));
    }

    #[test]
    fn a_run_resumed_after_a_crash_keeps_its_levels() {
        let db = TempDb::new();
        {
            let mut b = Bench::new(db.store());
            b.start();
            for i in 1..30 {
                b.second(i);
            }
        }
        let mut e = Engine::new(Pump::new(SimBus::new(), 1), db.store(), "test");
        e.set_refill(RefillConfig { pump_address: 2, ..RefillConfig::default() }, Instant::now());
        e.resume(t0() + jiff::SignedDuration::from_secs(60), Duration::from_secs(300)).unwrap();
        let st = e.refill_cycle_status().unwrap();
        assert_eq!(st.high_g, 1100.0);
        assert_eq!(st.low_g, 550.0);
    }

}
