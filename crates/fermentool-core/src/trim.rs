//! Pure math for the gravimetric feed trim: no I/O, no clocks (matches the
//! fermentool-curves convention -- feed it data, get a number back).

/// Evenly-spaced subsample of `points` down to at most `max_len`, always
/// keeping the first and last point (the endpoints matter most for a slope).
pub fn decimate(points: &[(f64, f64)], max_len: usize) -> Vec<(f64, f64)> {
    if points.len() <= max_len || max_len < 2 {
        return points.to_vec();
    }
    let stride = (points.len() - 1) as f64 / (max_len - 1) as f64;
    (0..max_len)
        .map(|i| points[((i as f64 * stride).round() as usize).min(points.len() - 1)])
        .collect()
}

/// Median of all pairwise slopes, robust to outliers without a hand-tuned
/// threshold. O(n^2); callers decimate to `MAX_SLOPE_POINTS` first.
pub const MAX_SLOPE_POINTS: usize = 200;

pub fn theil_sen_slope(points: &[(f64, f64)]) -> Option<f64> {
    if points.len() < 2 {
        return None;
    }
    let mut slopes = Vec::with_capacity(points.len() * (points.len() - 1) / 2);
    for i in 0..points.len() {
        for j in (i + 1)..points.len() {
            let (x1, y1) = points[i];
            let (x2, y2) = points[j];
            let dx = x2 - x1;
            if dx.abs() > 1e-12 {
                slopes.push((y2 - y1) / dx);
            }
        }
    }
    if slopes.is_empty() {
        return None;
    }
    slopes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = slopes.len() / 2;
    Some(if slopes.len() % 2 == 0 {
        (slopes[mid - 1] + slopes[mid]) / 2.0
    } else {
        slopes[mid]
    })
}

pub const PERTURBATION_MIN_G: f64 = 5.0;
pub const REFILL_THRESHOLD_G: f64 = 50.0;
pub const REFILL_SETTLE_VARIANCE_G: f64 = 0.5;
pub const REFILL_SETTLE_SECONDS: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ScaleState {
    Normal,
    Perturbation,
    RefillPending,
    RefillSettling,
}

#[derive(Debug, Clone, Copy)]
pub struct StateInput {
    pub weight_g: f64,
    pub prev_weight_g: f64,
    pub recent_variance_g: f64,
    pub seconds_in_settling: f64,
    pub manual_refill_mode: bool,
    pub manual_refill_done: bool,
}

/// Learning (the trim update, Task 6) is only allowed while this returns
/// `Normal`: every condition in the synthese_chemostat doc's "conditions
/// d'autorisation de l'apprentissage" checklist (pump running, scale stable,
/// no tank handling, no refill in progress, no active alarm) is implied by
/// "currently Normal" here, so callers check one thing, not a checklist.
pub fn next_state(current: ScaleState, input: &StateInput) -> ScaleState {
    let jump = (input.weight_g - input.prev_weight_g).abs();
    match current {
        ScaleState::Normal | ScaleState::Perturbation => {
            if input.manual_refill_mode || jump > REFILL_THRESHOLD_G {
                ScaleState::RefillPending
            } else if jump > PERTURBATION_MIN_G {
                ScaleState::Perturbation
            } else {
                ScaleState::Normal
            }
        }
        ScaleState::RefillPending => {
            if input.manual_refill_done || input.recent_variance_g < REFILL_SETTLE_VARIANCE_G {
                ScaleState::RefillSettling
            } else {
                ScaleState::RefillPending
            }
        }
        ScaleState::RefillSettling => {
            if input.seconds_in_settling >= REFILL_SETTLE_SECONDS {
                ScaleState::Normal
            } else {
                ScaleState::RefillSettling
            }
        }
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    fn input(weight: f64, prev: f64) -> StateInput {
        StateInput {
            weight_g: weight,
            prev_weight_g: prev,
            recent_variance_g: 0.0,
            seconds_in_settling: 0.0,
            manual_refill_mode: false,
            manual_refill_done: false,
        }
    }

    #[test]
    fn small_jump_stays_normal() {
        assert_eq!(next_state(ScaleState::Normal, &input(100.2, 100.0)), ScaleState::Normal);
    }

    #[test]
    fn moderate_jump_is_a_perturbation() {
        assert_eq!(next_state(ScaleState::Normal, &input(120.0, 100.0)), ScaleState::Perturbation);
    }

    #[test]
    fn perturbation_returns_to_normal_once_the_jump_settles() {
        assert_eq!(next_state(ScaleState::Perturbation, &input(120.0, 120.0)), ScaleState::Normal);
    }

    #[test]
    fn big_jump_is_refill_pending() {
        assert_eq!(next_state(ScaleState::Normal, &input(700.0, 100.0)), ScaleState::RefillPending);
    }

    #[test]
    fn manual_refill_mode_forces_refill_pending_even_without_a_jump() {
        let mut i = input(100.0, 100.0);
        i.manual_refill_mode = true;
        assert_eq!(next_state(ScaleState::Normal, &i), ScaleState::RefillPending);
    }

    #[test]
    fn refill_pending_moves_to_settling_once_variance_is_low() {
        let mut i = input(700.0, 700.0);
        i.recent_variance_g = 0.1;
        assert_eq!(next_state(ScaleState::RefillPending, &i), ScaleState::RefillSettling);
    }

    #[test]
    fn refill_pending_stays_pending_while_the_operator_is_still_handling_the_bottle() {
        let mut i = input(700.0, 690.0);
        i.recent_variance_g = 5.0;
        assert_eq!(next_state(ScaleState::RefillPending, &i), ScaleState::RefillPending);
    }

    #[test]
    fn settling_returns_to_normal_after_the_extra_delay() {
        let mut i = input(700.0, 700.0);
        i.seconds_in_settling = REFILL_SETTLE_SECONDS + 1.0;
        assert_eq!(next_state(ScaleState::RefillSettling, &i), ScaleState::Normal);
    }

    #[test]
    fn settling_stays_settling_before_the_delay_elapses() {
        let mut i = input(700.0, 700.0);
        i.seconds_in_settling = REFILL_SETTLE_SECONDS - 1.0;
        assert_eq!(next_state(ScaleState::RefillSettling, &i), ScaleState::RefillSettling);
    }

    #[test]
    fn manual_refill_done_forces_settling_from_pending() {
        let mut i = input(700.0, 700.0);
        i.recent_variance_g = 5.0;
        i.manual_refill_done = true;
        assert_eq!(next_state(ScaleState::RefillPending, &i), ScaleState::RefillSettling);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theil_sen_recovers_a_clean_linear_slope() {
        let pts: Vec<(f64, f64)> = (0..20).map(|i| (i as f64, 2.0 * i as f64 + 10.0)).collect();
        let slope = theil_sen_slope(&pts).unwrap();
        assert!((slope - 2.0).abs() < 1e-9, "got {slope}");
    }

    #[test]
    fn theil_sen_ignores_a_single_outlier() {
        let mut pts: Vec<(f64, f64)> = (0..20).map(|i| (i as f64, 2.0 * i as f64 + 10.0)).collect();
        pts[10].1 += 1000.0;
        let slope = theil_sen_slope(&pts).unwrap();
        assert!((slope - 2.0).abs() < 0.5, "got {slope}");
    }

    #[test]
    fn theil_sen_needs_at_least_two_points() {
        assert!(theil_sen_slope(&[]).is_none());
        assert!(theil_sen_slope(&[(0.0, 1.0)]).is_none());
    }

    #[test]
    fn decimate_keeps_endpoints_and_caps_length() {
        let pts: Vec<(f64, f64)> = (0..1000).map(|i| (i as f64, i as f64)).collect();
        let out = decimate(&pts, 200);
        assert!(out.len() <= 200);
        assert_eq!(out.first(), pts.first());
        assert_eq!(out.last(), pts.last());
    }

    #[test]
    fn decimate_is_a_noop_under_the_cap() {
        let pts: Vec<(f64, f64)> = (0..10).map(|i| (i as f64, i as f64)).collect();
        assert_eq!(decimate(&pts, 200), pts);
    }
}
