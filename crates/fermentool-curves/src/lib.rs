//! Pure, I/O-free time-profile math for Fermentool.
//!
//! Milestone 1 (this file): the type skeleton plus working `Linear` and
//! `Constant` evaluation, with endpoint/clamp invariants under test.
//!
//! Milestone 2 fills in `Exponential` (fed-batch), `Sigmoid`, `Step` and
//! `Custom`, each in both [`ParamMode`]s, per `docs/IMPLEMENTATION_PLAN.md` §4.4.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Which family of time profile a run follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveKind {
    Linear,
    Exponential,
    Sigmoid,
    Step,
    Constant,
    Custom,
}

/// How the non-trivial curve parameters are supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamMode {
    /// Start value, end value and duration are given; the curve constant is derived.
    Endpoints,
    /// A physiological parameter (mu, k, ...) is given; the end value is derived.
    Physio,
}

/// Everything needed to evaluate a setpoint over time.
///
/// Serialised as-is into `runs.curve_params` so a run can be replayed or resumed
/// after a restart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurveSpec {
    pub kind: CurveKind,
    pub mode: ParamMode,
    /// Setpoint at `elapsed = 0`.
    pub start: f64,
    /// Setpoint at `elapsed = duration` (Endpoints mode; derived in Physio mode).
    pub end: f64,
    #[serde(with = "duration_secs")]
    pub duration: Duration,
    /// Lower/upper safety clamp, already intersected with the pump's own limits
    /// (0.1..=350 rpm, or 0..=99999 ml/min).
    pub clamp_min: f64,
    pub clamp_max: f64,
}

impl CurveSpec {
    /// Setpoint for a given elapsed time, already clamped to `[clamp_min, clamp_max]`.
    ///
    /// This is the single entry point the engine calls every tick. It is a pure
    /// function of `elapsed`, which is why crash-resume is correct: feed it the
    /// real wall-clock elapsed time and it returns where the pump should be now.
    pub fn value_at(&self, elapsed: Duration) -> f64 {
        let raw = match self.kind {
            CurveKind::Constant => self.start,
            CurveKind::Linear => self.linear(elapsed),

            // Milestone 2 — fall back to linear for now so the shape is defined.
            CurveKind::Exponential
            | CurveKind::Sigmoid
            | CurveKind::Step
            | CurveKind::Custom => self.linear(elapsed),
        };
        raw.clamp(self.clamp_min, self.clamp_max)
    }

    /// Sample the curve into `(elapsed_seconds, value)` pairs for the UI preview
    /// chart. `samples` is the number of points including both endpoints.
    pub fn preview(&self, samples: usize) -> Vec<(f64, f64)> {
        let n = samples.max(2);
        let total = self.duration.as_secs_f64();
        (0..n)
            .map(|i| {
                let t = total * (i as f64) / ((n - 1) as f64);
                (t, self.value_at(Duration::from_secs_f64(t)))
            })
            .collect()
    }

    /// Normalised progress in `[0, 1]`.
    fn progress(&self, elapsed: Duration) -> f64 {
        if self.duration.is_zero() {
            return 1.0;
        }
        (elapsed.as_secs_f64() / self.duration.as_secs_f64()).clamp(0.0, 1.0)
    }

    fn linear(&self, elapsed: Duration) -> f64 {
        let p = self.progress(elapsed);
        self.start + (self.end - self.start) * p
    }
}

mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        Ok(Duration::from_secs(u64::deserialize(d)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: CurveKind) -> CurveSpec {
        CurveSpec {
            kind,
            mode: ParamMode::Endpoints,
            start: 10.0,
            end: 100.0,
            duration: Duration::from_secs(3600),
            clamp_min: 0.1,
            clamp_max: 350.0,
        }
    }

    #[test]
    fn linear_hits_both_endpoints() {
        let s = spec(CurveKind::Linear);
        assert!((s.value_at(Duration::ZERO) - 10.0).abs() < 1e-9);
        assert!((s.value_at(Duration::from_secs(1800)) - 55.0).abs() < 1e-9);
        assert!((s.value_at(Duration::from_secs(3600)) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn past_end_holds_final_value() {
        let s = spec(CurveKind::Linear);
        assert!((s.value_at(Duration::from_secs(9_999)) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn value_is_clamped_both_ways() {
        let mut s = spec(CurveKind::Linear);
        s.clamp_max = 50.0;
        assert!((s.value_at(Duration::from_secs(3600)) - 50.0).abs() < 1e-9);
        s.clamp_min = 20.0;
        assert!((s.value_at(Duration::ZERO) - 20.0).abs() < 1e-9);
    }

    #[test]
    fn constant_ignores_time() {
        let s = spec(CurveKind::Constant);
        assert!((s.value_at(Duration::ZERO) - 10.0).abs() < 1e-9);
        assert!((s.value_at(Duration::from_secs(123)) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn preview_spans_the_run() {
        let s = spec(CurveKind::Linear);
        let pts = s.preview(11);
        assert_eq!(pts.len(), 11);
        assert!((pts.first().unwrap().1 - 10.0).abs() < 1e-9);
        assert!((pts.last().unwrap().1 - 100.0).abs() < 1e-9);
    }

    #[test]
    fn spec_round_trips_through_json() {
        let s = spec(CurveKind::Exponential);
        let json = serde_json::to_string(&s).unwrap();
        let back: CurveSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
