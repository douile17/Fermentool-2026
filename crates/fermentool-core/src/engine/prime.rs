//! Priming the feed line, from the tubing calibration (2026-10-28): the feed
//! pump at its top speed, the way the calibration turns, until Stop or
//! [`PRIME_MAX`]. Not a run: nothing is weighed or counted; its start, a turn
//! and its end are journalled, with no run. Asked after a measurement pumped
//! the wrong way and emptied the line back into the bottle, where the
//! operator held the pump's own prime key for a minute.
//!
//! The end at [`PRIME_MAX`] is the control loop's ([`Engine::prime_next_due`],
//! [`Engine::prime_service`]), as the refill pump's Test: a page closed while
//! priming never leaves the pump running for good.

use std::time::{Duration, Instant};

use fermentool_modbus::{PumpTransport, Transport};
use jiff::Timestamp;
use serde::Serialize;

use super::{Engine, EngineError, Result};
use crate::config::Direction;
use crate::store::{EventLevel, NewEvent};

/// Priming stops by itself after this: a line fills in about a minute.
pub const PRIME_MAX: Duration = Duration::from_secs(300);

/// The line being primed.
#[derive(Debug, Clone)]
pub(super) struct Priming {
    direction: Direction,
    /// When the pump last started this way: a turn counts again from zero.
    since: Instant,
    since_wall: Timestamp,
    rpm: f64,
}

/// Priming, for the status: the calibration page shows it, from any window.
#[derive(Debug, Clone, Serialize)]
pub struct PrimingStatus {
    pub direction: Direction,
    /// When the pump started this way, so a page opened meanwhile counts on.
    pub since: Timestamp,
    pub max_s: u64,
    pub rpm: f64,
}

fn arrow(d: Direction) -> &'static str {
    match d {
        Direction::Cw => "cw",
        Direction::Ccw => "ccw",
    }
}

/// "1 min 04 s", "42 s": how long the line was primed, for the journal.
fn took(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s} s")
    } else {
        format!("{} min {:02} s", s / 60, s % 60)
    }
}

impl<T: Transport> Engine<T> {
    /// Prime the line `direction` at the pump's top speed. Asked again the
    /// other way while priming, the pump stops, then turns that way (the
    /// count starts again); the same way, nothing changes. Refused during a
    /// run, over a lost link, and on the simulator unless it is allowed.
    pub fn prime_start(&mut self, direction: Direction, now: Instant, wall: Timestamp) -> Result<()> {
        if self.active.is_some() {
            return Err(EngineError::Busy);
        }
        if self.serial_lost {
            return Err(EngineError::SerialDown);
        }
        if self.on_simulator() && !self.allow_simulator {
            return Err(EngineError::SimulatorNotAllowed);
        }
        let turn = match &self.priming {
            Some(p) if p.direction == direction => return Ok(()),
            Some(p) => Some(p.direction),
            None => None,
        };
        let rpm = self.feed.max_rpm();
        let res = (|| {
            if turn.is_some() {
                self.pump.stop()?;
            }
            // A LabV or PDS II head takes a speed only in transferring mode.
            self.pump.ensure_transferring().map_err(|e| EngineError::PumpMode(self.feed.kind, e))?;
            self.pump.set_direction(direction.clockwise())?;
            self.pump.set_speed_rpm(rpm as f32)?;
            self.pump.start()?;
            Ok(())
        })();
        if let Err(e) = res {
            // A Start whose answer was lost may have reached the pump.
            self.priming = None;
            self.stop_or_keep_pending(None, wall);
            return Err(e);
        }
        self.priming = Some(Priming { direction, since: now, since_wall: wall, rpm });
        let detail = match turn {
            Some(from) => format!("turned from {} to {}", arrow(from), arrow(direction)),
            None => format!("at {rpm} rpm, {}", arrow(direction)),
        };
        let _ = self.store.log_event(&NewEvent {
            run_id: None,
            wall_time: wall,
            level: EventLevel::Info,
            kind: "prime".into(),
            detail: Some(detail),
        });
        Ok(())
    }

    /// Stop priming. A Stop that does not get through stays pending, sent
    /// again until it does.
    pub fn prime_stop(&mut self, now: Instant, wall: Timestamp) -> Result<()> {
        self.prime_end(now, wall, None)
    }

    fn prime_end(&mut self, now: Instant, wall: Timestamp, why: Option<&str>) -> Result<()> {
        let Some(p) = self.priming.take() else {
            return Err(EngineError::Idle);
        };
        self.stop_or_keep_pending(None, wall);
        let mut detail = format!("after {}", took(now.saturating_duration_since(p.since)));
        if let Some(why) = why {
            detail = format!("{detail}, {why}");
        }
        let _ = self.store.log_event(&NewEvent {
            run_id: None,
            wall_time: wall,
            level: EventLevel::Info,
            kind: "prime_stop".into(),
            detail: Some(detail),
        });
        Ok(())
    }

    /// When priming ends by itself, for the control loop's wait.
    pub fn prime_next_due(&self) -> Option<Instant> {
        self.priming.as_ref().map(|p| p.since + PRIME_MAX)
    }

    /// The control loop's turn: stop priming at [`PRIME_MAX`]. Returns
    /// whether it stopped (the status changed).
    pub fn prime_service(&mut self, now: Instant, wall: Timestamp) -> bool {
        if !self.prime_next_due().is_some_and(|due| now >= due) {
            return false;
        }
        self.prime_end(now, wall, Some("the 5 min limit")).is_ok()
    }

    /// A run starts or the daemon goes down: priming ends first.
    pub fn prime_end_for(&mut self, now: Instant, wall: Timestamp, why: &str) {
        if self.priming.is_some() {
            let _ = self.prime_end(now, wall, Some(why));
        }
    }

    pub(super) fn priming_status(&self) -> Option<PrimingStatus> {
        self.priming.as_ref().map(|p| PrimingStatus {
            direction: p.direction,
            since: p.since_wall,
            max_s: PRIME_MAX.as_secs(),
            rpm: p.rpm,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{engine, linear_cfg};

    fn kinds(e: &Engine<fermentool_modbus::SimPump>) -> Vec<(String, String)> {
        let mut ev = e.store.events(None, 100).unwrap();
        ev.sort_by_key(|x| x.id);
        ev.into_iter()
            .filter(|x| x.kind.starts_with("prime"))
            .map(|x| (x.kind, x.detail.unwrap_or_default()))
            .collect()
    }

    #[test]
    fn priming_runs_the_pump_flat_out_the_way_asked_until_stop() {
        let mut e = engine();
        let t = Instant::now();
        e.prime_start(Direction::Ccw, t, Timestamp::now()).unwrap();
        let p = e.pump.transport();
        assert!(p.running());
        assert!(!p.direction_cw());
        assert!((p.speed_rpm() as f64 - e.feed.max_rpm()).abs() < 1e-3);
        assert_eq!(e.status().priming.unwrap().direction, Direction::Ccw);
        e.prime_stop(t + Duration::from_secs(64), Timestamp::now()).unwrap();
        assert!(!e.pump.transport().running());
        assert!(e.status().priming.is_none());
        assert_eq!(
            kinds(&e),
            vec![
                ("prime".into(), format!("at {} rpm, ccw", e.feed.max_rpm())),
                ("prime_stop".into(), "after 1 min 04 s".into()),
            ]
        );
    }

    #[test]
    fn the_other_way_while_priming_turns_the_pump_around() {
        let mut e = engine();
        let t = Instant::now();
        e.prime_start(Direction::Cw, t, Timestamp::now()).unwrap();
        e.prime_start(Direction::Ccw, t + Duration::from_secs(20), Timestamp::now()).unwrap();
        assert!(e.pump.transport().running());
        assert!(!e.pump.transport().direction_cw());
        // the count starts again with the new way
        assert_eq!(e.prime_next_due(), Some(t + Duration::from_secs(20) + PRIME_MAX));
        assert_eq!(kinds(&e)[1], ("prime".into(), "turned from cw to ccw".into()));
        // the same way again changes nothing
        e.prime_start(Direction::Ccw, t + Duration::from_secs(30), Timestamp::now()).unwrap();
        assert_eq!(kinds(&e).len(), 2);
    }

    #[test]
    fn priming_stops_by_itself_at_the_limit() {
        let mut e = engine();
        let t = Instant::now();
        e.prime_start(Direction::Cw, t, Timestamp::now()).unwrap();
        assert!(!e.prime_service(t + PRIME_MAX - Duration::from_secs(1), Timestamp::now()));
        assert!(e.pump.transport().running());
        assert!(e.prime_service(t + PRIME_MAX, Timestamp::now()));
        assert!(!e.pump.transport().running());
        assert_eq!(kinds(&e)[1], ("prime_stop".into(), "after 5 min 00 s, the 5 min limit".into()));
    }

    #[test]
    fn priming_is_refused_during_a_run_and_ends_when_one_starts() {
        let mut e = engine();
        let t = Instant::now();
        e.prime_start(Direction::Cw, t, Timestamp::now()).unwrap();
        e.start_run(linear_cfg(), Timestamp::now()).unwrap();
        assert!(e.status().priming.is_none(), "the run took the pump over");
        assert!(kinds(&e).iter().any(|(k, d)| k == "prime_stop" && d.ends_with("a run started")));
        assert!(matches!(e.prime_start(Direction::Cw, t, Timestamp::now()), Err(EngineError::Busy)));
    }

    #[test]
    fn stop_without_priming_is_idle() {
        let mut e = engine();
        assert!(matches!(e.prime_stop(Instant::now(), Timestamp::now()), Err(EngineError::Idle)));
    }
}
