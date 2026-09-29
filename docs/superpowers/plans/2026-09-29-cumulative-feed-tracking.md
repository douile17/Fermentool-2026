# Cumulative Feed Tracking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the mass that really leaves the feed bottle follow the mass the curve asks for, over the whole run, for any curve shape (linear, exponential, sigmoid, constant, step), to R² ≈ 1, and show the proof (chart, R², requested vs obtained µ).

**Architecture:** Replace the fixed-step trim (`trim::update_trim`, ±30 % of the error per second) with a cumulative tracker. The engine integrates three cumulative masses since run start: *required* (the raw curve), *commanded* (curve × c, what the pump was told) and *delivered* (weighed). The pump's real delivery ratio `k` is the robust slope of delivered vs commanded over a window large enough for the balance's resolution; c is set to `1/k` (feed-forward) times a term that pays back the cumulative deficit over 10 minutes. Delivered mass is carried across perturbations and refills. The journal records the weight each tick; a pure report function computes the chart, R² and fitted µ.

**Tech Stack:** Rust (fermentool-core, rusqlite, axum), Svelte 5 UI (ui/), existing `trim::theil_sen_slope` / `trim::decimate`, `engine::integrate_curve_mass`.

**Spec:** No separate spec document. The design was agreed in conversation on 2026-09-29 and is written out in "Design" below; that section is the authority.

## Design

Symbols, all in grams unless stated, `t` = run elapsed seconds, `ρ` = `scale.density_g_per_ml`, `u` = mL/min per curve unit (`1.0` in ml/min, the calibration's ml/min per rpm in rpm):

- demand rate `r(t) = curve(t) · u · ρ / 60` (g/s)
- required `R(t) = ∫₀ᵗ r` : what the curve asks for
- commanded `C(t) = ∫₀ᵗ r · c` : what the pump was told (c piecewise constant between scale reads)
- delivered `D(t)` : what left the bottle, measured
- deficit `E = R − D` (positive = under-delivered)
- pump delivery ratio `k = dD/dC`, estimated as the Theil-Sen slope of `(C, D)` over the shortest recent window with `ΔC ≥ 200 × resolution` and `Δt ≥ 120 s`

Control law, evaluated every 10 s while the state machine is `Normal`:

```
c_target = (1 / k) · (1 + E / (r · 600 s))
c_new    = clamp(prev_c + clamp(c_target − prev_c, ±0.02), 0.80, 1.25)
```

With `k` exact, `dE/dt = −E / 600 s`: the cumulative deficit decays with a 10 min time constant, so delivered mass tracks required mass, not just the instantaneous rate. Before the first `k` estimate, c stays at its start value (1.0, or the calibration's c0).

Delivered accounting:
- consecutive `Normal` reads: `D += w_prev − w_now`
- leaving `Normal`: remember an anchor `(last Normal weight, C at that read, refill = false)`; entering `RefillPending`/`RefillSettling` sets `refill = true`
- back to `Normal`: if not a refill and `w_now ≤ w_anchor + resolution`, `D += w_anchor − w_now` (measured, the bottle was only touched); otherwise `D += k · (C_now − C_anchor)` (estimated, the bottle gained mass), and the slope window restarts
- first read of a run: `D += C` so far (nominal, under 1 s of flow)
- crash resume: the persisted last weight becomes an anchor, so the downtime is measured by the weight difference

Alarm (sticky until the next `start_run`, freezes c, accounting continues): `1/k` outside `[0.80, 1.25]` for 30 consecutive updates (5 min): the pump cannot be corrected within bounds.

Balance resolution: taken from the reply's decimals (`740.5 g` → 0.1 g, `740 g` → 1 g).

Proof shown to the operator (Overview while running, History afterwards): delivered vs required volume (mL) chart, R² of delivered against required, final deficit (mL and %), and for exponential curves the requested µ and the µ fitted on the delivered volume.

## Global Constraints

- Manual mode (`gravimetric_trim = false`) stays byte-for-byte unchanged: c is not applied, the balance is not read during the run.
- c stays within `[TRIM_MIN, TRIM_MAX] = [0.80, 1.25]`; at most `0.02` change per update; updates at most every `10 s`.
- A lost or wedged balance never stops the pump: c freezes, the run continues.
- Tuning constants are Rust constants, not config, with the rationale in a doc comment.
- The balance is read once per second (unchanged).
- No em dash (U+2014) and no spaced hyphen (` - `) anywhere: comma, colon or period (CLAUDE.md).
- Commit messages: no `Co-Authored-By: Claude` trailer, no `Claude-Session` line (CLAUDE.md).
- After any `ui/src/` change, run `npm run build` from `ui/` (CLAUDE.md).
- Rust commands need `export PATH="$HOME/.cargo/bin:$PATH"` in Git Bash.

## Review Focus

1. Very low flow (0.05 ml/min, 0.1 g balance): no `k` for a long time, and no false alarm meanwhile. Pinned in Task 4 (`a_very_low_flow_never_false_alarms`).
2. A curve whose rate is 0 (linear from 0): no division by zero, c holds. Pinned in Task 2 (`zero_demand_rate_holds_the_feed_forward`).
3. A small top-up under 50 g (seen as a perturbation, net weight gain): must not count as negative delivery. Pinned in Task 2 (`a_net_gain_is_estimated_not_counted_negative`).
4. Crash resume mid-run: cumulative required/commanded/delivered survive and the downtime is measured. Pinned in Task 5 (`tracking_survives_a_crash_resume`).
5. A balance set to whole grams: resolution 1 g, window grows to 200 g. Pinned in Task 1 (`reads_the_resolution_from_the_decimals`).

---

## File Structure

- Create `crates/fermentool-core/src/tracking.rs`: pure math: `TrackPoint`, `delivery_ratio`, `next_c`, `needs_alarm`, `Anchor`, `settle_anchor`, `r_squared`, `fit_exponential_mu`, `requested_mu_per_hour`, constants.
- Modify `crates/fermentool-core/src/lib.rs`: `pub mod tracking;`
- Modify `crates/fermentool-core/src/scale.rs`: `SicsReading` with resolution.
- Modify `crates/fermentool-core/src/trim.rs`: remove `update_trim`, `TrimOutcome`, `TRIM_IGNORE_BAND`, `TRIM_ALARM_BAND`, `TRIM_GAMMA`, `TRIM_MAX_DELTA` and their tests.
- Modify `crates/fermentool-core/src/engine/mod.rs`: `Tracker` state, new `scale_tick` accounting and control, persistence, tick journal fields, `tracking_report`, status.
- Create `crates/fermentool-core/src/store/migrations/0004_tick_weight.sql`; modify `store/mod.rs` (`NewTick`/`TickRow` gain `weight_g`, `delivered_g`).
- Modify `crates/fermentool-core/src/control.rs`, `api.rs`: status field, `GET /api/runs/{id}/tracking`.
- Create `ui/src/components/TrackingPanel.svelte`; modify `ui/src/routes/Overview.svelte`, `ui/src/routes/History.svelte`.

---

### Task 1: Balance resolution from the reply

**Files:**
- Modify: `crates/fermentool-core/src/scale.rs`
- Modify: `crates/fermentool-core/src/engine/mod.rs` (`read_scale` call site)

**Interfaces:**
- Produces: `pub struct SicsReading { pub weight_g: f64, pub stable: bool, pub resolution_g: f64 }` and `pub fn parse_sics_weight(reply: &[u8]) -> Result<SicsReading, ScaleError>` (replaces the `(f64, bool)` tuple).

- [ ] **Step 1: Write the failing tests** (in `scale.rs` `mod tests`; update the two existing parse tests to read `.weight_g` / `.stable`)

```rust
#[test]
fn reads_the_resolution_from_the_decimals() {
    assert_eq!(parse_sics_weight(b"S S      740.5 g\r\n").unwrap().resolution_g, 0.1);
    assert_eq!(parse_sics_weight(b"S S     740.25 g\r\n").unwrap().resolution_g, 0.01);
    assert_eq!(parse_sics_weight(b"S S        740 g\r\n").unwrap().resolution_g, 1.0);
}
```

- [ ] **Step 2: Run, verify it fails.** `cargo test -p fermentool-core --lib scale::tests` → compile error, no field `resolution_g`.

- [ ] **Step 3: Implement**

```rust
/// One MT-SICS weight reply.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SicsReading {
    pub weight_g: f64,
    pub stable: bool,
    /// The display step, read from the number's decimals (`740.5` → 0.1 g).
    pub resolution_g: f64,
}
```

In `parse_sics_weight`, after the unit check:

```rust
    let decimals = parts[2].split_once('.').map_or(0, |(_, frac)| frac.len());
    Ok(SicsReading {
        weight_g: value,
        stable,
        resolution_g: 10f64.powi(-(decimals as i32)),
    })
```

In `engine/mod.rs`, `read_scale` keeps returning `Option<(f64, bool)>` for now: map `Ok(r) => (r.weight_g, r.stable)` and store `r.resolution_g` in a new `Engine` field `scale_resolution_g: f64` (default `0.1`).

- [ ] **Step 4: Run, verify pass.** `cargo test -p fermentool-core` → all pass.

- [ ] **Step 5: Commit** `git commit -m "feat(core): read the balance resolution from its reply"`

---

### Task 2: Tracking math (pure)

**Files:**
- Create: `crates/fermentool-core/src/tracking.rs`
- Modify: `crates/fermentool-core/src/lib.rs`

**Interfaces:**
- Consumes: `trim::{decimate, theil_sen_slope, MAX_SLOPE_POINTS, TRIM_MIN, TRIM_MAX}`
- Produces:
  - `pub struct TrackPoint { pub t_s: f64, pub commanded_g: f64, pub delivered_g: f64 }` (Serialize, Deserialize, Clone, Copy)
  - `pub fn delivery_ratio(points: &[TrackPoint], resolution_g: f64) -> Option<f64>`
  - `pub fn next_c(prev_c: f64, k: f64, deficit_g: f64, demand_rate_g_s: f64) -> f64`
  - `pub fn needs_alarm(k: f64) -> bool`
  - `pub struct Anchor { pub weight_g: f64, pub commanded_g: f64, pub refill: bool }` (Serialize, Deserialize)
  - `pub fn settle_anchor(a: &Anchor, weight_now: f64, commanded_now: f64, k: Option<f64>, resolution_g: f64) -> f64`
  - constants `MIN_WINDOW_RESOLUTIONS = 200.0`, `MIN_WINDOW_S = 120.0`, `DEFICIT_HORIZON_S = 600.0`, `MAX_C_STEP = 0.02`, `UPDATE_EVERY_S = 10.0`, `ALARM_AFTER_SATURATED_UPDATES: u32 = 30`

- [ ] **Step 1: Write the failing tests** (end of `tracking.rs`)

```rust
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
        // 0.01 g/s for 600 s = 6 g, below 200 × 0.1 g = 20 g.
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
```

- [ ] **Step 2: Run, verify fail.** Add `pub mod tracking;` to `lib.rs`, then `cargo test -p fermentool-core --lib tracking::` → compile errors (items missing).

- [ ] **Step 3: Implement** (top of `tracking.rs`)

```rust
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
```

- [ ] **Step 4: Run, verify pass.** `cargo test -p fermentool-core --lib tracking::` → 11 passed.

- [ ] **Step 5: Commit** `git commit -m "feat(core): cumulative feed tracking math"`

---

### Task 3: Proof math (pure): R² and fitted µ

**Files:**
- Modify: `crates/fermentool-core/src/tracking.rs`

**Interfaces:**
- Produces:
  - `pub fn r_squared(required: &[f64], delivered: &[f64]) -> Option<f64>`
  - `pub fn fit_exponential_mu(points: &[(f64, f64)], mu_guess: f64) -> Option<f64>` (points: `(t_hours, delivered_ml)`)
  - `pub fn requested_mu_per_hour(spec: &fermentool_curves::CurveSpec) -> Option<f64>`

- [ ] **Step 1: Write the failing tests** (append to `tracking::tests`)

```rust
    #[test]
    fn r_squared_is_one_for_a_perfect_track_and_lower_otherwise() {
        let req: Vec<f64> = (0..100).map(|i| i as f64).collect();
        assert!((r_squared(&req, &req).unwrap() - 1.0).abs() < 1e-12);
        let off: Vec<f64> = req.iter().map(|v| v * 0.9).collect();
        assert!(r_squared(&req, &off).unwrap() < 0.99);
    }

    #[test]
    fn fits_the_mu_of_an_exponential_cumulative_volume() {
        // F(t) = 2 · e^(0.15 t) mL/h, so V(t) = 2/0.15 · (e^(0.15 t) − 1).
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
```

- [ ] **Step 2: Run, verify fail.** `cargo test -p fermentool-core --lib tracking::` → compile errors.

- [ ] **Step 3: Implement**

```rust
/// Coefficient of determination of `delivered` against `required` (both
/// cumulative, same length): `1 − Σ(d − r)² / Σ(r − r̄)²`.
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
/// delivered volume: least squares on `V(t) = F0/µ · (e^(µt) − 1)`, F0 in
/// closed form for each µ, µ by golden-section search on `ln µ` over
/// `[guess/4, guess·4]`.
pub fn fit_exponential_mu(points: &[(f64, f64)], mu_guess: f64) -> Option<f64> {
    if points.len() < 3 || !(mu_guess > 0.0) {
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
```

(Check `CurveKind` is exported from `fermentool_curves` and `start` is a public field; both are, see `crates/fermentool-curves/src/lib.rs`.)

- [ ] **Step 4: Run, verify pass.** `cargo test -p fermentool-core --lib tracking::` → 14 passed.

- [ ] **Step 5: Commit** `git commit -m "feat(core): R² and fitted µ for the delivered feed"`

---

### Task 4: The engine tracks the cumulative feed

**Files:**
- Modify: `crates/fermentool-core/src/engine/mod.rs`
- Modify: `crates/fermentool-core/src/trim.rs` (remove the old controller)

**Interfaces:**
- Consumes: everything from Tasks 1 to 3; `integrate_curve_mass(spec, start_s, end_s, density, ml_per_unit)`.
- Produces:
  - `#[derive(Default, Serialize, Deserialize)] struct Tracker { last_t_s: Option<f64>, required_g: f64, commanded_g: f64, delivered_g: f64, last_weight_g: Option<f64>, last_normal_commanded_g: f64, anchor: Option<tracking::Anchor>, #[serde(skip)] points: Vec<tracking::TrackPoint>, k: Option<f64>, last_update_t_s: f64, saturated_updates: u32 }`
  - `Engine.tracker: Tracker`
  - `pub struct TrackingStatus { pub required_ml: f64, pub delivered_ml: f64, pub deficit_pct: Option<f64>, pub delivery_ratio: Option<f64> }` in `EngineStatus.tracking: Option<TrackingStatus>` (Some only during a trimmed run)

- [ ] **Step 1: Write the failing tests** (engine `mod tests`; they replace `trim_c_converges_toward_the_pumps_real_delivery_ratio`, `a_correctly_delivering_pump_leaves_c_alone`, `the_first_normal_read_seeds_the_mass_balance_baseline`, `a_trim_alarm_is_not_erased_by_the_next_healthy_read`, `recover_scale_does_not_treat_a_trim_alarm_as_a_down_link`, `an_rpm_calibration_converts_the_curve_for_the_mass_balance`, `attach_scale_uses_the_configured_density`, which assert the old per-second behaviour; delete those seven)

```rust
    /// Simulate `secs` one-second balance reads of a bottle drained by a pump
    /// that delivers `k(t)` × what it is commanded, the command being the
    /// run's own curve × the engine's current c. The balance reports 0.1 g
    /// steps (ScriptedScale's format). Returns the true delivered grams.
    fn simulate(
        e: &mut Engine<SimPump>,
        spec: &CurveSpec,
        k: impl Fn(f64) -> f64,
        start_g: f64,
        from_s: i64,
        secs: i64,
    ) -> f64 {
        let mut w = start_g;
        let mut delivered = 0.0;
        for i in from_s..from_s + secs {
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            let rate = spec.value_at(Duration::from_secs(i as u64)) / 60.0; // g/s at 1 g/mL
            let out = k(i as f64) * rate * e.trim_c();
            w -= out;
            delivered += out;
        }
        delivered
    }

    #[test]
    fn delivered_mass_tracks_an_exponential_curve_to_r2_one() {
        // 1 ml/min, µ = 0.15/h, 6 h; the pump starts at 85 % and drifts to 92 %.
        let spec = CurveSpec::exponential_physio(1.0, 0.15, Duration::from_secs(6 * 3600));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let cfg = RunConfig { curve: spec.clone(), ..trim_cfg() };
        e.start_run(cfg, t0()).unwrap();
        let k = |t: f64| 0.85 + 0.07 * t / (6.0 * 3600.0);
        let truth = simulate(&mut e, &spec, k, 5_000.0, 0, 6 * 3600);
        let st = e.status();
        let tr = st.tracking.expect("tracking status during a trimmed run");
        assert!(st.scale_ok, "no alarm expected");
        // The engine's own count agrees with the truth to the balance step.
        assert!((tr.delivered_ml - truth).abs() < 0.5, "{} vs {truth}", tr.delivered_ml);
        // And the cumulative delivered volume matches the curve.
        let pct = tr.deficit_pct.unwrap();
        assert!(pct.abs() < 1.0, "final deficit {pct} %");
        // c sits on the pump's final inverse ratio.
        assert!((e.trim_c() - 1.0 / 0.92).abs() < 0.03, "c = {}", e.trim_c());
    }

    #[test]
    fn a_linear_curve_from_zero_is_tracked_too() {
        let spec = CurveSpec::linear(0.0, 10.0, Duration::from_secs(3 * 3600));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        simulate(&mut e, &spec, |_| 0.9, 5_000.0, 0, 3 * 3600);
        let pct = e.status().tracking.unwrap().deficit_pct.unwrap();
        assert!(pct.abs() < 1.0, "final deficit {pct} %");
    }

    #[test]
    fn a_refill_keeps_the_cumulative_count() {
        let spec = CurveSpec::linear(60.0, 60.0, Duration::from_secs(36_000));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        let mut truth = simulate(&mut e, &spec, |_| 0.9, 2_000.0, 0, 1800);
        // Bottle swapped: +1500 g, then the run goes on from the new weight.
        let w_after = 2_000.0 - truth + 1_500.0;
        truth += simulate(&mut e, &spec, |_| 0.9, w_after, 1800, 1800);
        let tr = e.status().tracking.unwrap();
        // The refill itself is estimated from k, so allow a few grams.
        assert!((tr.delivered_ml - truth).abs() < 5.0, "{} vs {truth}", tr.delivered_ml);
        assert!(tr.deficit_pct.unwrap().abs() < 1.0);
    }

    #[test]
    fn a_blocked_line_alarms_after_five_minutes_and_freezes_c() {
        let spec = CurveSpec::linear(60.0, 60.0, Duration::from_secs(36_000));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        simulate(&mut e, &spec, |_| 0.0, 2_000.0, 0, 120 + 10 * 29);
        assert!(e.status().scale_ok, "not before 30 saturated updates");
        simulate(&mut e, &spec, |_| 0.0, 2_000.0, 410, 30);
        assert!(!e.status().scale_ok, "a line delivering nothing must alarm");
        let c = e.trim_c();
        simulate(&mut e, &spec, |_| 0.0, 2_000.0, 440, 60);
        assert_eq!(e.trim_c(), c, "c frozen by the alarm");
    }

    #[test]
    fn a_very_low_flow_never_false_alarms() {
        // 0.05 ml/min on a 0.1 g balance: 3 g/h, the window needs 20 g.
        let spec = CurveSpec::linear(0.05, 0.05, Duration::from_secs(10 * 3600));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        simulate(&mut e, &spec, |_| 0.95, 500.0, 0, 10 * 3600);
        assert!(e.status().scale_ok);
        assert!(e.status().tracking.unwrap().deficit_pct.unwrap().abs() < 3.0);
    }

    #[test]
    fn an_rpm_calibration_converts_the_curve_for_tracking() {
        // Calibrated at 50 rpm -> 600 g in 5 min = 120 ml/min, 2.4 ml/min
        // per rpm; a flat 50 rpm run delivering exactly that keeps c at 1.
        let mut e = engine();
        let cal = seed_calibration(&e.store, ControlVar::Rpm, 50.0, 600.0);
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let spec = CurveSpec::linear(50.0, 50.0, Duration::from_secs(36_000));
        let cfg = RunConfig {
            control_var: ControlVar::Rpm,
            gravimetric_trim: true,
            tubing_calibration_id: Some(cal),
            curve: spec.clone(),
            ..linear_cfg()
        };
        e.start_run(cfg, t0()).unwrap();
        let mut w = 10_000.0;
        for i in 0..900 {
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            w -= 2.0 * e.trim_c();
        }
        assert!(e.status().scale_ok);
        assert!((e.trim_c() - 1.0).abs() < 0.01, "c = {}", e.trim_c());
    }
```

Also update `a_resumed_rpm_trim_run_keeps_its_volume_conversion` to drain `2.0 * b.trim_c()` for 900 ticks and assert `(b.trim_c() - 1.0).abs() < 0.01` instead of `scale_ok` (the new controller needs a window before it moves c).

- [ ] **Step 2: Run, verify fail.** `cargo test -p fermentool-core --lib engine::tests` → compile error, no field `tracking` on `EngineStatus`.

- [ ] **Step 3: Implement**

1. Add `Tracker` (Interfaces above) and `Engine.tracker: Tracker` (initialised `Tracker::default()`); add `tracker: Tracker` to `PersistedTrim` with `#[serde(default)]`, save and restore it like `trim_c`.
2. `start_run`: in the reset block, `self.tracker = Tracker { last_t_s: Some(0.0), ..Tracker::default() };`.
3. `scale_tick`, right after the successful read, before the state machine, integrate the three masses:

```rust
        let spec = active.spec.clone(); // `active` borrowed immutably above
        let t = now.duration_since(active.started_at).as_secs_f64().max(0.0);
        let from = self.tracker.last_t_s.unwrap_or(t);
        let ml_per_unit = self.rpm_to_ml_min.unwrap_or(1.0);
        let req = integrate_curve_mass(&spec, from, t, self.scale_density_g_per_ml, ml_per_unit);
        self.tracker.required_g += req;
        self.tracker.commanded_g += req * self.trim_c;
        self.tracker.last_t_s = Some(t);
```

4. After `self.scale_state = next;` replace everything down to the end of `scale_tick` (the old seed / weight_buffer / `integrate_curve_mass` / `update_trim` block, keep the Theil-Sen `last_rate_g_per_min` diagnostic) with:

```rust
        if self.scale_state != trim::ScaleState::Normal {
            let refill = matches!(
                self.scale_state,
                trim::ScaleState::RefillPending | trim::ScaleState::RefillSettling
            );
            let anchor = self.tracker.anchor.get_or_insert(tracking::Anchor {
                weight_g: self.tracker.last_weight_g.unwrap_or(weight_g),
                commanded_g: self.tracker.last_normal_commanded_g,
                refill: false,
            });
            anchor.refill |= refill;
            self.save_trim_state();
            return;
        }

        let tr = &mut self.tracker;
        if let Some(a) = tr.anchor.take() {
            tr.delivered_g += tracking::settle_anchor(&a, weight_g, tr.commanded_g, tr.k, self.scale_resolution_g);
            if a.refill {
                tr.points.clear();
            }
        } else if let Some(w0) = tr.last_weight_g {
            tr.delivered_g += w0 - weight_g;
        } else {
            tr.delivered_g += tr.commanded_g; // first read: under a second of flow, nominal
        }
        tr.last_weight_g = Some(weight_g);
        tr.last_normal_commanded_g = tr.commanded_g;
        tr.points.push(tracking::TrackPoint { t_s: t, commanded_g: tr.commanded_g, delivered_g: tr.delivered_g });
        if tr.points.len() > WEIGHT_BUFFER_CAP {
            let keep = tr.points.len() - WEIGHT_BUFFER_CAP / 2;
            tr.points.drain(..keep.min(tr.points.len()));
        }

        if self.scale_ok && t - tr.last_update_t_s >= tracking::UPDATE_EVERY_S {
            tr.last_update_t_s = t;
            if let Some(k) = tracking::delivery_ratio(&tr.points, self.scale_resolution_g) {
                tr.k = Some(k);
                tr.saturated_updates = if tracking::needs_alarm(k) { tr.saturated_updates + 1 } else { 0 };
                if tr.saturated_updates >= tracking::ALARM_AFTER_SATURATED_UPDATES {
                    self.scale_ok = false;
                } else {
                    let rate = spec.value_at(Duration::from_secs_f64(t)) * ml_per_unit
                        * self.scale_density_g_per_ml / 60.0;
                    let deficit = tr.required_g - tr.delivered_g;
                    self.trim_c = tracking::next_c(self.trim_c, k, deficit, rate);
                }
            }
        }
        self.save_trim_state();
```

(The point buffer keeps the most recent half when full: the slope only uses a recent suffix, so old points are never needed.)

5. `resume`: after restoring, if `self.tracker.last_weight_g` is `Some(w)` and no anchor is set, set `self.tracker.anchor = Some(tracking::Anchor { weight_g: w, commanded_g: self.tracker.last_normal_commanded_g, refill: false })`, so the downtime is measured by the weight difference.
6. `status()`: `tracking: self.active.as_ref().filter(|a| a.gravimetric_trim).map(|_| { let ml = |g: f64| g / self.scale_density_g_per_ml; TrackingStatus { required_ml: ml(self.tracker.required_g), delivered_ml: ml(self.tracker.delivered_g), deficit_pct: (self.tracker.required_g > 0.0).then(|| 100.0 * (self.tracker.required_g - self.tracker.delivered_g) / self.tracker.required_g), delivery_ratio: self.tracker.k } })`. Mirror `tracking` in `control::DaemonStatus`.
7. `trim.rs`: delete `update_trim`, `TrimOutcome`, `TRIM_IGNORE_BAND`, `TRIM_ALARM_BAND`, `TRIM_GAMMA`, `TRIM_MAX_DELTA` and `mod trim_tests`; keep `TRIM_MIN`, `TRIM_MAX`. Remove `refill_weight_g`/`refill_at` only if nothing else reads them after this change (the Theil-Sen rate diagnostic uses `refill_at`; keep both if so).

- [ ] **Step 4: Run, verify pass.** `cargo test -p fermentool-core` → all pass, including the 6 new tests. If `delivered_mass_tracks_an_exponential_curve_to_r2_one` fails on the deficit bound, print `tr.deficit_pct` and `e.trim_c()` every 600 s before changing any constant; the Design's constants are the contract.

- [ ] **Step 5: Commit** `git commit -m "feat(core): track the cumulative delivered feed instead of stepping the trim"`

---

### Task 5: Journal the weight and survive a crash

**Files:**
- Create: `crates/fermentool-core/src/store/migrations/0004_tick_weight.sql`
- Modify: `crates/fermentool-core/src/store/mod.rs`, `crates/fermentool-core/src/engine/mod.rs`

**Interfaces:**
- Produces: `NewTick.weight_g: Option<f64>`, `NewTick.delivered_g: Option<f64>`, same on `TickRow`; migration version 4.

- [ ] **Step 1: Write the failing tests**

In `store/mod.rs` tests (and bump both `user_version` asserts from 3 to 4):

```rust
    #[test]
    fn a_tick_carries_the_weight_and_delivered_mass() {
        let s = Store::open_in_memory().unwrap();
        let run = s.insert_run(&sample_run()).unwrap();
        s.append_tick(&NewTick {
            run_id: run, seq: 0, wall_time: ts("2026-09-01T09:30:01Z"), elapsed_s: 1.0,
            target: 1.0, written_ok: true, readback: None, note: None,
            weight_g: Some(742.5), delivered_g: Some(0.3),
        }).unwrap();
        let t = s.last_tick(run).unwrap().unwrap();
        assert_eq!(t.weight_g, Some(742.5));
        assert_eq!(t.delivered_g, Some(0.3));
    }
```

In engine tests:

```rust
    #[test]
    fn tracking_survives_a_crash_resume() {
        let db = TempDb::new();
        let spec = CurveSpec::linear(60.0, 60.0, Duration::from_secs(36_000));
        let before = {
            let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
            a.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
            a.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
            simulate(&mut a, &spec, |_| 1.0, 5_000.0, 0, 600);
            a.status().tracking.unwrap()
        };
        // 60 s down, the pump kept delivering 1 g/s: the bottle is 60 g lighter.
        let mut b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        b.resume(at(660), grace()).unwrap();
        let w = 5_000.0 - before.delivered_ml - 60.0;
        b.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
        b.scale_tick(at(660));
        let after = b.status().tracking.unwrap();
        assert!((after.delivered_ml - (before.delivered_ml + 60.0)).abs() < 0.5);
        assert!((after.required_ml - (before.required_ml + 60.0)).abs() < 0.5);
    }

    #[test]
    fn the_journal_records_the_weight_during_a_trimmed_run() {
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[1000.0])));
        let id = e.start_run(flat_60_ml_min_cfg(), t0()).unwrap();
        e.scale_tick(t0());
        e.tick(at(1)).unwrap();
        let t = e.store.last_tick(id).unwrap().unwrap();
        assert_eq!(t.weight_g, Some(1000.0));
        assert!(t.delivered_g.is_some());
    }
```

- [ ] **Step 2: Run, verify fail.** `cargo test -p fermentool-core` → compile errors (fields missing).

- [ ] **Step 3: Implement**

`0004_tick_weight.sql`:

```sql
-- Balance reading and cumulative delivered mass at each journal tick, for
-- the tracking proof (chart, R², fitted µ). NULL when no balance was read.
ALTER TABLE ticks ADD COLUMN weight_g REAL;
ALTER TABLE ticks ADD COLUMN delivered_g REAL;
```

Register it as `Migration { version: 4, sql: include_str!("migrations/0004_tick_weight.sql") }`. Add the two fields to `NewTick`, `TickRow`, `TICK_COLS`, `append_tick` (`?9, ?10`) and `row_to_tick` (indices 9, 10). Fix every other `NewTick { .. }` literal (3 sites: `grep -rn "NewTick {" crates/`) with `weight_g: None, delivered_g: None`. In `Engine::tick`, fill them: `weight_g: self.live_weight.map(|(w, _)| w)`, `delivered_g: active_is_trimmed.then_some(self.tracker.delivered_g)` (capture `active.gravimetric_trim` at the top of `tick` with the other copies). The resume test passes once Task 4 step 3.5 (anchor from the persisted weight) is in, because `Tracker` is persisted by `save_trim_state` on every read.

- [ ] **Step 4: Run, verify pass.** `cargo test -p fermentool-core` → all pass.

- [ ] **Step 5: Commit** `git commit -m "feat(core): journal the weight and delivered mass each tick"`

---

### Task 6: Tracking report API

**Files:**
- Modify: `crates/fermentool-core/src/engine/mod.rs`, `control.rs`, `api.rs`

**Interfaces:**
- Produces:
  - `#[derive(Serialize)] pub struct TrackingReport { pub points: Vec<[f64; 3]>, pub r_squared: Option<f64>, pub deficit_ml: f64, pub deficit_pct: Option<f64>, pub mu_requested: Option<f64>, pub mu_delivered: Option<f64> }` (points: `[t_s, required_ml, delivered_ml]`, at most 2000)
  - `Engine::tracking_report(&self, run_id: i64) -> Result<Option<TrackingReport>>`
  - `Command::Tracking(i64, oneshot::Sender<Result<Option<TrackingReport>, String>>)`
  - `GET /api/runs/{id}/tracking` → 200 report, 404 when the run has no delivered data

- [ ] **Step 1: Write the failing tests**

Engine:

```rust
    #[test]
    fn the_tracking_report_proves_an_exponential_run() {
        let spec = CurveSpec::exponential_physio(1.0, 0.15, Duration::from_secs(4 * 3600));
        let mut e = engine();
        e.attach_scale_for_test(Box::new(ScriptedScale::new(&[])));
        let id = e.start_run(RunConfig { curve: spec.clone(), ..trim_cfg() }, t0()).unwrap();
        let mut w = 5_000.0;
        for i in 0..4 * 3600 {
            e.attach_scale_for_test(Box::new(ScriptedScale::new(&[w])));
            e.scale_tick(at(i));
            e.tick(at(i)).unwrap();
            w -= 0.9 * spec.value_at(Duration::from_secs(i as u64)) / 60.0 * e.trim_c();
        }
        let r = e.tracking_report(id).unwrap().expect("report");
        assert!(r.r_squared.unwrap() > 0.999, "R² {:?}", r.r_squared);
        assert!((r.mu_requested.unwrap() - 0.15).abs() < 1e-9);
        assert!((r.mu_delivered.unwrap() - 0.15).abs() < 0.01, "µ {:?}", r.mu_delivered);
        assert!(r.points.len() <= 2000);
    }

    #[test]
    fn no_report_for_a_run_without_a_balance() {
        let mut e = engine();
        let id = e.start_run(linear_cfg(), t0()).unwrap();
        e.tick(at(1)).unwrap();
        assert!(e.tracking_report(id).unwrap().is_none());
    }
```

API (`api.rs` tests):

```rust
    #[tokio::test]
    async fn tracking_is_404_for_a_run_without_balance_data() {
        let app = router(test_state());
        let res = app.oneshot(get("/api/runs/1/tracking")).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
```

- [ ] **Step 2: Run, verify fail.** `cargo test -p fermentool-core` → compile errors.

- [ ] **Step 3: Implement** `tracking_report`:

```rust
    /// Chart, R² and fitted µ for a run, from its journal. `None` when the
    /// run has no delivered-mass data (no balance, or not trimmed).
    pub fn tracking_report(&self, run_id: i64) -> Result<Option<TrackingReport>> {
        let Some(run) = self.store.run(run_id)? else { return Ok(None) };
        let ticks: Vec<(f64, f64)> = self
            .store
            .ticks(run_id, 0, i64::MAX)?
            .into_iter()
            .filter_map(|t| t.delivered_g.map(|d| (t.elapsed_s, d)))
            .collect();
        if ticks.len() < 2 {
            return Ok(None);
        }
        let rho = self.scale_density_g_per_ml;
        let ml_per_unit = match run.control_var {
            ControlVar::MlMin => 1.0,
            ControlVar::Rpm => match run.tubing_calibration_id {
                Some(id) => self.store.calibration(id)?.map_or(1.0, |c| calibration_ml_per_rpm(&c)),
                None => 1.0,
            },
        };
        let sampled = trim::decimate(&ticks, 2000);
        let mut required_ml = Vec::with_capacity(sampled.len());
        let (mut acc, mut prev) = (0.0, 0.0);
        for &(t, _) in &sampled {
            acc += integrate_curve_mass(&run.curve, prev, t, rho, ml_per_unit) / rho;
            required_ml.push(acc);
            prev = t;
        }
        let delivered_ml: Vec<f64> = sampled.iter().map(|&(_, d)| d / rho).collect();
        let (req_end, del_end) = (*required_ml.last().unwrap(), *delivered_ml.last().unwrap());
        let mu_requested = tracking::requested_mu_per_hour(&run.curve);
        let mu_delivered = mu_requested.and_then(|mu| {
            let pts: Vec<(f64, f64)> =
                sampled.iter().zip(&delivered_ml).map(|(&(t, _), &v)| (t / 3600.0, v)).collect();
            tracking::fit_exponential_mu(&pts, mu)
        });
        Ok(Some(TrackingReport {
            points: sampled
                .iter()
                .zip(required_ml.iter().zip(&delivered_ml))
                .map(|(&(t, _), (&r, &d))| [t, r, d])
                .collect(),
            r_squared: tracking::r_squared(&required_ml, &delivered_ml),
            deficit_ml: req_end - del_end,
            deficit_pct: (req_end > 0.0).then(|| 100.0 * (req_end - del_end) / req_end),
            mu_requested,
            mu_delivered,
        }))
    }
```

In `control.rs`: the `Command::Tracking` variant and its `handle` arm (`let _ = reply.send(engine.tracking_report(id).map_err(|e| e.to_string())); false`). In `api.rs`: route `.route("/api/runs/{id}/tracking", get(get_tracking))`, handler mapping `Ok(None)` to `ApiError::NotFound`, `Ok(Some(r))` to `Json(r)`.

- [ ] **Step 4: Run, verify pass.** `cargo test -p fermentool-core` → all pass.

- [ ] **Step 5: Commit** `git commit -m "feat(core): tracking report with R² and fitted µ"`

---

### Task 7: Show the proof in the UI

**Files:**
- Create: `ui/src/components/TrackingPanel.svelte`
- Modify: `ui/src/routes/Overview.svelte`, `ui/src/routes/History.svelte`, `ui/dist` (build output)

**Interfaces:**
- Consumes: `GET /api/runs/{id}/tracking` (Task 6), `status.tracking` (Task 4), `Chart` props `{ planned, actual, nowS, durationS, unit, digits }` with series as `[[t_s, value], ...]`.

- [ ] **Step 1: `TrackingPanel.svelte`**

```svelte
<script>
  // Delivered (weighed) vs requested feed volume for one run, with R², the
  // cumulative deficit and, for an exponential curve, requested vs fitted µ.
  import { get } from '../lib/api.js';
  import { num } from '../lib/fmt.js';
  import Chart from './Chart.svelte';

  let { runId, durationS, live = false } = $props();
  let report = $state(null);

  $effect(() => {
    const id = runId;
    let stop = false;
    const load = () =>
      get(`/api/runs/${id}/tracking`)
        .then((r) => !stop && (report = r))
        .catch(() => !stop && (report = null));
    load();
    // While running, refresh every 10 s (the controller's own cadence).
    const t = live ? setInterval(load, 10_000) : null;
    return () => {
      stop = true;
      if (t) clearInterval(t);
    };
  });

  const requested = $derived(report ? report.points.map((p) => [p[0], p[1]]) : []);
  const delivered = $derived(report ? report.points.map((p) => [p[0], p[2]]) : []);
</script>

{#if report}
  <section class="card track">
    <div class="eyebrow">Feed delivered vs requested (weighed)</div>
    <Chart planned={requested} actual={delivered} nowS={null} {durationS} unit="mL" digits={1} />
    <div class="stats mono">
      <span>R² <b>{report.r_squared != null ? report.r_squared.toFixed(5) : '·'}</b></span>
      <span>deficit <b>{num(report.deficit_ml, 1)} mL</b>{report.deficit_pct != null ? ` (${num(report.deficit_pct, 2)} %)` : ''}</span>
      {#if report.mu_requested != null}
        <span>µ requested <b>{report.mu_requested.toFixed(4)} h⁻¹</b></span>
        <span>µ delivered <b>{report.mu_delivered != null ? report.mu_delivered.toFixed(4) : '·'} h⁻¹</b></span>
      {/if}
    </div>
  </section>
{/if}

<style>
  .track { margin-top: var(--s-5); }
  .track .eyebrow { margin-bottom: var(--s-3); color: var(--ink); }
  .stats { display: flex; flex-wrap: wrap; gap: var(--s-2) var(--s-5); font-size: 13px; margin-top: var(--s-3); }
</style>
```

- [ ] **Step 2: Overview.** In the active-run view, when `app.status.active.gravimetric_trim`, render `<TrackingPanel runId={app.status.active.run_id} durationS={app.status.active.duration_s} live />` below the existing run chart (the `<Chart` at `Overview.svelte:292`).

- [ ] **Step 3: History.** In the selected-run detail, below the existing `<Chart` (`History.svelte:236`), render `<TrackingPanel runId={sel.run.id} durationS={sel.run.duration_s} />`; it renders nothing for a run without balance data (404).

- [ ] **Step 4: Build.** `cd ui && npm run build` → built, no warnings.

- [ ] **Step 5: Smoke (isolated).** Release build, run it with `APPDATA` pointing at a scratch dir holding a `config.toml` with `serial.path = "sim"`, `allow_simulator = true`, `port = 8739`, `[scale] path` = the balance's COM port, and `FERMENTOOL_NO_BROWSER=1`. Start a trimmed ml/min run; check `/api/status` carries `tracking` and `/api/runs/{id}/tracking` answers once ticks exist. (With the simulated pump nothing drains, so the controller will alarm after 5 min: expected.) Shut down with `POST /api/shutdown`.

- [ ] **Step 6: Commit** `git add ui/src ui/dist && git commit -m "feat(ui): delivered vs requested feed chart with R² and µ"`

---

## Self-Review

1. **Design coverage:** control law (Task 2 `next_c`, Task 4 wiring), k estimation with resolution-aware window (Tasks 1, 2), cumulative accounting across perturbation/refill/first read/resume (Tasks 2, 4, 5), alarm on saturation (Tasks 2, 4), any curve shape (Task 4 exponential + linear-from-zero tests, report generic), proof chart + R² + µ (Tasks 3, 6, 7), journaled weight (Task 5).
2. **Placeholders:** none; every code step carries its code.
3. **Type consistency:** `TrackPoint`, `Anchor`, `delivery_ratio`, `next_c`, `needs_alarm`, `settle_anchor` defined in Task 2 and used with the same signatures in Task 4; `r_squared`, `fit_exponential_mu`, `requested_mu_per_hour` defined in Task 3 and used in Task 6; `TrackingStatus` (Task 4) and `TrackingReport` (Task 6) consumed by Task 7 with the same field names.
4. **Review Focus:** all five lines are pinned by a named test in their owning task.
