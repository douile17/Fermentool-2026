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
    fn a_net_gain_is_estimated_not_counted_negative() {
        // 30 g top-up seen as a perturbation: the bottle gained mass.
        let a = Anchor { weight_g: 1000.0, commanded_g: 50.0, refill: false };
        let d = settle_anchor(&a, 1025.0, 60.0, None, 0.1);
        assert!((d - 10.0).abs() < 1e-9, "got {d}");
    }
}
