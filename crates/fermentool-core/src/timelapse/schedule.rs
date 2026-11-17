//! When a timelapse's images are due: slot k at `anchor + (k - anchor_slot)
//! * interval`, on the wall clock. Pure: no clock read here.

use jiff::{SignedDuration, Timestamp};

/// A slot due this long ago is still taken (the thread wakes a little late).
pub const LATE_S: i64 = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Schedule {
    pub anchor: Timestamp,
    pub anchor_slot: i64,
    pub interval_s: i64,
}

impl Schedule {
    pub fn due_at(&self, slot: i64) -> Timestamp {
        self.anchor + SignedDuration::from_secs((slot - self.anchor_slot) * self.interval_s.max(1))
    }

    /// The next slot to take after `after_slot`: the first not due before
    /// `now - LATE_S`, never one already past `after_slot` (a clock set back
    /// gives no burst) nor below the anchor.
    pub fn next_slot(&self, after_slot: Option<i64>, now: Timestamp) -> i64 {
        let interval = self.interval_s.max(1) as f64;
        // in fractions of a second: whole seconds stretched the grace to 3 s
        let since = now.duration_since(self.anchor).as_secs_f64() - LATE_S as f64;
        let by_clock = self.anchor_slot + if since <= 0.0 { 0 } else { (since / interval).ceil() as i64 };
        by_clock.max(after_slot.map_or(self.anchor_slot, |s| s + 1))
    }

    /// Slots between the last taken (`None`: none yet) and `next`, none
    /// taken: the missed ones.
    pub fn missed_between(&self, last_slot: Option<i64>, next: i64) -> i64 {
        match last_slot {
            Some(l) => (next - l - 1).max(0),
            None => (next - self.anchor_slot).max(0),
        }
    }

    /// The same timelapse every `interval_s`: the next slot keeps its time
    /// under the old interval and anchors the new one.
    pub fn with_interval(&self, interval_s: i64, after_slot: Option<i64>, now: Timestamp) -> Schedule {
        let next = self.next_slot(after_slot, now);
        Schedule { anchor: self.due_at(next), anchor_slot: next, interval_s }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        format!("2026-11-05T{s}Z").parse().unwrap()
    }

    fn every_5_min() -> Schedule {
        Schedule { anchor: ts("10:00:00"), anchor_slot: 0, interval_s: 300 }
    }

    #[test]
    fn slots_from_the_anchor() {
        assert_eq!(every_5_min().due_at(3), ts("10:15:00"));
    }

    #[test]
    fn next_slot_after_a_restart_skips_and_counts() {
        let s = every_5_min();
        let next = s.next_slot(Some(2), ts("10:31:00"));
        assert_eq!(next, 7);
        assert_eq!(s.due_at(next), ts("10:35:00"));
        assert_eq!(s.missed_between(Some(2), 7), 4);
    }

    #[test]
    fn a_slot_just_passed_is_still_taken() {
        assert_eq!(every_5_min().next_slot(Some(2), ts("10:15:01")), 3);
    }

    #[test]
    fn the_first_image_is_at_the_anchor() {
        let s = every_5_min();
        assert_eq!(s.next_slot(None, ts("10:00:00")), 0);
        assert_eq!(s.missed_between(None, 0), 0);
        // no image yet and three slots gone
        assert_eq!(s.missed_between(None, 3), 3);
    }

    #[test]
    fn the_grace_is_two_seconds_not_three() {
        // 10:15:02.6: slot 3 (10:15) is 2.6 s gone, past the grace
        let now: Timestamp = "2026-11-05T10:15:02.6Z".parse().unwrap();
        assert_eq!(every_5_min().next_slot(Some(2), now), 4);
    }

    #[test]
    fn clock_set_back_never_bursts() {
        let s = every_5_min();
        let next = s.next_slot(Some(10), ts("09:50:00"));
        assert_eq!(next, 11);
        assert_eq!(s.missed_between(Some(10), next), 0);
    }

    #[test]
    fn interval_change_applies_after_the_next_image() {
        let s = every_5_min().with_interval(60, Some(2), ts("10:12:00"));
        assert_eq!((s.anchor, s.anchor_slot, s.interval_s), (ts("10:15:00"), 3, 60));
        assert_eq!(s.due_at(3), ts("10:15:00"));
        assert_eq!(s.due_at(4), ts("10:16:00"));
    }
}
