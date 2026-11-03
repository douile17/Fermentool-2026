//! Per-minute timing summaries (count, p50, p99, max) of the loops that must
//! stay on time, logged under the `timing` target. A baseline to measure a
//! heavier neighbour (the camera) against; nothing here touches the pumps.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    /// How late a setpoint write started against its 150 ms schedule.
    SetpointLate,
    /// Interval between control ticks.
    Tick,
    /// Age of the balance reading when the engine takes it.
    ScaleAge,
    /// Duration of one balance poll.
    ScalePoll,
}

impl Metric {
    pub fn name(self) -> &'static str {
        match self {
            Metric::SetpointLate => "setpoint_late",
            Metric::Tick => "tick",
            Metric::ScaleAge => "scale_age",
            Metric::ScalePoll => "scale_poll",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

const ALL: [Metric; 4] = [
    Metric::SetpointLate,
    Metric::Tick,
    Metric::ScaleAge,
    Metric::ScalePoll,
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub count: usize,
    pub p50: f64,
    pub p99: f64,
    pub max: f64,
}

/// Nearest-rank percentiles of a window of samples (ms).
pub fn summarize(samples: &[f64]) -> Option<Summary> {
    if samples.is_empty() {
        return None;
    }
    let mut v = samples.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    let rank = |p: f64| v[((p * n as f64).ceil() as usize).clamp(1, n) - 1];
    Some(Summary {
        count: n,
        p50: rank(0.50),
        p99: rank(0.99),
        max: v[n - 1],
    })
}

pub struct Timing {
    window_start: Instant,
    samples: [Vec<f64>; 4],
}

impl Timing {
    pub fn new(now: Instant) -> Timing {
        Timing {
            window_start: now,
            samples: Default::default(),
        }
    }

    pub fn record(&mut self, m: Metric, ms: f64) {
        self.samples[m.index()].push(ms);
    }

    /// The window's summaries once 60 s have passed, then the window restarts
    /// at `now`. Only metrics with samples; `None` for a quiet minute.
    pub fn take_if_due(&mut self, now: Instant) -> Option<Vec<(Metric, Summary)>> {
        if now.saturating_duration_since(self.window_start) < WINDOW {
            return None;
        }
        self.window_start = now;
        let out: Vec<_> = ALL
            .iter()
            .filter_map(|&m| {
                let s = summarize(&self.samples[m.index()]);
                self.samples[m.index()].clear();
                s.map(|s| (m, s))
            })
            .collect();
        (!out.is_empty()).then_some(out)
    }
}

static GLOBAL: OnceLock<Mutex<Timing>> = OnceLock::new();

fn global() -> &'static Mutex<Timing> {
    GLOBAL.get_or_init(|| Mutex::new(Timing::new(Instant::now())))
}

/// Record one sample (ms) from any thread.
pub fn record(m: Metric, ms: f64) {
    if let Ok(mut t) = global().lock() {
        t.record(m, ms);
    }
}

/// Log the minute's summaries if it is over. Cheap when it is not.
pub fn flush_if_due(now: Instant) {
    let due = match global().lock() {
        Ok(mut t) => t.take_if_due(now),
        Err(_) => None,
    };
    let round = |x: f64| (x * 10.0).round() / 10.0;
    for (m, s) in due.unwrap_or_default() {
        tracing::info!(
            target: "timing",
            metric = m.name(),
            count = s.count,
            p50_ms = round(s.p50),
            p99_ms = round(s.p99),
            max_ms = round(s.max),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_uses_nearest_rank() {
        let s: Vec<f64> = (1..=100).map(f64::from).collect();
        let r = summarize(&s).unwrap();
        assert_eq!((r.count, r.p50, r.p99, r.max), (100, 50.0, 99.0, 100.0));
    }

    #[test]
    fn summarize_of_nothing_is_none() {
        assert!(summarize(&[]).is_none());
    }

    #[test]
    fn take_if_due_waits_a_minute_then_clears() {
        let t0 = Instant::now();
        let mut t = Timing::new(t0);
        t.record(Metric::Tick, 2.0);
        assert!(t.take_if_due(t0 + Duration::from_secs(59)).is_none());
        let out = t.take_if_due(t0 + Duration::from_secs(60)).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0.name(), "tick");
        // cleared, window restarted at 60 s
        assert!(t.take_if_due(t0 + Duration::from_secs(121)).is_none());
    }

    #[test]
    fn a_quiet_minute_reports_nothing() {
        let t0 = Instant::now();
        let mut t = Timing::new(t0);
        assert!(t.take_if_due(t0 + Duration::from_secs(61)).map_or(true, |v| v.is_empty()));
    }
}
