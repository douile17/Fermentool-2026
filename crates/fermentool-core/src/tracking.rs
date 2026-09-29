//! Cumulative feed tracking: pure math, no I/O, no clocks. The engine feeds
//! it cumulative masses since run start, it returns the pump correction.

use serde::{Deserialize, Serialize};

use crate::trim::{decimate, theil_sen_slope, MAX_SLOPE_POINTS, TRIM_MAX, TRIM_MIN};

/// A slope needs at least this many balance steps of commanded mass, so the
/// balance's rounding is a small part of what it measures.
pub const MIN_WINDOW_RESOLUTIONS: f64 = 200.0;
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

/// The pump's real delivered / commanded ratio: Theil-Sen slope of delivered
/// vs commanded over the shortest recent window holding enough mass and
/// time. `None` until such a window exists.
pub fn delivery_ratio(points: &[TrackPoint], resolution_g: f64) -> Option<f64> {
    let last = points.last()?;
    let min_mass = MIN_WINDOW_RESOLUTIONS * resolution_g;
    let start = points.iter().rposition(|p| {
        last.commanded_g - p.commanded_g >= min_mass && last.t_s - p.t_s >= MIN_WINDOW_S
    })?;
    let xy: Vec<(f64, f64)> = points[start..]
        .iter()
        .map(|p| (p.commanded_g, p.delivered_g))
        .collect();
    let k = theil_sen_slope(&decimate(&xy, MAX_SLOPE_POINTS))?;
    k.is_finite().then_some(k.max(0.0))
}

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
pub fn next_c(prev_c: f64, k: f64, deficit_g: f64, demand_rate_g_s: f64) -> f64 {
    let base = if k > 1e-9 { 1.0 / k } else { TRIM_MAX };
    let repay = if demand_rate_g_s > 1e-9 {
        deficit_g / (demand_rate_g_s * DEFICIT_HORIZON_S)
    } else {
        0.0
    };
    let target = (base * (1.0 + repay)).clamp(TRIM_MIN, TRIM_MAX);
    (prev_c + (target - prev_c).clamp(-MAX_C_STEP, MAX_C_STEP)).clamp(TRIM_MIN, TRIM_MAX)
}

/// Whether `k` needs a c outside `[TRIM_MIN, TRIM_MAX]`.
pub fn needs_alarm(k: f64) -> bool {
    k < 1.0 / TRIM_MAX || k > 1.0 / TRIM_MIN
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
        let k = delivery_ratio(&line(0.85, 600, 1.0, 1.0), 0.1).unwrap();
        assert!((k - 0.85).abs() < 0.005, "got {k}");
    }

    #[test]
    fn no_ratio_until_the_window_holds_enough_mass() {
        // 0.01 g/s for 600 s = 6 g, below 200 x 0.1 g = 20 g.
        assert_eq!(delivery_ratio(&line(1.0, 600, 1.0, 0.01), 0.1), None);
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
        let k = delivery_ratio(&pts, 0.1).expect("the span must survive thinning");
        assert!((k - 0.95).abs() < 0.02, "got {k}");
    }

    #[test]
    fn next_c_is_the_inverse_ratio_when_on_track() {
        let mut c = 1.0;
        for _ in 0..20 {
            c = next_c(c, 0.9, 0.0, 1.0);
        }
        assert!((c - 1.0 / 0.9).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn next_c_moves_at_most_one_step() {
        assert!((next_c(1.0, 0.8, 0.0, 1.0) - 1.02).abs() < 1e-12);
    }

    #[test]
    fn a_deficit_raises_c_above_the_inverse_ratio() {
        // 60 g behind at 1 g/s: pay back 60 g over 600 s, +10 %.
        let mut c = 1.0;
        for _ in 0..50 {
            c = next_c(c, 1.0, 60.0, 1.0);
        }
        assert!((c - 1.1).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn zero_demand_rate_holds_the_feed_forward() {
        let c = next_c(1.0, 1.0, 5.0, 0.0);
        assert!(c.is_finite());
        assert_eq!(c, 1.0);
    }

    #[test]
    fn next_c_stays_within_bounds() {
        let mut c = 1.0;
        for _ in 0..100 {
            c = next_c(c, 0.1, 1e6, 1.0);
        }
        assert_eq!(c, TRIM_MAX);
    }

    #[test]
    fn alarm_only_outside_what_c_can_correct() {
        assert!(!needs_alarm(0.85));
        assert!(needs_alarm(0.75)); // would need c = 1.33
        assert!(needs_alarm(0.0)); // nothing leaves the bottle
        assert!(needs_alarm(1.30)); // would need c = 0.77
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
