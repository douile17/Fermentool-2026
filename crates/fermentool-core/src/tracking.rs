//! Cumulative feed tracking: pure math, no I/O, no clocks. The engine feeds
//! it cumulative masses since run start, it returns the pump correction.

use serde::{Deserialize, Serialize};

use crate::trim::{decimate, theil_sen_slope, MAX_SLOPE_POINTS, TRIM_MAX, TRIM_MIN};

/// How far c may move from 1: `[1/(1+p), 1+p]` for a ±p limit, so a pump
/// running short and one running long get the same room. The default ±25 %
/// is `[0.80, 1.25]`. Set per bench in Settings (`[scale] trim_limit_pct`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: f64,
    pub max: f64,
}

impl Bounds {
    pub fn from_limit_pct(pct: f64) -> Self {
        let max = 1.0 + pct.max(0.0) / 100.0;
        Self { min: 1.0 / max, max }
    }

    pub fn clamp(self, c: f64) -> f64 {
        c.clamp(self.min, self.max)
    }
}

impl Default for Bounds {
    fn default() -> Self {
        Self { min: TRIM_MIN, max: TRIM_MAX }
    }
}

/// A slope needs at least this many balance steps of commanded mass, so the
/// balance's rounding is a small part of what it measures. 50 steps (5 g on
/// a 0.1 g balance) give a first estimate within a few %, early in the run;
/// 200 made a 2 ml/min run wait 10 min before any correction.
pub const MIN_WINDOW_RESOLUTIONS: f64 = 50.0;
/// Once this much history exists, the slope is taken over it instead: a 5 g
/// window kept for the whole run made c swing ±10 % in minutes on balance
/// noise and pump pulsation (run 19, 2 ml/min). 20 g, and at least 5 min.
pub const SETTLED_WINDOW_RESOLUTIONS: f64 = 200.0;
pub const SETTLED_WINDOW_S: f64 = 300.0;
/// Largest change of c per update once the ratio rests on a settled window:
/// slow drift (tube wear, temperature) needs no more, and it keeps a noisy
/// update from moving the pump by more than 3 % a minute.
pub const SETTLED_C_STEP: f64 = 0.005;
/// Before the first ratio exists, a deficit this many balance steps wide is
/// read as rounding and start-up noise, not as the pump being off.
pub const EARLY_DEADBAND_RESOLUTIONS: f64 = 3.0;
/// ...and at least this long, so pump pulsation averages out.
pub const MIN_WINDOW_S: f64 = 120.0;
/// Time over which a cumulative deficit is paid back (10 min time constant).
pub const DEFICIT_HORIZON_S: f64 = 600.0;
/// Largest change of c per update, so the pump never jumps.
pub const MAX_C_STEP: f64 = 0.02;
/// c is recomputed at most this often.
pub const UPDATE_EVERY_S: f64 = 10.0;
/// Updates in a row with `1/k` outside the c bounds before alarming (5 min).
pub const ALARM_AFTER_SATURATED_UPDATES: u32 = 30;

/// One `Normal` balance read, cumulative since run start.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrackPoint {
    pub t_s: f64,
    pub commanded_g: f64,
    pub delivered_g: f64,
}

/// The pump's real delivered / commanded ratio, and whether it rests on a
/// settled window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ratio {
    pub k: f64,
    pub settled: bool,
}

/// Theil-Sen slope of delivered vs commanded over the shortest recent window
/// holding the settled mass and time when the history allows it, else over
/// the shortest one holding the minimum. `None` until even that exists.
pub fn delivery_ratio(points: &[TrackPoint], resolution_g: f64) -> Option<Ratio> {
    match window_start(points, resolution_g, SETTLED_WINDOW_RESOLUTIONS, SETTLED_WINDOW_S) {
        Some(s) => slope_from(points, s).map(|k| Ratio { k, settled: true }),
        None => recent_ratio(points, resolution_g).map(|k| Ratio { k, settled: false }),
    }
}

/// The ratio over the shortest recent window holding the minimum mass and
/// time only, never the settled one: what the pump does *now*. Used to see
/// a feed flowing again after an alarm, which a 20 g window still full of
/// the stopped stretch would hide for many minutes.
pub fn recent_ratio(points: &[TrackPoint], resolution_g: f64) -> Option<f64> {
    slope_from(points, window_start(points, resolution_g, MIN_WINDOW_RESOLUTIONS, MIN_WINDOW_S)?)
}

/// Drop the history before the recent window: once an alarm clears, what
/// came before it (a dry bottle, a pinched tube) says nothing of the pump.
pub fn keep_recent(points: &mut Vec<TrackPoint>, resolution_g: f64) {
    if let Some(i) = window_start(points, resolution_g, MIN_WINDOW_RESOLUTIONS, MIN_WINDOW_S) {
        points.drain(..i);
    }
}

fn window_start(points: &[TrackPoint], resolution_g: f64, mass_steps: f64, secs: f64) -> Option<usize> {
    let last = points.last()?;
    let mass = mass_steps * resolution_g;
    points
        .iter()
        .rposition(|p| last.commanded_g - p.commanded_g >= mass && last.t_s - p.t_s >= secs)
}

fn slope_from(points: &[TrackPoint], start: usize) -> Option<f64> {
    let xy: Vec<(f64, f64)> = points[start..]
        .iter()
        .map(|p| (p.commanded_g, p.delivered_g))
        .collect();
    let k = theil_sen_slope(&decimate(&xy, MAX_SLOPE_POINTS))?;
    k.is_finite().then_some(k.max(0.0))
}

/// Why the trim stopped regulating. Raised with c restored to its value from
/// before the problem and frozen; `FeedStopped` and `Saturated` clear by
/// themselves once the feed flows within bounds again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrimAlarm {
    /// The pump runs but the bottle's weight does not move: bottle empty,
    /// line blocked or disconnected.
    FeedStopped,
    /// The pump delivers outside what the correction limit can make up.
    Saturated,
    /// The weight moves the wrong way for the configured balance position.
    WrongSide,
}

impl TrimAlarm {
    pub fn event_kind(self) -> &'static str {
        match self {
            TrimAlarm::FeedStopped => "alarm_feed_stopped",
            TrimAlarm::Saturated => "alarm_saturated",
            TrimAlarm::WrongSide => "alarm_wrong_side",
        }
    }
}

/// The weight must fall this many balance steps for the feed to count as
/// leaving the bottle (one step is rounding and jitter).
pub const FLOW_MOVE_RESOLUTIONS: f64 = 2.0;
/// No such fall for this long, while the pump was told to move at least
/// `FEED_STOP_RESOLUTIONS` balance steps: the feed has stopped. 3 min, and
/// 1 g on a 0.1 g balance (10 g on a 1 g one), so neither a slow feed nor a
/// coarse balance reads as stopped.
pub const FEED_STOP_MIN_S: f64 = 180.0;
pub const FEED_STOP_RESOLUTIONS: f64 = 10.0;

/// Where the feed was last seen leaving the bottle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FlowRef {
    pub t_s: f64,
    pub weight_g: f64,
    pub commanded_g: f64,
}

/// Update the flow reference with a `Normal` read: it moves on each real fall.
pub fn track_flow(r: Option<FlowRef>, t_s: f64, weight_g: f64, commanded_g: f64, resolution_g: f64) -> FlowRef {
    match r {
        Some(r) if r.weight_g - weight_g < FLOW_MOVE_RESOLUTIONS * resolution_g => r,
        _ => FlowRef { t_s, weight_g, commanded_g },
    }
}

/// Whether the pump has been told to move feed that never left the bottle.
pub fn feed_stopped(r: Option<FlowRef>, t_s: f64, commanded_g: f64, resolution_g: f64) -> bool {
    r.is_some_and(|r| {
        t_s - r.t_s >= FEED_STOP_MIN_S && commanded_g - r.commanded_g >= FEED_STOP_RESOLUTIONS * resolution_g
    })
}

/// c as it was at `t_s`, from the `(t_s, c)` samples kept at each update.
pub fn c_at(history: &std::collections::VecDeque<(f64, f64)>, t_s: f64) -> Option<f64> {
    history.iter().rev().find(|(t, _)| *t <= t_s).map(|(_, c)| *c)
}

/// How long the c history reaches back: an alarm's onset is never older.
pub const C_HISTORY_S: f64 = 3600.0;

/// Bound the point history to `cap`: the newer half stays at full density,
/// the older half is evenly thinned. The span back to the first point is
/// kept, so a slow feed still reaches the window's minimum mass.
pub fn thin(points: &[TrackPoint], cap: usize) -> Vec<TrackPoint> {
    if points.len() <= cap || cap < 4 {
        return points.to_vec();
    }
    let newer = points.len() / 2;
    let split = points.len() - newer;
    let old_xy: Vec<(f64, f64)> = (0..split).map(|i| (i as f64, 0.0)).collect();
    let mut out: Vec<TrackPoint> = decimate(&old_xy, cap - newer)
        .into_iter()
        .map(|(i, _)| points[i as usize])
        .collect();
    out.dedup_by(|a, b| a.t_s == b.t_s);
    out.extend_from_slice(&points[split..]);
    out
}

/// Next c: feed-forward `1/k`, plus paying back the cumulative deficit over
/// `DEFICIT_HORIZON_S`, moved by at most `MAX_C_STEP` and kept in bounds.
pub fn next_c(prev_c: f64, k: f64, deficit_g: f64, demand_rate_g_s: f64, bounds: Bounds) -> f64 {
    next_c_stepped(prev_c, k, deficit_g, demand_rate_g_s, MAX_C_STEP, bounds)
}

/// [`next_c`] for a ratio from [`delivery_ratio`]: the full step while the
/// ratio is still a first, short-window estimate, [`SETTLED_C_STEP`] once it
/// rests on a settled window.
pub fn next_c_for(prev_c: f64, ratio: Ratio, deficit_g: f64, demand_rate_g_s: f64, bounds: Bounds) -> f64 {
    let step = if ratio.settled { SETTLED_C_STEP } else { MAX_C_STEP };
    next_c_stepped(prev_c, ratio.k, deficit_g, demand_rate_g_s, step, bounds)
}

fn next_c_stepped(
    prev_c: f64,
    k: f64,
    deficit_g: f64,
    demand_rate_g_s: f64,
    max_step: f64,
    bounds: Bounds,
) -> f64 {
    let base = if k > 1e-9 { 1.0 / k } else { bounds.max };
    let repay = if demand_rate_g_s > 1e-9 {
        deficit_g / (demand_rate_g_s * DEFICIT_HORIZON_S)
    } else {
        0.0
    };
    let target = bounds.clamp(base * (1.0 + repay));
    bounds.clamp(prev_c + (target - prev_c).clamp(-max_step, max_step))
}

/// Before the first ratio window is full, a deficit at least this many
/// balance steps, and this share of what was asked, is no start-up noise:
/// the pump is really off (tube moved, wrong calibration), so the early law
/// estimates the ratio from what it has instead of nudging around the seed.
pub const EARLY_SIGNIFICANT_RESOLUTIONS: f64 = 10.0;
pub const EARLY_SIGNIFICANT_FRACTION: f64 = 0.05;
/// ...and that early estimate needs this much history to mean anything.
pub const EARLY_RATIO_MIN_S: f64 = 20.0;

/// Whether a deficit (either sign) before the first ratio is a real offset.
pub fn deficit_is_significant(deficit_g: f64, required_g: f64, resolution_g: f64) -> bool {
    deficit_g.abs() >= (EARLY_SIGNIFICANT_RESOLUTIONS * resolution_g).max(EARLY_SIGNIFICANT_FRACTION * required_g)
}

/// A first, rough ratio from all the points so far: the Theil-Sen slope of
/// delivered vs commanded. A slope, not `delivered / commanded`, so the
/// constant start-up lag (pump spinning up, balance filter: ~2 s of flow)
/// does not read as the pump running short. `None` under
/// `EARLY_RATIO_MIN_S` or `EARLY_SIGNIFICANT_RESOLUTIONS` of command.
pub fn early_ratio(points: &[TrackPoint], resolution_g: f64) -> Option<f64> {
    let (first, last) = (points.first()?, points.last()?);
    if last.t_s - first.t_s < EARLY_RATIO_MIN_S
        || last.commanded_g - first.commanded_g < EARLY_SIGNIFICANT_RESOLUTIONS * resolution_g
    {
        return None;
    }
    let xy: Vec<(f64, f64)> = points.iter().map(|p| (p.commanded_g, p.delivered_g)).collect();
    let k = theil_sen_slope(&decimate(&xy, MAX_SLOPE_POINTS))?;
    (k.is_finite() && k > 0.0).then_some(k)
}

/// Next c before the delivery ratio can be measured: the run's starting c
/// (1.0, or the tubing calibration's c0) as the feed-forward, plus paying
/// back the cumulative deficit, less a dead band of balance rounding. Same
/// horizon, step and bounds as [`next_c`], so the run is corrected from its
/// first seconds instead of after a full ratio window. Anchored on `seed_c`,
/// not on `prev_c`, so it stays a proportional term on the cumulative
/// deficit (a PI on the flow) and cannot wind up.
pub fn next_c_early(
    prev_c: f64,
    seed_c: f64,
    deficit_g: f64,
    demand_rate_g_s: f64,
    resolution_g: f64,
    bounds: Bounds,
) -> f64 {
    let band = EARLY_DEADBAND_RESOLUTIONS * resolution_g;
    let deficit = deficit_g.signum() * (deficit_g.abs() - band).max(0.0);
    let repay = if demand_rate_g_s > 1e-9 {
        deficit / (demand_rate_g_s * DEFICIT_HORIZON_S)
    } else {
        0.0
    };
    let target = bounds.clamp(seed_c * (1.0 + repay));
    bounds.clamp(prev_c + (target - prev_c).clamp(-MAX_C_STEP, MAX_C_STEP))
}

/// Whether `k` needs a c outside `bounds`.
pub fn needs_alarm(k: f64, bounds: Bounds) -> bool {
    k < 1.0 / bounds.max || k > 1.0 / bounds.min
}

/// Where the balance stood when it last left `Normal`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub weight_g: f64,
    pub commanded_g: f64,
    /// The state machine went through a refill.
    pub refill: bool,
}

/// Mass delivered between the anchor and the first `Normal` read after it:
/// measured when the bottle only lost mass, estimated from `k` (1.0 if
/// unknown) when it gained some (a refill or a top-up).
pub fn settle_anchor(
    a: &Anchor,
    weight_now: f64,
    commanded_now: f64,
    k: Option<f64>,
    resolution_g: f64,
) -> f64 {
    if !a.refill && weight_now <= a.weight_g + resolution_g {
        (a.weight_g - weight_now).max(0.0)
    } else {
        k.unwrap_or(1.0) * (commanded_now - a.commanded_g).max(0.0)
    }
}

/// Coefficient of determination of `delivered` against `required` (both
/// cumulative, same length): `1 - sum((d - r)^2) / sum((r - mean r)^2)`.
pub fn r_squared(required: &[f64], delivered: &[f64]) -> Option<f64> {
    if required.len() != delivered.len() || required.len() < 2 {
        return None;
    }
    let mean = required.iter().sum::<f64>() / required.len() as f64;
    let ss_tot: f64 = required.iter().map(|r| (r - mean).powi(2)).sum();
    let ss_res: f64 = required.iter().zip(delivered).map(|(r, d)| (d - r).powi(2)).sum();
    (ss_tot > 0.0).then(|| 1.0 - ss_res / ss_tot)
}

/// µ (per hour) of the exponential feed that best explains a cumulative
/// delivered volume: least squares on `V(t) = F0/µ (e^(µt) - 1)`, F0 in
/// closed form for each µ, µ by golden-section search on `ln µ` over
/// `[guess/4, guess*4]`. `points` are `(t_hours, delivered_ml)`.
pub fn fit_exponential_mu(points: &[(f64, f64)], mu_guess: f64) -> Option<f64> {
    if points.len() < 3 || mu_guess.is_nan() || mu_guess <= 0.0 {
        return None;
    }
    let sse = |mu: f64| {
        let g: Vec<f64> = points.iter().map(|(t, _)| ((mu * t).exp() - 1.0) / mu).collect();
        let gg: f64 = g.iter().map(|x| x * x).sum();
        if gg <= 0.0 {
            return f64::INFINITY;
        }
        let f0 = points.iter().zip(&g).map(|((_, v), x)| v * x).sum::<f64>() / gg;
        points.iter().zip(&g).map(|((_, v), x)| (v - f0 * x).powi(2)).sum::<f64>()
    };
    let phi = (5f64.sqrt() - 1.0) / 2.0;
    let (mut a, mut b) = ((mu_guess / 4.0).ln(), (mu_guess * 4.0).ln());
    for _ in 0..100 {
        let c = b - phi * (b - a);
        let d = a + phi * (b - a);
        if sse(c.exp()) < sse(d.exp()) {
            b = d;
        } else {
            a = c;
        }
    }
    let mu = ((a + b) / 2.0).exp();
    mu.is_finite().then_some(mu)
}

/// The µ an exponential curve asks for, whatever its parameter mode.
pub fn requested_mu_per_hour(spec: &fermentool_curves::CurveSpec) -> Option<f64> {
    if spec.kind() != fermentool_curves::CurveKind::Exponential {
        return None;
    }
    let (s, e, h) = (spec.start, spec.effective_end(), spec.duration_hours());
    (s > 0.0 && e > 0.0 && h > 0.0).then(|| (e / s).ln() / h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(k: f64, n: usize, dt: f64, rate_g_s: f64) -> Vec<TrackPoint> {
        (0..n)
            .map(|i| {
                let t = i as f64 * dt;
                let c = rate_g_s * t;
                // quantised to a 0.1 g balance
                TrackPoint { t_s: t, commanded_g: c, delivered_g: (k * c * 10.0).round() / 10.0 }
            })
            .collect()
    }

    #[test]
    fn delivery_ratio_recovers_k_through_balance_quantisation() {
        let k = delivery_ratio(&line(0.85, 600, 1.0, 1.0), 0.1).unwrap().k;
        assert!((k - 0.85).abs() < 0.005, "got {k}");
    }

    #[test]
    fn no_ratio_until_the_window_holds_enough_mass() {
        // 0.01 g/s for 400 s = 4 g, below 50 x 0.1 g = 5 g.
        assert_eq!(delivery_ratio(&line(1.0, 400, 1.0, 0.01), 0.1), None);
    }

    #[test]
    fn a_five_gram_window_is_still_accurate_on_a_0_1_g_balance() {
        // 2 ml/min at 90 %: 5 g of command after 150 s, read in 0.1 g steps.
        let k = delivery_ratio(&line(0.9, 160, 1.0, 2.0 / 60.0), 0.1).unwrap().k;
        assert!((k - 0.9).abs() < 0.03, "got {k}");
    }

    #[test]
    fn thinning_keeps_the_whole_span_and_the_recent_points() {
        let pts = line(1.0, 1000, 1.0, 1.0);
        let thin_pts = thin(&pts, 800);
        assert!(thin_pts.len() <= 800);
        assert_eq!(thin_pts.first(), pts.first(), "the oldest point survives");
        assert_eq!(&thin_pts[thin_pts.len() - 500..], &pts[500..], "the recent half is intact");
        assert!(thin_pts.windows(2).all(|w| w[0].t_s < w[1].t_s), "still in time order");
    }

    #[test]
    fn a_long_low_flow_history_still_yields_a_ratio_after_thinning() {
        // 0.001 g/s for 8 h: 28.8 g over the run, only 0.8 g per 800 reads.
        let mut pts = Vec::new();
        for p in line(0.95, 8 * 3600, 1.0, 0.001) {
            pts.push(p);
            if pts.len() > 800 {
                pts = thin(&pts, 800);
            }
        }
        let k = delivery_ratio(&pts, 0.1).expect("the span must survive thinning").k;
        assert!((k - 0.95).abs() < 0.02, "got {k}");
    }

    #[test]
    fn next_c_is_the_inverse_ratio_when_on_track() {
        let mut c = 1.0;
        for _ in 0..20 {
            c = next_c(c, 0.9, 0.0, 1.0, Bounds::default());
        }
        assert!((c - 1.0 / 0.9).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn next_c_moves_at_most_one_step() {
        assert!((next_c(1.0, 0.8, 0.0, 1.0, Bounds::default()) - 1.02).abs() < 1e-12);
    }

    #[test]
    fn a_deficit_raises_c_above_the_inverse_ratio() {
        // 60 g behind at 1 g/s: pay back 60 g over 600 s, +10 %.
        let mut c = 1.0;
        for _ in 0..50 {
            c = next_c(c, 1.0, 60.0, 1.0, Bounds::default());
        }
        assert!((c - 1.1).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn zero_demand_rate_holds_the_feed_forward() {
        let c = next_c(1.0, 1.0, 5.0, 0.0, Bounds::default());
        assert!(c.is_finite());
        assert_eq!(c, 1.0);
    }

    #[test]
    fn early_c_ignores_rounding_and_pays_back_a_real_deficit() {
        // 2 ml/min = 1/30 g/s: 600 s of it is 20 g.
        let rate = 1.0 / 30.0;
        // Within the 0.3 g dead band: stays on the calibration's c0.
        assert_eq!(next_c_early(1.007, 1.007, 0.25, rate, 0.1, Bounds::default()), 1.007);
        // 0.93 g behind (run 17 at 5.5 min): 0.63 g past the band, +3.15 %.
        let mut c = 1.007;
        for _ in 0..10 {
            c = next_c_early(c, 1.007, 0.93, rate, 0.1, Bounds::default());
        }
        assert!((c - 1.007 * 1.0315).abs() < 1e-9, "got {c}");
        // Ahead of the curve: below c0, symmetric.
        assert!(next_c_early(1.0, 1.0, -2.0, rate, 0.1, Bounds::default()) < 1.0);
        // Anchored on the seed, not on the last c: a constant deficit gives a
        // constant c, it does not keep climbing.
        let mut settled = 1.0;
        for _ in 0..10 {
            settled = next_c_early(settled, 1.0, 0.93, rate, 0.1, Bounds::default());
        }
        let again = next_c_early(settled, 1.0, 0.93, rate, 0.1, Bounds::default());
        assert!((again - settled).abs() < 1e-12, "{settled} -> {again}");
    }

    #[test]
    fn next_c_stays_within_bounds() {
        let mut c = 1.0;
        for _ in 0..100 {
            c = next_c(c, 0.1, 1e6, 1.0, Bounds::default());
        }
        assert_eq!(c, TRIM_MAX);
    }

    #[test]
    fn alarm_only_outside_what_c_can_correct() {
        assert!(!needs_alarm(0.85, Bounds::default()));
        assert!(needs_alarm(0.75, Bounds::default())); // would need c = 1.33
        assert!(needs_alarm(0.0, Bounds::default())); // nothing leaves the bottle
        assert!(needs_alarm(1.30, Bounds::default())); // would need c = 0.77
    }

    #[test]
    fn the_limit_widens_the_bounds_symmetrically() {
        let b = Bounds::from_limit_pct(25.0);
        assert!((b.max - 1.25).abs() < 1e-12 && (b.min - 0.8).abs() < 1e-12);
        assert_eq!(Bounds::default(), b);
        // Run 39: the moved tube delivered 79 %, c needed 1.27.
        let wide = Bounds::from_limit_pct(40.0);
        assert!(needs_alarm(0.79, Bounds::default()));
        assert!(!needs_alarm(0.79, wide));
        let mut c = 1.0;
        for _ in 0..50 {
            c = next_c(c, 0.79, 0.0, 1.0, wide);
        }
        assert!((c - 1.0 / 0.79).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn a_real_early_offset_is_told_from_start_up_noise() {
        // 9 ml/min = 0.15 g/s on a 0.1 g balance.
        let rate = 0.15;
        // The usual start-up lag, ~0.5 g at 10 s: not significant.
        assert!(!deficit_is_significant(0.5, 10.0 * rate, 0.1));
        // Run 39 at 30 s: 1.16 g behind of 4.5 g asked, 26 %: significant.
        assert!(deficit_is_significant(1.16, 30.0 * rate, 0.1));
        // Over a long run, 1 g of 200 g (0.5 %) is not.
        assert!(!deficit_is_significant(1.0, 200.0, 0.1));
    }

    #[test]
    fn the_early_ratio_ignores_the_start_up_lag() {
        // Pump at 78 %, but nothing arrives for the first 2 s (spin-up and
        // balance filter): delivered/commanded would read ~0.70 at 20 s, the
        // slope still reads the pump.
        let pts: Vec<TrackPoint> = (0..=25)
            .map(|i| {
                let t = i as f64;
                let commanded = 0.15 * t;
                let delivered = (0.78 * 0.15 * (t - 2.0).max(0.0) * 10.0).round() / 10.0;
                TrackPoint { t_s: t, commanded_g: commanded, delivered_g: delivered }
            })
            .collect();
        let k = early_ratio(&pts, 0.1).expect("25 s and 3.75 g of history");
        assert!((k - 0.78).abs() < 0.05, "got {k}");
        assert_eq!(early_ratio(&pts[..10], 0.1), None, "under 20 s");
    }

    #[test]
    fn a_touch_without_refill_is_measured() {
        let a = Anchor { weight_g: 1000.0, commanded_g: 50.0, refill: false };
        assert_eq!(settle_anchor(&a, 990.0, 60.0, Some(0.9), 0.1), 10.0);
    }

    #[test]
    fn a_refill_is_estimated_from_the_ratio() {
        let a = Anchor { weight_g: 100.0, commanded_g: 50.0, refill: true };
        assert!((settle_anchor(&a, 1100.0, 60.0, Some(0.9), 0.1) - 9.0).abs() < 1e-9);
    }

    #[test]
    fn r_squared_is_one_for_a_perfect_track_and_lower_otherwise() {
        let req: Vec<f64> = (0..100).map(|i| i as f64).collect();
        assert!((r_squared(&req, &req).unwrap() - 1.0).abs() < 1e-12);
        let off: Vec<f64> = req.iter().map(|v| v * 0.9).collect();
        assert!(r_squared(&req, &off).unwrap() < 0.99);
    }

    #[test]
    fn fits_the_mu_of_an_exponential_cumulative_volume() {
        // F(t) = 2 e^(0.15 t) mL/h, so V(t) = 2/0.15 (e^(0.15 t) - 1).
        let pts: Vec<(f64, f64)> = (0..=200)
            .map(|i| {
                let t = i as f64 * 0.1;
                (t, 2.0 / 0.15 * ((0.15 * t).exp() - 1.0))
            })
            .collect();
        let mu = fit_exponential_mu(&pts, 0.1).unwrap();
        assert!((mu - 0.15).abs() < 1e-4, "got {mu}");
    }

    #[test]
    fn requested_mu_for_both_exponential_modes_and_none_otherwise() {
        use fermentool_curves::CurveSpec;
        use std::time::Duration;
        let d = Duration::from_secs(10 * 3600);
        let physio = CurveSpec::exponential_physio(1.0, 0.15, d);
        assert!((requested_mu_per_hour(&physio).unwrap() - 0.15).abs() < 1e-9);
        let ends = CurveSpec::exponential_endpoints(1.0, (1.5f64).exp(), d);
        assert!((requested_mu_per_hour(&ends).unwrap() - 0.15).abs() < 1e-9);
        assert_eq!(requested_mu_per_hour(&CurveSpec::linear(1.0, 2.0, d)), None);
    }

    #[test]
    fn a_net_gain_is_estimated_not_counted_negative() {
        // 30 g top-up seen as a perturbation: the bottle gained mass.
        let a = Anchor { weight_g: 1000.0, commanded_g: 50.0, refill: false };
        let d = settle_anchor(&a, 1025.0, 60.0, None, 0.1);
        assert!((d - 10.0).abs() < 1e-9, "got {d}");
    }
}
