//! The refill pump: a second LabQ on the feed pump's RS-485 line, at its own
//! MODBUS address (`[refill]`). A run chooses in New run whether it drives
//! it (`RunConfig::auto_refill`). This part only links it: a Stop before the
//! run starts (a run is refused if it does not answer), then one every
//! [`REASSERT`], so a LabQ that restarted by itself after a power cut is
//! stopped within seconds; a last one when the run ends; and a 3 s Test from
//! Settings to check the address and the direction. Outside such a run
//! nothing is sent to it. The automatic cycle comes later
//! (`docs/superpowers/specs/2026-10-02-auto-refill-design.md`, step 3).
//!
//! Everything goes through the feed pump's `Pump` and its watchdog transport
//! ([`Pump::with_address`]), on the control thread: no second handle on the
//! port, the inter-frame gap is the transport's.

use std::time::{Duration, Instant};

use fermentool_modbus::{PumpTransport, Transport};
use jiff::Timestamp;
use serde::Serialize;

use super::{Engine, EngineError, Result};
use crate::config::RefillConfig;
use crate::store::{EventLevel, NewEvent};

/// The Stop is sent again this often while the refill pump answers.
pub const REASSERT: Duration = Duration::from_secs(10);

/// ... and this often while it does not: each unanswered Stop holds the line
/// (and the feed pump's setpoint) for its timeout, twice with the retry.
pub const REASSERT_SILENT: Duration = Duration::from_secs(60);

/// A Stop owed after a Test (the pump may be running) is retried this soon.
const RETRY_OWED: Duration = Duration::from_secs(1);

/// How long a Test runs the refill pump.
pub const TEST_RUN: Duration = Duration::from_secs(3);

#[derive(Debug, Default)]
pub(super) struct RefillLink {
    pub(super) cfg: RefillConfig,
    /// The active run drives the refill pump.
    run: bool,
    /// A run that drove it ended, or is waiting to be resumed after a crash,
    /// and no Stop has got through since: one is sent every
    /// [`REASSERT_SILENT`] until one does.
    recovering: bool,
    /// The refill pump's last answer: `None` until asked (since boot, or
    /// since its address changed).
    responding: Option<bool>,
    /// When the next Stop goes out. `None`: nothing to send.
    pub(super) next_stop: Option<Instant>,
    /// A Test is running until then.
    test_until: Option<Instant>,
    /// The pump may be running (a Test, a Start whose answer was lost): a
    /// Stop is due every [`RETRY_OWED`] until one is acknowledged.
    pub(super) stop_owed: bool,
    /// Stops not acknowledged in a row.
    pub(super) stop_fails: u32,
    /// The run's refill levels (`refill_cycle`), `None` outside such a run.
    pub(super) plan: Option<super::refill_cycle::Plan>,
    pub(super) phase: super::refill_cycle::Phase,
    /// A fill was cut by a lost balance read: it goes on to the high level
    /// once the balance answers, not only under the low level.
    pub(super) resume_fill: bool,
    /// What the balance saw of the fill under way.
    pub(super) fill: super::refill_cycle::FillTrack,
    /// The first balance read missed in a row during a fill: the fill is
    /// cut only once they have been missing for `REFILL_BLIND_S`.
    pub(super) blind_since: Option<jiff::Timestamp>,
}

/// The refill pump's part of the status frame. `None` there while nothing
/// drives it and nothing is owed to it.
#[derive(Debug, Clone, Serialize)]
pub struct RefillStatus {
    /// The active run drives it.
    pub in_run: bool,
    pub address: u8,
    /// `None` until it has been asked.
    pub responding: Option<bool>,
    /// A Test is running it now.
    pub testing: bool,
    /// The run's automatic refill, `None` outside such a run.
    pub cycle: Option<super::refill_cycle::CycleStatus>,
}

impl<T: Transport> Engine<T> {
    /// Apply `[refill]` (at boot and on each Settings save). A Test still
    /// running at an old address is stopped there first; a run driving the
    /// refill pump asks the new address at once.
    pub fn set_refill(&mut self, cfg: RefillConfig, now: Instant) {
        let moved = self.refill.cfg.pump_address != cfg.pump_address;
        let new_speed = (self.refill.cfg.speed_rpm != cfg.speed_rpm).then_some(cfg.speed_rpm);
        if moved && self.refill.stop_owed && self.refill_line_up() {
            self.refill_stop(now);
        }
        self.refill.cfg = cfg;
        // The speed is written at each fill's start: a change during a fill
        // goes to the pump now, or the fill ran on at the old one (run 210).
        if let (false, Some(rpm)) = (moved, new_speed) {
            self.refill_speed_now(rpm);
        }
        if moved {
            self.refill.responding = None;
            self.refill.test_until = None;
            self.refill.stop_owed = false;
            if self.refill.run || self.refill.recovering {
                self.refill.next_stop = Some(now);
            }
        }
    }

    /// At boot: a run left open that drove the refill pump may have left it
    /// running (step 3's cycle): a Stop is the first frame on the line.
    pub fn refill_after_boot(&mut self, now: Instant) {
        if self.store.running_run().ok().flatten().is_some_and(|r| r.auto_refill) {
            self.refill.recovering = true;
            self.refill.next_stop = Some(now);
        }
    }

    pub fn refill_status(&self) -> Option<RefillStatus> {
        let r = &self.refill;
        (r.run || r.recovering || r.stop_owed).then(|| RefillStatus {
            in_run: r.run,
            address: r.cfg.pump_address,
            responding: r.responding,
            testing: r.test_until.is_some(),
            cycle: self.refill_cycle_status(),
        })
    }

    /// When the control loop has refill work next (end of a Test, a Stop).
    pub fn refill_next_due(&self) -> Option<Instant> {
        match (self.refill.test_until, self.refill.next_stop) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The refill pump can be talked to: the line is up and it is the real
    /// one (the stand-in left by a port that would not open proves nothing).
    pub(super) fn refill_line_up(&self) -> bool {
        !self.serial_lost && !(self.on_simulator() && self.serial_is_real())
    }

    /// The control loop's turn: end a Test that is over, or send the Stop
    /// that is due. Returns whether the status changed.
    pub fn refill_service(&mut self, now: Instant) -> bool {
        if !self.refill_next_due().is_some_and(|due| now >= due) {
            return false;
        }
        if !self.refill_line_up() {
            // The feed pump's recovery owns the line; ask again after it.
            self.refill.next_stop = Some(now + RETRY_OWED);
            return false;
        }
        if self.refill_filling() && !self.refill.stop_owed {
            // The cycle runs it on purpose: no reassert until it stops.
            self.refill.next_stop = Some(now + REASSERT);
            return false;
        }
        let before = (self.refill.responding, self.refill.test_until.is_some());
        self.refill_stop(now);
        before != (self.refill.responding, self.refill.test_until.is_some())
    }

    /// Stop the refill pump now, and schedule the next Stop.
    pub(super) fn refill_stop(&mut self, now: Instant) -> bool {
        let addr = self.refill.cfg.pump_address;
        let ok = self.pump.with_address(addr, |p| p.stop()).is_ok();
        // The Test is over either way; a Stop that did not get through is
        // `stop_owed`'s to retry, on its own cadence.
        self.refill.test_until = None;
        if ok {
            self.refill.stop_owed = false;
            self.refill.recovering = false;
            self.refill.stop_fails = 0;
        } else {
            self.refill.stop_fails = self.refill.stop_fails.saturating_add(1);
        }
        self.refill_answered(ok);
        let r = &self.refill;
        self.refill.next_stop = if r.stop_owed {
            Some(now + RETRY_OWED)
        } else if r.run && ok {
            Some(now + REASSERT)
        } else if r.run || r.recovering {
            Some(now + REASSERT_SILENT)
        } else {
            None
        };
        ok
    }

    /// Record whether the refill pump answered, journalling a change.
    fn refill_answered(&mut self, ok: bool) {
        let was = self.refill.responding.replace(ok);
        let addr = self.refill.cfg.pump_address;
        let (kind, level, detail) = match (was, ok) {
            (Some(true) | None, false) => (
                "refill_pump_lost",
                EventLevel::Warn,
                format!(
                    "the refill pump (address {addr}) does not answer: check its power, its RS-485 wiring and its MODBUS address"
                ),
            ),
            (Some(false), true) => (
                "refill_pump_ok",
                EventLevel::Info,
                format!("the refill pump (address {addr}) answers again"),
            ),
            _ => return,
        };
        if kind == "refill_pump_lost" {
            tracing::warn!("{detail}");
        } else {
            tracing::info!("{detail}");
        }
        let _ = self.store.log_event(&NewEvent {
            run_id: self.active.as_ref().map(|a| a.id),
            wall_time: Timestamp::now(),
            level,
            kind: kind.into(),
            detail: Some(detail),
        });
    }

    /// A run that drives the refill pump is about to start: it must answer a
    /// Stop first, or the run is refused (a refill that cannot happen would
    /// leave the bottle to run dry).
    pub(super) fn refill_before_run(&mut self, feed_addr: u8, now: Instant) -> Result<()> {
        let addr = self.refill.cfg.pump_address;
        if addr == feed_addr {
            return Err(EngineError::Config(format!(
                "the refill pump has the feed pump's MODBUS address ({addr}): give it its own in Settings, Refill pump"
            )));
        }
        self.refill.test_until = None;
        if !self.refill_stop(now) {
            return Err(EngineError::RefillPumpSilent { address: addr });
        }
        Ok(())
    }

    /// The run now active drives the refill pump.
    pub(super) fn refill_run_started(&mut self, now: Instant) {
        self.refill.run = true;
        self.refill.recovering = false;
        self.refill.next_stop = Some(now + REASSERT);
    }

    /// A run that drove the refill pump is resuming: a Stop first, before the
    /// feed pump moves. A silent refill pump does not hold the resume up (the
    /// culture needs its feed more).
    pub(super) fn refill_resume(&mut self, now: Instant) {
        self.refill.test_until = None;
        if self.refill_line_up() {
            self.refill_stop(now);
        }
    }

    /// The resumed run drives the refill pump again; a silent one is asked
    /// every minute.
    pub(super) fn refill_run_resumed(&mut self, now: Instant) {
        self.refill_run_started(now);
        if self.refill.responding != Some(true) {
            self.refill.next_stop = Some(now + REASSERT_SILENT);
        }
    }

    /// The run ended (stopped, aborted, or discarded after a crash): a last
    /// Stop; one that does not get through is retried every minute.
    pub(super) fn refill_run_ended(&mut self, now: Instant) {
        if self.refill.plan.is_some() {
            self.refill_plan_end();
        }
        if !(self.refill.run || self.refill.recovering) {
            return;
        }
        self.refill.run = false;
        self.refill.recovering = true;
        if self.refill_line_up() {
            self.refill_stop(now);
        } else {
            self.refill.next_stop = Some(now + RETRY_OWED);
        }
    }

    /// Settings' Test: run the refill pump [`TEST_RUN`] at its configured
    /// speed and direction, then stop it (the control loop sends the Stop).
    /// Refused during a run: a Test there would pour into the weighed bottle
    /// mid-regulation.
    pub fn refill_test(&mut self, now: Instant) -> std::result::Result<(), String> {
        if self.active.is_some() {
            return Err("Not during a run: stop the run first, the refill pump would pour into the weighed bottle.".into());
        }
        if !self.refill_line_up() {
            return Err("The pump line is not connected: connect the feed pump first, the refill pump shares its cable.".into());
        }
        if self.refill.test_until.is_some() {
            return Err("A test is already running.".into());
        }
        let cfg = self.refill.cfg.clone();
        if cfg.pump_address == self.pump_addr {
            return Err(format!(
                "Address {} is the feed pump's: set another address in the refill pump's own menu, then here.",
                cfg.pump_address
            ));
        }
        let res = self.pump.with_address(cfg.pump_address, |p| {
            p.set_speed_rpm(cfg.speed_rpm as f32)?;
            p.set_direction(cfg.direction.clockwise())?;
            p.start()
        });
        // Whatever happened, a Stop follows: a Start whose answer was lost
        // may have reached the pump.
        self.refill.stop_owed = true;
        match res {
            Ok(()) => {
                self.refill_answered(true);
                self.refill.test_until = Some(now + TEST_RUN);
                self.refill.next_stop = Some(now + TEST_RUN);
                Ok(())
            }
            Err(e) => {
                self.refill_answered(false);
                self.refill.next_stop = Some(now);
                Err(format!(
                    "The refill pump at address {} did not answer ({e}). Check its power, its RS-485 wiring and the MODBUS address set in its own menu.",
                    cfg.pump_address
                ))
            }
        }
    }

    /// Settings > Pumps: does the refill pump answer at its address? A read
    /// of its speed register, never a Start, allowed during a run (the
    /// control thread serialises it with the feed pump's writes). Outside a
    /// run nothing else ever asks it, so its state was unknown there.
    pub fn refill_ping(&mut self) -> std::result::Result<bool, String> {
        if !self.refill_line_up() {
            return Err("The pump line is not connected: the refill pump shares the feed pump's cable.".into());
        }
        let addr = self.refill.cfg.pump_address;
        if addr == self.pump_addr {
            return Err(format!("Address {addr} is the feed pump's: give the refill pump another one."));
        }
        let ok = self.pump.with_address(addr, |p| p.read_speed_rpm()).is_ok();
        self.refill_answered(ok);
        Ok(ok)
    }

    /// A run is starting: a Test still running is cut short.
    pub(super) fn refill_end_test(&mut self, now: Instant) {
        if self.refill.test_until.is_some() && self.refill_line_up() {
            self.refill_stop(now);
        }
    }

    /// The daemon is going down: one last Stop to a pump that may be running.
    pub fn refill_shutdown(&mut self) {
        let r = &self.refill;
        if (r.run || r.recovering || r.stop_owed) && self.refill_line_up() {
            self.refill_stop(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Direction;
    use crate::engine::RunConfig;
    use crate::store::{RunStatus, Store};
    use fermentool_modbus::{Pump, SimBus};

    fn engine() -> Engine<SimBus> {
        let mut e = Engine::new(Pump::new(SimBus::new(), 1), Store::open_in_memory().unwrap(), "test");
        e.pump_addr = 1;
        e
    }

    fn at(addr: u8) -> RefillConfig {
        RefillConfig { pump_address: addr, ..RefillConfig::default() }
    }

    fn refill_pump(e: &Engine<SimBus>, addr: u8) -> &fermentool_modbus::SimPump {
        e.pump.transport().get(addr).expect("the refill pump was addressed")
    }

    fn kinds(e: &Engine<SimBus>, run_id: i64) -> Vec<String> {
        e.store.events(Some(run_id), 1000).unwrap().into_iter().map(|ev| ev.kind).collect()
    }

    /// A trimmed dosing run that drives the refill pump, on a balance under
    /// the feed bottle.
    fn refill_run(e: &mut Engine<SimBus>) -> RunConfig {
        e.attach_scale_for_test(Box::new(fermentool_modbus::SimPump::new(9)));
        e.live_weight = Some((1100.0, true));
        RunConfig {
            gravimetric_trim: true,
            auto_refill: true,
            refill_draw_g: Some(550.0),
            control_var: crate::store::ControlVar::MlMin,
            ..super::super::tests::linear_cfg()
        }
    }

    #[test]
    fn a_ping_tells_whether_the_refill_pump_answers_and_never_starts_it() {
        // Settings > Pumps shows each pump's state, outside a run too.
        let mut e = engine();
        e.set_refill(at(2), Instant::now());
        assert_eq!(e.refill_ping(), Ok(true));
        assert!(!refill_pump(&e, 2).running(), "a ping only reads");
        e.pump.transport_mut().set_plugged(2, false);
        assert_eq!(e.refill_ping(), Ok(false));
    }

    #[test]
    fn a_ping_at_the_feed_pumps_address_is_refused() {
        let mut e = engine();
        e.set_refill(at(1), Instant::now());
        assert!(e.refill_ping().is_err());
    }

    #[test]
    fn nothing_is_sent_to_the_refill_pump_outside_a_run_that_drives_it() {
        let mut e = engine();
        let t0 = Instant::now();
        e.set_refill(at(2), t0);
        e.refill_after_boot(t0);
        assert!(!e.refill_service(t0 + Duration::from_secs(60)));
        e.start_run(super::super::tests::linear_cfg(), Timestamp::now()).unwrap();
        e.refill_service(t0 + Duration::from_secs(120));
        assert!(e.pump.transport().get(2).is_none());
        assert!(e.refill_status().is_none());
    }

    #[test]
    fn a_run_that_drives_it_stops_it_first_then_every_ten_seconds() {
        let mut e = engine();
        let t0 = Instant::now();
        e.set_refill(at(2), t0);
        e.pump.with_address(2, |p| p.start()).unwrap();
        let cfg = refill_run(&mut e);
        e.start_run(cfg, Timestamp::now()).unwrap();
        assert!(!refill_pump(&e, 2).running(), "stopped before the run started");
        let st = e.refill_status().unwrap();
        assert!(st.in_run);
        assert_eq!(st.responding, Some(true));
        assert!(e.status().active.unwrap().auto_refill);

        // A LabQ that restarted by itself after a power cut.
        let now = Instant::now();
        e.pump.with_address(2, |p| p.start()).unwrap();
        e.refill_service(now + Duration::from_secs(5));
        assert!(refill_pump(&e, 2).running(), "not before the next reassert");
        e.refill_service(now + REASSERT);
        assert!(!refill_pump(&e, 2).running());
        assert_eq!(e.pump.address(), 1, "the feed pump is addressed again");

        // The run's end: a last Stop, then nothing more.
        e.pump.with_address(2, |p| p.start()).unwrap();
        e.stop_run(Timestamp::now()).unwrap();
        assert!(!refill_pump(&e, 2).running());
        assert!(e.refill_status().is_none());
        assert_eq!(e.refill_next_due(), None);
    }

    #[test]
    fn a_run_is_refused_when_its_refill_pump_does_not_answer() {
        let mut e = engine();
        e.pump.transport_mut().set_plugged(2, false);
        e.set_refill(at(2), Instant::now());
        let cfg = refill_run(&mut e);
        let err = e.start_run(cfg, Timestamp::now()).unwrap_err();
        assert!(matches!(err, EngineError::RefillPumpSilent { address: 2 }), "{err}");
        assert!(err.hint().is_some());
        assert!(e.status().active.is_none());
        assert!(e.pump.transport().get(1).is_none_or(|p| !p.running()), "the feed pump never started");
    }

    #[test]
    fn a_refill_run_needs_the_balance_regulation_and_its_own_address() {
        let mut e = engine();
        e.set_refill(at(2), Instant::now());
        let cfg = refill_run(&mut e);
        let no_trim = RunConfig { gravimetric_trim: false, ..cfg.clone() };
        assert!(e.start_run(no_trim, Timestamp::now()).is_err());
        e.set_refill(at(1), Instant::now());
        let err = e.start_run(cfg, Timestamp::now()).unwrap_err();
        assert!(err.to_string().contains("feed pump's MODBUS address"), "{err}");
    }

    #[test]
    fn a_silent_refill_pump_mid_run_is_journalled_once_and_asked_less_often() {
        let mut e = engine();
        e.set_refill(at(2), Instant::now());
        let cfg = refill_run(&mut e);
        let id = e.start_run(cfg, Timestamp::now()).unwrap();
        let t0 = Instant::now();
        e.pump.transport_mut().set_plugged(2, false);
        e.refill_service(t0 + REASSERT);
        assert_eq!(e.refill_status().unwrap().responding, Some(false));
        assert_eq!(e.refill_next_due(), Some(t0 + REASSERT + REASSERT_SILENT));
        e.refill_service(t0 + REASSERT + REASSERT_SILENT);
        assert_eq!(kinds(&e, id).iter().filter(|k| *k == "refill_pump_lost").count(), 1);

        e.pump.transport_mut().set_plugged(2, true);
        e.refill_service(t0 + REASSERT + 2 * REASSERT_SILENT);
        assert_eq!(e.refill_status().unwrap().responding, Some(true));
        assert!(kinds(&e, id).contains(&"refill_pump_ok".to_string()));
    }

    #[test]
    fn after_a_crash_the_refill_pump_is_stopped_first_and_driven_again_on_resume() {
        let db = super::super::tests::TempDb::new();
        {
            let mut a = Engine::new(Pump::new(SimBus::new(), 1), db.store(), "test");
            a.set_refill(at(2), Instant::now());
            let cfg = refill_run(&mut a);
            a.start_run(cfg, Timestamp::now()).unwrap();
        } // crash: the run is left open
        let mut b = Engine::new(Pump::new(SimBus::new(), 1), db.store(), "test");
        let t0 = Instant::now();
        b.set_refill(at(2), t0);
        b.refill_after_boot(t0);
        assert_eq!(b.refill_next_due(), Some(t0), "the first frame on the line");
        b.pump.with_address(2, |p| p.start()).unwrap();
        b.refill_service(t0);
        assert!(!refill_pump(&b, 2).running());

        b.pump.with_address(2, |p| p.start()).unwrap();
        b.resume(Timestamp::now(), Duration::from_secs(300)).unwrap();
        assert!(!refill_pump(&b, 2).running(), "stopped before the feed pump moved");
        assert!(b.refill_status().unwrap().in_run);
    }

    #[test]
    fn discarding_a_crashed_refill_run_stops_the_refill_pump() {
        let db = super::super::tests::TempDb::new();
        {
            let mut a = Engine::new(Pump::new(SimBus::new(), 1), db.store(), "test");
            a.set_refill(at(2), Instant::now());
            let cfg = refill_run(&mut a);
            a.start_run(cfg, Timestamp::now()).unwrap();
        }
        let mut b = Engine::new(Pump::new(SimBus::new(), 1), db.store(), "test");
        b.set_refill(at(2), Instant::now());
        b.refill_after_boot(Instant::now());
        b.pump.with_address(2, |p| p.start()).unwrap();
        b.discard_recovery(Timestamp::now(), RunStatus::Stopped).unwrap();
        assert!(!refill_pump(&b, 2).running());
        assert!(b.refill_status().is_none());
    }

    #[test]
    fn a_test_runs_the_refill_pump_for_three_seconds_in_its_direction() {
        let mut e = engine();
        let t0 = Instant::now();
        e.set_refill(RefillConfig { speed_rpm: 120.0, direction: Direction::Ccw, ..at(4) }, t0);
        e.refill_test(t0).unwrap();
        let p = refill_pump(&e, 4);
        assert!(p.running());
        assert_eq!(p.speed_rpm(), 120.0);
        assert!(!p.direction_cw());
        assert!(e.refill_status().unwrap().testing);
        assert!(e.refill_test(t0).is_err(), "one test at a time");

        assert!(!e.refill_service(t0 + Duration::from_secs(2)));
        assert!(refill_pump(&e, 4).running());
        assert!(e.refill_service(t0 + TEST_RUN));
        assert!(!refill_pump(&e, 4).running());
        assert!(e.refill_status().is_none(), "nothing owed once stopped");
        assert_eq!(e.refill_next_due(), None);
        assert!(e.pump.transport().get(1).is_none(), "the feed pump was never touched");
    }

    #[test]
    fn a_test_on_the_feed_pumps_address_is_refused() {
        let mut e = engine();
        e.set_refill(at(1), Instant::now());
        assert!(e.refill_test(Instant::now()).unwrap_err().contains("feed pump's"));
        assert!(e.pump.transport().get(1).is_none());
    }

    #[test]
    fn a_stop_lost_after_a_test_is_sent_again_every_second() {
        let mut e = engine();
        let t0 = Instant::now();
        e.set_refill(at(2), t0);
        e.refill_test(t0).unwrap();
        e.pump.transport_mut().pump(2).drop_next = 2; // the Stop and its retry
        e.refill_service(t0 + TEST_RUN);
        assert!(refill_pump(&e, 2).running());
        assert_eq!(e.refill_status().unwrap().responding, Some(false));
        assert_eq!(e.refill_next_due(), Some(t0 + TEST_RUN + RETRY_OWED));
        e.refill_service(t0 + TEST_RUN + RETRY_OWED);
        assert!(!refill_pump(&e, 2).running());
        assert_eq!(e.refill_next_due(), None);
    }

    #[test]
    fn a_test_of_a_silent_address_says_what_to_check() {
        let mut e = engine();
        let t0 = Instant::now();
        e.pump.transport_mut().set_plugged(5, false);
        e.set_refill(at(5), t0);
        let err = e.refill_test(t0).unwrap_err();
        assert!(err.contains("address 5"), "{err}");
        assert!(!e.refill_status().unwrap().testing);
    }

    #[test]
    fn a_test_is_refused_during_a_run_and_cut_short_by_a_start() {
        let mut e = engine();
        let t0 = Instant::now();
        e.set_refill(at(2), t0);
        e.refill_test(t0).unwrap();
        e.start_run(super::super::tests::linear_cfg(), Timestamp::now()).unwrap();
        assert!(!refill_pump(&e, 2).running(), "the start stopped the test");
        assert!(e.refill_test(t0 + Duration::from_secs(1)).unwrap_err().contains("during a run"));
    }

    #[test]
    fn moving_the_address_stops_a_test_at_the_old_one() {
        let mut e = engine();
        let t0 = Instant::now();
        e.set_refill(at(2), t0);
        e.refill_test(t0).unwrap();
        e.set_refill(at(3), t0 + Duration::from_secs(1));
        assert!(!refill_pump(&e, 2).running());
        assert!(e.pump.transport().get(3).is_none(), "nothing to ask at the new one outside a run");
    }

    #[test]
    fn the_shutdown_stops_a_refill_pump_driven_by_the_run() {
        let mut e = engine();
        e.set_refill(at(2), Instant::now());
        let cfg = refill_run(&mut e);
        e.start_run(cfg, Timestamp::now()).unwrap();
        e.pump.with_address(2, |p| p.start()).unwrap();
        e.refill_shutdown();
        assert!(!refill_pump(&e, 2).running());
    }
}
