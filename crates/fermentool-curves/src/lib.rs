//! Pure, I/O-free time-profile math for Fermentool.
//!
//! A [`CurveSpec`] maps elapsed time to a pump setpoint (rpm or ml/min). It is a
//! pure function of elapsed time, feed it the real wall-clock elapsed after a
//! restart and it returns exactly where the pump should be, which is what makes
//! crash-resume correct (`docs/IMPLEMENTATION_PLAN.md` §4.4, §4.7).
//!
//! Shapes: [`CurveKind::Linear`], [`CurveKind::Exponential`] (fed-batch),
//! [`CurveKind::Sigmoid`] (logistic), [`CurveKind::Step`],
//! [`CurveKind::Constant`], [`CurveKind::Custom`] (piecewise).
//!
//! Non-trivial shapes take their shape constant in one of two [`ParamMode`]s:
//!
//! * [`ParamMode::Endpoints`], you give `start`, `end`, `duration`; the shape
//!   constant is derived so the curve passes through both endpoints.
//! * [`ParamMode::Physio`], you give `start`, `duration` and a physiological
//!   rate (µ, slope, k); `end` is derived. Read it back with
//!   [`CurveSpec::effective_end`] / [`CurveSpec::resolved`].

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Default dimensionless steepness for an endpoints-mode sigmoid.
pub const DEFAULT_SIGMOID_STEEPNESS: f64 = 8.0;

/// Family of time profile, mirrors the `runs.curve_kind` column and the UI select.
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

impl CurveKind {
    /// The `snake_case` token used in storage and the API (matches the serde name).
    pub fn as_str(self) -> &'static str {
        match self {
            CurveKind::Linear => "linear",
            CurveKind::Exponential => "exponential",
            CurveKind::Sigmoid => "sigmoid",
            CurveKind::Step => "step",
            CurveKind::Constant => "constant",
            CurveKind::Custom => "custom",
        }
    }
}

/// How the shape constant of a non-trivial curve is supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamMode {
    /// Start value, end value and duration are given; the shape constant is derived.
    Endpoints,
    /// A physiological rate is given; the end value is derived from it.
    Physio,
}

impl ParamMode {
    /// The `snake_case` token used in storage and the API.
    pub fn as_str(self) -> &'static str {
        match self {
            ParamMode::Endpoints => "endpoints",
            ParamMode::Physio => "physio",
        }
    }
}

/// Interpolation between [`CustomPoint`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interp {
    Linear,
    Hold,
}

/// Parameters for [`CurveKind::Linear`].
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct LinearParams {
    /// Physio mode only: setpoint change per hour. `end = start + rate_per_hour · duration_h`.
    pub rate_per_hour: f64,
}

/// Parameters for [`CurveKind::Exponential`] (classic fed-batch feeding).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ExponentialParams {
    /// Physio mode only: specific rate µ per hour. `end = start · e^(µ · duration_h)`.
    pub mu_per_hour: f64,
}

/// Parameters for [`CurveKind::Sigmoid`] (logistic S-curve).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SigmoidParams {
    /// Endpoints mode: dimensionless steepness `a` (~4..12; [`DEFAULT_SIGMOID_STEEPNESS`]).
    pub steepness: f64,
    /// Endpoints mode: inflection position as a fraction of the run, in `[0, 1]`.
    pub midpoint_frac: f64,
    /// Physio mode: logistic rate `k` per hour.
    pub k_per_hour: f64,
    /// Physio mode: inflection time in hours from start.
    pub midpoint_hours: f64,
}

impl Default for SigmoidParams {
    fn default() -> Self {
        Self {
            steepness: DEFAULT_SIGMOID_STEEPNESS,
            midpoint_frac: 0.5,
            k_per_hour: 1.0,
            midpoint_hours: 0.0,
        }
    }
}

/// One piece of a [`CurveParams::Step`] profile: hold `value` from `at_seconds`
/// until the next segment (or the end of the run).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StepSegment {
    pub at_seconds: f64,
    pub value: f64,
}

/// One point of a [`CurveParams::Custom`] profile.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CustomPoint {
    pub at_seconds: f64,
    pub value: f64,
}

/// Kind-specific parameters. Serialised into `runs.curve_params`; the `kind` tag
/// is the single source of truth for [`CurveParams::kind`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CurveParams {
    Linear(LinearParams),
    Exponential(ExponentialParams),
    Sigmoid(SigmoidParams),
    Step {
        segments: Vec<StepSegment>,
    },
    Constant,
    Custom {
        points: Vec<CustomPoint>,
        interp: Interp,
    },
}

impl CurveParams {
    /// The [`CurveKind`] this parameter set describes.
    pub fn kind(&self) -> CurveKind {
        match self {
            CurveParams::Linear(_) => CurveKind::Linear,
            CurveParams::Exponential(_) => CurveKind::Exponential,
            CurveParams::Sigmoid(_) => CurveKind::Sigmoid,
            CurveParams::Step { .. } => CurveKind::Step,
            CurveParams::Constant => CurveKind::Constant,
            CurveParams::Custom { .. } => CurveKind::Custom,
        }
    }
}

/// Everything needed to evaluate a setpoint over time. Serialised as-is into
/// `runs.curve_params` so a run can be replayed or resumed after a restart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurveSpec {
    pub mode: ParamMode,
    /// Setpoint at `elapsed = 0`.
    pub start: f64,
    /// Setpoint at `elapsed = duration`. In [`ParamMode::Physio`] for linear and
    /// exponential this is *derived*, use [`CurveSpec::effective_end`].
    pub end: f64,
    #[serde(with = "duration_secs")]
    pub duration: Duration,
    /// Lower/upper safety clamp. The engine overrides these with the real pump
    /// limits (0.1..=350 rpm, or 0..=99999 ml/min) before a run starts.
    pub clamp_min: f64,
    pub clamp_max: f64,
    pub params: CurveParams,
}

impl CurveSpec {
    fn base(
        mode: ParamMode,
        start: f64,
        end: f64,
        duration: Duration,
        params: CurveParams,
    ) -> Self {
        Self {
            mode,
            start,
            end,
            duration,
            clamp_min: 0.0,
            clamp_max: f64::MAX,
            params,
        }
    }

    /// `start` → `end` straight line over `duration`.
    pub fn linear(start: f64, end: f64, duration: Duration) -> Self {
        Self::base(
            ParamMode::Endpoints,
            start,
            end,
            duration,
            CurveParams::Linear(LinearParams::default()),
        )
    }

    /// Hold `value` for the whole run.
    pub fn constant(value: f64, duration: Duration) -> Self {
        Self::base(
            ParamMode::Endpoints,
            value,
            value,
            duration,
            CurveParams::Constant,
        )
    }

    /// Exponential curve through `start` and `end` (both must be > 0).
    pub fn exponential_endpoints(start: f64, end: f64, duration: Duration) -> Self {
        Self::base(
            ParamMode::Endpoints,
            start,
            end,
            duration,
            CurveParams::Exponential(ExponentialParams::default()),
        )
    }

    /// Exponential fed-batch feeding `S · e^(µ·t)`; `end` is derived.
    pub fn exponential_physio(start: f64, mu_per_hour: f64, duration: Duration) -> Self {
        let mut c = Self::base(
            ParamMode::Physio,
            start,
            start,
            duration,
            CurveParams::Exponential(ExponentialParams { mu_per_hour }),
        );
        c.end = c.effective_end();
        c
    }

    /// Logistic S-curve through `start` and `end`, default steepness.
    pub fn sigmoid_endpoints(start: f64, end: f64, duration: Duration) -> Self {
        Self::base(
            ParamMode::Endpoints,
            start,
            end,
            duration,
            CurveParams::Sigmoid(SigmoidParams::default()),
        )
    }

    /// Builder: set the safety clamp.
    pub fn with_clamp(mut self, min: f64, max: f64) -> Self {
        self.clamp_min = min;
        self.clamp_max = max;
        self
    }

    /// The [`CurveKind`] of this spec.
    pub fn kind(&self) -> CurveKind {
        self.params.kind()
    }

    /// Run length in hours.
    pub fn duration_hours(&self) -> f64 {
        self.duration.as_secs_f64() / 3600.0
    }

    /// The concrete end value, resolving Physio-mode linear / exponential.
    pub fn effective_end(&self) -> f64 {
        match (self.mode, &self.params) {
            (ParamMode::Physio, CurveParams::Linear(p)) => {
                self.start + p.rate_per_hour * self.duration_hours()
            }
            (ParamMode::Physio, CurveParams::Exponential(p)) => {
                self.start * (p.mu_per_hour * self.duration_hours()).exp()
            }
            _ => self.end,
        }
    }

    /// A copy with `end` materialised from [`effective_end`], for storage / display.
    pub fn resolved(&self) -> CurveSpec {
        let mut c = self.clone();
        c.end = self.effective_end();
        c
    }

    /// Setpoint for a given elapsed time, already clamped to `[clamp_min, clamp_max]`.
    ///
    /// This is the single entry point the tick engine calls. Pure in `elapsed`,
    /// which is why crash-resume is correct: pass the real elapsed time and it
    /// returns where the pump should be now.
    pub fn value_at(&self, elapsed: Duration) -> f64 {
        if self.duration.is_zero() {
            return self.end.clamp(self.clamp_min, self.clamp_max);
        }

        let s = self.start;
        let e = self.effective_end();
        let d = self.duration.as_secs_f64();
        let t = elapsed.as_secs_f64().clamp(0.0, d);
        let p = t / d;

        let raw = match &self.params {
            CurveParams::Constant => s,
            CurveParams::Linear(_) => lerp(s, e, p),
            CurveParams::Exponential(_) => {
                if s > 0.0 && e > 0.0 {
                    s * (e / s).powf(p)
                } else {
                    // Invalid config; validate() rejects it upstream.
                    lerp(s, e, p)
                }
            }
            CurveParams::Sigmoid(sp) => {
                sigmoid_value(s, e, p, t / 3600.0, d / 3600.0, self.mode, sp)
            }
            CurveParams::Step { segments } => step_value(s, segments, t),
            CurveParams::Custom { points, interp } => custom_value(s, e, p, points, *interp, t),
        };

        // `validate()` rejects specs whose derived `end` isn't finite, but keep
        // `value_at` total anyway: a non-finite `end` can make `lerp` yield NaN
        // (`s + inf·0`), and a NaN setpoint must never leave this function.
        // Fall back to `start`, a known-good value.
        let raw = if raw.is_nan() { s } else { raw };
        // Not `f64::clamp`, it panics if a bound is NaN or `min > max`.
        // `validate()` forbids both, but a spec that skipped validation (e.g.
        // straight into `preview`) must still get a number, not a panic: treat a
        // non-finite or inverted bound as "no limit on that side".
        let mut v = raw;
        if self.clamp_min.is_finite() && v < self.clamp_min {
            v = self.clamp_min;
        }
        if self.clamp_max.is_finite() && v > self.clamp_max {
            v = self.clamp_max;
        }
        v
    }

    /// Sample the curve into `(elapsed_seconds, value)` pairs for the UI preview
    /// chart. `samples` is the point count including both endpoints (min 2).
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

    /// Structural validation. Returns a human-readable message on the first problem.
    pub fn validate(&self) -> Result<(), String> {
        if self.duration.is_zero() {
            return Err("duration must be greater than zero".into());
        }
        if !self.start.is_finite() || !self.end.is_finite() {
            return Err("start and end must be finite numbers".into());
        }
        if !self.clamp_min.is_finite() || !self.clamp_max.is_finite() {
            return Err("clamp_min and clamp_max must be finite".into());
        }
        if self.clamp_min > self.clamp_max {
            return Err("clamp_min must not exceed clamp_max".into());
        }
        if self.clamp_min < 0.0 {
            return Err("clamp_min must be >= 0".into());
        }
        // A Physio rate can be finite yet still blow `end` up to ±inf over a
        // long duration (`start * e^(µ·hours)`), and inf feeds NaN through
        // `lerp`/`powf` in `value_at`. Reject it here so a NaN setpoint can
        // never reach the pump layer.
        if !self.effective_end().is_finite() {
            return Err(
                "the derived end value is not finite, the physiological rate is too large for this duration"
                    .into(),
            );
        }

        let d = self.duration.as_secs_f64();
        match &self.params {
            CurveParams::Constant => {}
            CurveParams::Linear(p) => {
                if self.mode == ParamMode::Physio && !p.rate_per_hour.is_finite() {
                    return Err("linear rate_per_hour must be finite".into());
                }
            }
            CurveParams::Exponential(p) => match self.mode {
                ParamMode::Endpoints => {
                    if self.start <= 0.0 || self.effective_end() <= 0.0 {
                        return Err("exponential (endpoints) needs start > 0 and end > 0".into());
                    }
                }
                ParamMode::Physio => {
                    if !p.mu_per_hour.is_finite() {
                        return Err("exponential mu_per_hour must be finite".into());
                    }
                    if self.start <= 0.0 {
                        return Err("exponential (physio) needs start > 0".into());
                    }
                }
            },
            CurveParams::Sigmoid(p) => {
                if self.mode == ParamMode::Endpoints {
                    if !p.steepness.is_finite() || p.steepness <= 0.0 {
                        return Err("sigmoid steepness must be > 0".into());
                    }
                    if !(0.0..=1.0).contains(&p.midpoint_frac) {
                        return Err("sigmoid midpoint_frac must be within [0, 1]".into());
                    }
                } else if !p.k_per_hour.is_finite() {
                    return Err("sigmoid k_per_hour must be finite".into());
                }
            }
            CurveParams::Step { segments } => {
                if segments.is_empty() {
                    return Err("step profile needs at least one segment".into());
                }
                let mut prev = f64::NEG_INFINITY;
                for seg in segments {
                    if !seg.at_seconds.is_finite() || !seg.value.is_finite() {
                        return Err("step segment values must be finite".into());
                    }
                    if seg.at_seconds < 0.0 || seg.at_seconds > d {
                        return Err("step segment at_seconds must be within [0, duration]".into());
                    }
                    if seg.at_seconds < prev {
                        return Err("step segments must be ordered by at_seconds".into());
                    }
                    prev = seg.at_seconds;
                }
            }
            CurveParams::Custom { points, .. } => {
                if points.len() < 2 {
                    return Err("custom profile needs at least two points".into());
                }
                let mut prev = f64::NEG_INFINITY;
                for pt in points {
                    if !pt.at_seconds.is_finite() || !pt.value.is_finite() {
                        return Err("custom point values must be finite".into());
                    }
                    if pt.at_seconds < 0.0 || pt.at_seconds > d {
                        return Err("custom point at_seconds must be within [0, duration]".into());
                    }
                    if pt.at_seconds <= prev {
                        return Err("custom points must strictly increase in at_seconds".into());
                    }
                    prev = pt.at_seconds;
                }
            }
        }
        Ok(())
    }
}

fn lerp(a: f64, b: f64, p: f64) -> f64 {
    a + (b - a) * p
}

fn logistic(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

fn sigmoid_value(
    s: f64,
    e: f64,
    p: f64,
    t_hours: f64,
    dur_hours: f64,
    mode: ParamMode,
    sp: &SigmoidParams,
) -> f64 {
    match mode {
        ParamMode::Physio => {
            // A raw logistic does not sit on `s` at t=0 or `e` at t=duration.
            // Rescale it onto `[0, dur_hours]` the same way the endpoints form
            // is normalised, so the curve hits both endpoints exactly for any
            // `k_per_hour` / `midpoint_hours` (and stays monotonic, including
            // `k_per_hour < 0`).
            let g = |th: f64| logistic(sp.k_per_hour * (th - sp.midpoint_hours));
            let lo = g(0.0);
            let hi = g(dur_hours);
            if (hi - lo).abs() < 1e-12 {
                lerp(s, e, p)
            } else {
                s + (e - s) * (g(t_hours) - lo) / (hi - lo)
            }
        }
        ParamMode::Endpoints => {
            let a = if sp.steepness > 0.0 {
                sp.steepness
            } else {
                DEFAULT_SIGMOID_STEEPNESS
            };
            let m = sp.midpoint_frac.clamp(0.0, 1.0);
            let lo = logistic(a * (0.0 - m));
            let hi = logistic(a * (1.0 - m));
            if (hi - lo).abs() < 1e-12 {
                lerp(s, e, p)
            } else {
                s + (e - s) * (logistic(a * (p - m)) - lo) / (hi - lo)
            }
        }
    }
}

fn step_value(start: f64, segments: &[StepSegment], t: f64) -> f64 {
    // Order-independent: the value of the latest segment whose start is <= t.
    let mut v = start;
    let mut best = f64::NEG_INFINITY;
    for seg in segments {
        if seg.at_seconds <= t + 1e-9 && seg.at_seconds >= best {
            best = seg.at_seconds;
            v = seg.value;
        }
    }
    v
}

fn custom_value(
    start: f64,
    end: f64,
    p: f64,
    points: &[CustomPoint],
    interp: Interp,
    t: f64,
) -> f64 {
    match points.len() {
        0 => return lerp(start, end, p),
        1 => return points[0].value,
        _ => {}
    }

    let first = points[0];
    let last = points[points.len() - 1];
    if t <= first.at_seconds {
        return first.value;
    }
    if t >= last.at_seconds {
        return last.value;
    }

    // points are ascending in at_seconds (validate() enforces it).
    let mut i = 0;
    while i + 1 < points.len() && points[i + 1].at_seconds <= t {
        i += 1;
    }
    let a = points[i];
    let b = points[i + 1];
    match interp {
        Interp::Hold => a.value,
        Interp::Linear => {
            let span = b.at_seconds - a.at_seconds;
            if span.abs() < 1e-9 {
                b.value
            } else {
                a.value + (b.value - a.value) * (t - a.at_seconds) / span
            }
        }
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

    const EPS: f64 = 1e-6;
    const HOUR: u64 = 3600;

    fn hours(h: u64) -> Duration {
        Duration::from_secs(h * HOUR)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < EPS
    }

    #[test]
    fn linear_endpoints_hits_start_mid_end() {
        let c = CurveSpec::linear(10.0, 100.0, hours(1));
        assert!(close(c.value_at(Duration::ZERO), 10.0));
        assert!(close(c.value_at(Duration::from_secs(1800)), 55.0));
        assert!(close(c.value_at(hours(1)), 100.0));
        assert!(close(c.value_at(hours(5)), 100.0)); // held past the end
    }

    #[test]
    fn linear_physio_derives_end_from_slope() {
        let c = CurveSpec {
            mode: ParamMode::Physio,
            start: 10.0,
            end: 0.0,
            duration: hours(4),
            clamp_min: 0.0,
            clamp_max: f64::MAX,
            params: CurveParams::Linear(LinearParams { rate_per_hour: 5.0 }),
        };
        assert!(close(c.effective_end(), 30.0));
        assert!(close(c.value_at(hours(2)), 20.0));
        assert!(close(c.resolved().end, 30.0));
    }

    #[test]
    fn constant_ignores_time() {
        let c = CurveSpec::constant(42.0, hours(10));
        assert!(close(c.value_at(Duration::ZERO), 42.0));
        assert!(close(c.value_at(hours(7)), 42.0));
    }

    #[test]
    fn exponential_endpoints_is_geometric() {
        let c = CurveSpec::exponential_endpoints(1.0, 100.0, hours(2));
        assert!(close(c.value_at(Duration::ZERO), 1.0));
        assert!(close(c.value_at(hours(1)), 10.0)); // geometric mean of 1 and 100
        assert!(close(c.value_at(hours(2)), 100.0));
    }

    #[test]
    fn exponential_physio_matches_closed_form() {
        let mu = 2.0_f64.ln(); // doubling per hour
        let c = CurveSpec::exponential_physio(2.0, mu, hours(3));
        assert!(close(c.effective_end(), 16.0));
        assert!(close(c.value_at(hours(1)), 4.0));
        assert!(close(c.value_at(hours(2)), 8.0));

        // ...and equals the endpoints form built from the derived end.
        let eq = CurveSpec::exponential_endpoints(2.0, 16.0, hours(3));
        assert!(close(c.value_at(hours(1)), eq.value_at(hours(1))));
    }

    #[test]
    fn sigmoid_endpoints_are_exact_and_symmetric() {
        let c = CurveSpec::sigmoid_endpoints(10.0, 110.0, hours(1));
        assert!(close(c.value_at(Duration::ZERO), 10.0));
        assert!(close(c.value_at(hours(1)), 110.0));
        assert!(close(c.value_at(Duration::from_secs(1800)), 60.0)); // midpoint = mean
    }

    #[test]
    fn sigmoid_physio_stays_within_endpoints_and_crosses_mean_at_midpoint() {
        let c = CurveSpec {
            mode: ParamMode::Physio,
            start: 0.0,
            end: 100.0,
            duration: hours(10),
            clamp_min: 0.0,
            clamp_max: f64::MAX,
            params: CurveParams::Sigmoid(SigmoidParams {
                steepness: 0.0,
                midpoint_frac: 0.0,
                k_per_hour: 1.5,
                midpoint_hours: 5.0,
            }),
        };
        // Normalised: sits exactly on the endpoints, and a midpoint at the
        // half-way mark still crosses the mean by symmetry.
        assert!(close(c.value_at(Duration::ZERO), 0.0));
        assert!(close(c.value_at(hours(10)), 100.0));
        assert!(close(c.value_at(hours(5)), 50.0));
        let mut prev = f64::NEG_INFINITY;
        for i in 0..=100 {
            let v = c.value_at(Duration::from_secs_f64(i as f64 * 360.0));
            assert!((0.0..=100.0).contains(&v));
            assert!(v >= prev - EPS, "sigmoid physio should be monotonic");
            prev = v;
        }
    }

    #[test]
    fn sigmoid_physio_hits_endpoints_for_offset_midpoint_and_negative_rate() {
        // Inflection well off-centre: a raw logistic would start far from `start`
        // and finish short of `end`; the normalised form pins both.
        let rising = CurveSpec {
            mode: ParamMode::Physio,
            start: 10.0,
            end: 40.0,
            duration: hours(8),
            clamp_min: 0.0,
            clamp_max: f64::MAX,
            params: CurveParams::Sigmoid(SigmoidParams {
                steepness: 0.0,
                midpoint_frac: 0.0,
                k_per_hour: 1.0,
                midpoint_hours: 2.0,
            }),
        };
        assert!(close(rising.value_at(Duration::ZERO), 10.0));
        assert!(close(rising.value_at(hours(8)), 40.0));

        // Negative rate with the same endpoints: still runs start -> end,
        // monotonically (the normalisation flips sign with the denominator).
        let falling_rate = CurveSpec {
            params: CurveParams::Sigmoid(SigmoidParams {
                k_per_hour: -1.0,
                midpoint_hours: 6.0,
                ..SigmoidParams::default()
            }),
            ..rising.clone()
        };
        assert!(close(falling_rate.value_at(Duration::ZERO), 10.0));
        assert!(close(falling_rate.value_at(hours(8)), 40.0));
        let mut prev = f64::NEG_INFINITY;
        for i in 0..=100 {
            let v = falling_rate.value_at(Duration::from_secs_f64(i as f64 * 288.0));
            assert!((10.0..=40.0).contains(&v));
            assert!(v >= prev - EPS, "monotonic regardless of k sign");
            prev = v;
        }
    }

    #[test]
    fn step_holds_each_segment_and_is_order_independent() {
        let c = CurveSpec {
            mode: ParamMode::Endpoints,
            start: 5.0,
            end: 5.0,
            duration: hours(3),
            clamp_min: 0.0,
            clamp_max: f64::MAX,
            params: CurveParams::Step {
                segments: vec![
                    StepSegment {
                        at_seconds: 7200.0,
                        value: 30.0,
                    },
                    StepSegment {
                        at_seconds: 3600.0,
                        value: 20.0,
                    },
                ],
            },
        };
        assert!(close(c.value_at(Duration::from_secs(600)), 5.0)); // before first
        assert!(close(c.value_at(Duration::from_secs(3600)), 20.0));
        assert!(close(c.value_at(Duration::from_secs(5400)), 20.0));
        assert!(close(c.value_at(Duration::from_secs(7200)), 30.0));
        assert!(close(c.value_at(hours(3)), 30.0));
    }

    #[test]
    fn custom_linear_and_hold_interpolation() {
        let pts = vec![
            CustomPoint {
                at_seconds: 0.0,
                value: 10.0,
            },
            CustomPoint {
                at_seconds: 100.0,
                value: 20.0,
            },
            CustomPoint {
                at_seconds: 200.0,
                value: 20.0,
            },
        ];
        let lin = CurveSpec {
            mode: ParamMode::Endpoints,
            start: 10.0,
            end: 20.0,
            duration: Duration::from_secs(200),
            clamp_min: 0.0,
            clamp_max: f64::MAX,
            params: CurveParams::Custom {
                points: pts.clone(),
                interp: Interp::Linear,
            },
        };
        assert!(close(lin.value_at(Duration::from_secs(50)), 15.0));
        assert!(close(lin.value_at(Duration::from_secs(150)), 20.0));
        assert!(close(lin.value_at(Duration::from_secs(200)), 20.0));

        let hold = CurveSpec {
            params: CurveParams::Custom {
                points: pts,
                interp: Interp::Hold,
            },
            ..lin
        };
        assert!(close(hold.value_at(Duration::from_secs(50)), 10.0));
        assert!(close(hold.value_at(Duration::from_secs(199)), 20.0));
    }

    #[test]
    fn value_is_clamped_both_ways() {
        let c = CurveSpec::linear(0.0, 500.0, hours(1)).with_clamp(20.0, 350.0);
        assert!(close(c.value_at(Duration::ZERO), 20.0));
        assert!(close(c.value_at(hours(1)), 350.0));
    }

    #[test]
    fn preview_spans_the_run() {
        let c = CurveSpec::linear(10.0, 100.0, hours(1));
        let pts = c.preview(11);
        assert_eq!(pts.len(), 11);
        assert!(close(pts.first().unwrap().1, 10.0));
        assert!(close(pts.last().unwrap().1, 100.0));
    }

    #[test]
    fn monotonic_when_rising() {
        for c in [
            CurveSpec::linear(1.0, 100.0, hours(1)),
            CurveSpec::exponential_endpoints(1.0, 100.0, hours(1)),
            CurveSpec::sigmoid_endpoints(1.0, 100.0, hours(1)),
        ] {
            let mut prev = f64::NEG_INFINITY;
            for i in 0..=200 {
                let v = c.value_at(Duration::from_secs_f64(i as f64 * 18.0));
                assert!(v >= prev - EPS, "{:?} should be non-decreasing", c.kind());
                prev = v;
            }
        }
    }

    #[test]
    fn validate_accepts_good_specs_and_rejects_bad_ones() {
        assert!(CurveSpec::linear(1.0, 10.0, hours(1)).validate().is_ok());
        assert!(CurveSpec::exponential_endpoints(1.0, 10.0, hours(1))
            .validate()
            .is_ok());

        assert!(CurveSpec::linear(1.0, 10.0, Duration::ZERO)
            .validate()
            .is_err());
        assert!(CurveSpec::exponential_endpoints(0.0, 10.0, hours(1))
            .validate()
            .is_err());

        let empty_step = CurveSpec {
            params: CurveParams::Step { segments: vec![] },
            ..CurveSpec::constant(1.0, hours(1))
        };
        assert!(empty_step.validate().is_err());

        let bad_custom = CurveSpec {
            duration: Duration::from_secs(100),
            params: CurveParams::Custom {
                points: vec![
                    CustomPoint {
                        at_seconds: 50.0,
                        value: 1.0,
                    },
                    CustomPoint {
                        at_seconds: 50.0,
                        value: 2.0,
                    },
                ],
                interp: Interp::Linear,
            },
            ..CurveSpec::constant(1.0, Duration::from_secs(100))
        };
        assert!(bad_custom.validate().is_err());

        // A finite µ / rate that still overflows `end` to ±inf over a long run
        // must be rejected, otherwise `value_at` can emit NaN.
        let exp_overflow = CurveSpec::exponential_physio(2.0, 20.0, hours(100));
        assert!(!exp_overflow.effective_end().is_finite());
        assert!(exp_overflow.validate().is_err());

        let lin_overflow = CurveSpec {
            mode: ParamMode::Physio,
            params: CurveParams::Linear(LinearParams { rate_per_hour: 1e307 }),
            ..CurveSpec::linear(1.0, 1.0, hours(100))
        };
        assert!(!lin_overflow.effective_end().is_finite());
        assert!(lin_overflow.validate().is_err());

        // And even if a caller ignores validate(), value_at never emits NaN.
        for c in [&exp_overflow, &lin_overflow] {
            for frac in [0.0, 0.25, 0.5, 1.0] {
                let v = c.value_at(Duration::from_secs_f64(frac * 100.0 * 3600.0));
                assert!(!v.is_nan(), "value_at produced NaN at frac {frac}");
            }
        }

        // A non-finite clamp bound is rejected by validate(), and value_at still
        // returns a number instead of panicking in `f64::clamp` when a caller
        // skips validation.
        let bad_clamp = CurveSpec {
            clamp_max: f64::NAN,
            ..CurveSpec::linear(1.0, 2.0, hours(1))
        };
        assert!(bad_clamp.validate().is_err());
        let v = bad_clamp.value_at(Duration::from_secs_f64(1800.0));
        assert!(v.is_finite(), "value_at must stay total for a NaN clamp bound");
    }

    #[test]
    fn every_param_variant_round_trips_through_json() {
        let specs = vec![
            CurveSpec::linear(1.0, 2.0, hours(1)),
            CurveSpec::exponential_physio(1.0, 0.3, hours(1)),
            CurveSpec::sigmoid_endpoints(1.0, 2.0, hours(1)),
            CurveSpec::constant(3.0, hours(1)),
            CurveSpec {
                params: CurveParams::Step {
                    segments: vec![StepSegment {
                        at_seconds: 0.0,
                        value: 1.0,
                    }],
                },
                ..CurveSpec::constant(1.0, hours(1))
            },
            CurveSpec {
                params: CurveParams::Custom {
                    points: vec![
                        CustomPoint {
                            at_seconds: 0.0,
                            value: 1.0,
                        },
                        CustomPoint {
                            at_seconds: 3600.0,
                            value: 2.0,
                        },
                    ],
                    interp: Interp::Hold,
                },
                ..CurveSpec::constant(1.0, hours(1))
            },
        ];
        for s in specs {
            let json = serde_json::to_string(&s).unwrap();
            let back: CurveSpec = serde_json::from_str(&json).unwrap();
            assert_eq!(s, back, "round-trip mismatch for {:?}", s.kind());
        }
    }
}
