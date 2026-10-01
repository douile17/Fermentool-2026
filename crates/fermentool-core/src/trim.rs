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

/// A jump between two reads is a perturbation (bottle touched, tube pulled,
/// something set on the pan) when it exceeds `PERTURBATION_FLOW_FACTOR` times
/// what the pump should move in that read, bounded to
/// `[PERTURBATION_FLOOR_G, PERTURBATION_MIN_G]`. A fixed 5 g let a 3.4 g
/// bump at 2 ml/min (0.03 g/s) pass as flow and the trim chase it to +25 %;
/// the floor stays above the 0.1 g balance's own ±0.5 g read-to-read jitter.
pub const PERTURBATION_MIN_G: f64 = 5.0;
pub const PERTURBATION_FLOOR_G: f64 = 1.0;
pub const PERTURBATION_FLOW_FACTOR: f64 = 5.0;
pub const REFILL_THRESHOLD_G: f64 = 50.0;
pub const REFILL_SETTLE_VARIANCE_G: f64 = 0.5;
pub const REFILL_SETTLE_SECONDS: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// Mass the pump was commanded to move since the previous read.
    pub expected_step_g: f64,
}

/// Largest jump between two reads still taken as pumping.
pub fn perturbation_limit_g(expected_step_g: f64) -> f64 {
    (PERTURBATION_FLOW_FACTOR * expected_step_g.max(0.0))
        .clamp(PERTURBATION_FLOOR_G, PERTURBATION_MIN_G)
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
            } else if jump > perturbation_limit_g(input.expected_step_g) {
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

    #[test]
    fn scale_state_serializes_as_snake_case_like_the_rest_of_the_api() {
        assert_eq!(serde_json::to_string(&ScaleState::Normal).unwrap(), "\"normal\"");
        assert_eq!(
            serde_json::to_string(&ScaleState::RefillPending).unwrap(),
            "\"refill_pending\""
        );
    }

    fn input(weight: f64, prev: f64) -> StateInput {
        StateInput {
            weight_g: weight,
            prev_weight_g: prev,
            recent_variance_g: 0.0,
            seconds_in_settling: 0.0,
            manual_refill_mode: false,
            manual_refill_done: false,
            expected_step_g: 0.0,
        }
    }

    #[test]
    fn a_small_bump_at_low_flow_is_a_perturbation_not_flow() {
        // Run 16: 2 ml/min is ~0.034 g per 1 s read; the pan gained 3.4 g.
        let mut i = input(432.9, 429.5);
        i.expected_step_g = 2.0 / 60.0;
        assert_eq!(next_state(ScaleState::Normal, &i), ScaleState::Perturbation);
        // ...while the balance's own ±0.5 g read-to-read jitter stays flow.
        let mut i = input(423.5, 423.0);
        i.expected_step_g = 2.0 / 60.0;
        assert_eq!(next_state(ScaleState::Normal, &i), ScaleState::Normal);
    }

    #[test]
    fn the_limit_follows_the_flow_between_its_bounds() {
        assert_eq!(perturbation_limit_g(0.0), PERTURBATION_FLOOR_G);
        assert_eq!(perturbation_limit_g(0.5), 2.5);
        // 60 ml/min and up keep the historical 5 g.
        assert_eq!(perturbation_limit_g(1.0), PERTURBATION_MIN_G);
        assert_eq!(perturbation_limit_g(10.0), PERTURBATION_MIN_G);
        // A 3.4 g move at 60 ml/min is ordinary pumping.
        let mut i = input(96.6, 100.0);
        i.expected_step_g = 1.0;
        assert_eq!(next_state(ScaleState::Normal, &i), ScaleState::Normal);
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

/// Bounds of the pump correction factor c.
pub const TRIM_MIN: f64 = 0.80;
pub const TRIM_MAX: f64 = 1.25;

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

/// A tubing calibration's replicate spread above this CV% is flagged to the
/// operator. A warning only, never a block: a human decides whether to redo it.
pub const CALIBRATION_CV_WARN_PCT: f64 = 5.0;

/// Three hand-weighed bursts at one commanded `setpoint` (the run's own unit,
/// rpm or mL/min).
#[derive(Debug, Clone, Copy)]
pub struct CalibrationInput {
    pub setpoint: f64,
    pub density_g_per_ml: f64,
    /// `(duration_min, weight_g)` per replicate, the duration from the burst
    /// run's own clock.
    pub samples: [(f64, f64); 3],
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct CalibrationResult {
    pub measured_ml_min: [f64; 3],
    pub mean_measured_ml_min: f64,
    /// Population CV of the three measured flows, percent.
    pub cv_pct: f64,
    /// `setpoint / mean_measured`. In ml/min mode it is dimensionless, with
    /// the trim's convention (above 1 = under-delivering = speed up), and
    /// seeds `trim_c` directly. In rpm mode it is rpm per (mL/min), not a
    /// trim factor: the engine uses its inverse as the rpm-to-volume
    /// conversion and starts `trim_c` at 1.0.
    pub c0: f64,
}

/// Pure: weights to flows, then mean, CV and `c0`. Callers validate that
/// durations, weights and density are positive and finite.
pub fn compute_calibration(input: &CalibrationInput) -> CalibrationResult {
    let measured_ml_min = input
        .samples
        .map(|(dur_min, weight_g)| weight_g / (input.density_g_per_ml * dur_min));
    let mean = measured_ml_min.iter().sum::<f64>() / 3.0;
    let variance = measured_ml_min.iter().map(|m| (m - mean).powi(2)).sum::<f64>() / 3.0;
    let cv_pct = if mean.abs() > 1e-9 { 100.0 * variance.sqrt() / mean } else { 0.0 };
    let c0 = if mean.abs() > 1e-9 { input.setpoint / mean } else { 1.0 };
    CalibrationResult {
        measured_ml_min,
        mean_measured_ml_min: mean,
        cv_pct,
        c0,
    }
}

#[cfg(test)]
mod calibration_tests {
    use super::*;

    #[test]
    fn identical_replicates_give_zero_cv() {
        let input = CalibrationInput {
            setpoint: 10.0,
            density_g_per_ml: 1.0,
            samples: [(5.0, 50.0), (5.0, 50.0), (5.0, 50.0)],
        };
        let r = compute_calibration(&input);
        assert!((r.mean_measured_ml_min - 10.0).abs() < 1e-9);
        assert!(r.cv_pct.abs() < 1e-9);
        assert!((r.c0 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn underdelivery_gives_c0_above_one() {
        // commanded 10 mL/min, measured 9 mL/min: the pump needs to speed up.
        let input = CalibrationInput {
            setpoint: 10.0,
            density_g_per_ml: 1.0,
            samples: [(5.0, 45.0), (5.0, 45.0), (5.0, 45.0)],
        };
        let r = compute_calibration(&input);
        assert!((r.mean_measured_ml_min - 9.0).abs() < 1e-9);
        assert!(r.c0 > 1.0, "got {}", r.c0);
        assert!((r.c0 - 10.0 / 9.0).abs() < 1e-6);
    }

    #[test]
    fn a_divergent_replicate_raises_cv_pct() {
        let input = CalibrationInput {
            setpoint: 10.0,
            density_g_per_ml: 1.0,
            samples: [(5.0, 50.0), (5.0, 50.0), (5.0, 60.0)],
        };
        let r = compute_calibration(&input);
        assert!(r.cv_pct > CALIBRATION_CV_WARN_PCT, "got {}", r.cv_pct);
    }

    #[test]
    fn density_and_duration_convert_grams_to_ml_per_min() {
        // 118 g of a 1.18 g/mL feed in 10 min is 100 mL / 10 min = 10 mL/min.
        let input = CalibrationInput {
            setpoint: 10.0,
            density_g_per_ml: 1.18,
            samples: [(10.0, 118.0), (10.0, 118.0), (10.0, 118.0)],
        };
        let r = compute_calibration(&input);
        assert!((r.measured_ml_min[0] - 10.0).abs() < 1e-9);
        assert!((r.c0 - 1.0).abs() < 1e-9);
    }
}
