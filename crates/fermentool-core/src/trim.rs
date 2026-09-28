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
