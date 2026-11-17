# Rate change, part 2: the hold of every curve, and a fresh start for the correction

Date: 2026-10-27 (replaces this file's 2026-10-26 draft, written before the
2026-10-22 implementation was found merged). Status: design, decisions taken
with the user, awaiting approval of the plan.

Builds on: `docs/superpowers/specs/2026-10-22-constant-rate-change-design.md`
and its plan (merged, `301ae54`): a Constant run's rate changes mid-run,
stored in `setpoint_changes`, driven through `RunRow::effective_curve()`,
notified (`setpoint_change`), shown in History, offered back by Run again.
Mockup: https://claude.ai/artifact/2JRodCC5X7CbAALsE5Mhau. Owed-feed study
(the three strategies simulated): https://claude.ai/artifact/T5WP3wTXr9otYbezmHwUrt.

## Why

1. The user wants the rate changeable in the **hold** of every curve, not
   only of a Constant run: past its end, any curve is a flat rate.
2. A rate change today leaves the balance correction as it was: the deficit
   accumulated at the old rate is paid back over 10 min **at the new
   rate**. After a 5 to 0.5 ml/min cut with 1 g behind, c jumps from +2 %
   to +20 %, right when the operator asked for less substrate; after a 0.5
   to 5 ml/min rise, the +20 % that was right at 0.5 ml/min only ramps down
   0.5 % per 10 s and overfeeds 2 to 3 g. The ratio windows also mix the two
   speeds for 5 min, and a peristaltic tube's ratio is not quite the same at
   every speed.

## Decisions (the user, 2026-10-23 and 2026-10-27)

- **Hold changes for every shape.** Constant: any time (as today). Linear,
  Exponential, Sigmoid, Step, Custom: once `curve_done`, never before.
- **A rate change resets the correction's integral, nothing else** ("comme
  si on revenait à zéro du PID", option "retard seulement"):
  - the owed feed (requested minus weighed) at the change is **written off**
    for the regulation: not paid back;
  - c drops its catch-up share at once and stands on its feed-forward:
    `1/k` with the pump ratio measured, the run's seed c before that;
  - the ratio window restarts at the change; c is **not** sent back to the
    tubing calibration's c0 and the ratio learned so far stays the feed-forward.
- **The report judges each rate step on its own** ("repart à 0 à chaque
  palier"): the gap chart and the verdict restart at 0 at each change; the
  gap written off stays visible on its own step and is listed once.
- Every change is notified (exists).

## Non-goals

- Changing the shape, the duration or the unit of a curve, or a rate
  change before the end of a non-constant curve.
- A per-speed tube calibration (the deferred multi-speed idea): c keeps the
  ratio it learned; the restarted window adapts it at the new speed.
- Changing what an alarm's own write-off does (`forgiven_g`).

## Anti-goals

- `CurveSpec::value_at` impure, or `runs.curve_params` rewritten.
- A non-constant run's curve **reshaped** by a hold change: its duration
  must not stretch (stretching it rescales an exponential or a sigmoid).
- A change read by the balance logic as a refill, a perturbation, a stopped
  feed, a saturated pump or a wrong side.
- A change during a refill or a touch writing off a gap the balance could
  not measure (the delivered count is frozen at the anchor then).
- A real stopped feed or a real saturation after a change going unnoticed.
- Run again turning a non-constant run into a Constant one.
- A run with no change behaving, journalling or reporting differently.

## Design

### 1. The curve: holds for every shape (`fermentool-curves`)

- `CurveSpec` gains `overrides: Vec<StepSegment>` (`#[serde(default,
  skip_serializing_if = "Vec::is_empty")]`): from each `at_seconds` on, the
  value is that segment's, whatever the shape, past the duration included.
  `value_at` looks at the raw elapsed time **before** clamping it to the
  duration; the value goes through the same clamps.
- `with_rate_changes`: Constant as today (a Step curve, its duration
  stretched to the last change, the UI relies on it); every other shape: a
  clone with `overrides` = the changes, **duration unchanged**.
- `peak_value` includes the override values. `preview` is unchanged past
  the duration (the charts end at the curve's end).
- Old JSON without `overrides` reads as empty: every stored run is unchanged.

### 2. The daemon accepts a hold change

- `rate_bounds` gives bounds for a dosing run whose curve is Constant **or**
  `curve_done`. `set_constant_rate` becomes `set_rate` (same checks, same
  order: pump first, then the row).
- `EngineError::NotConstant` becomes `RateNotYet`, code `rate_not_yet`,
  `Display` "the rate can change only on a Constant run, or once the curve
  is done", the hint saying why a running exponential has no single rate.
- The `setpoint_change` event and its notification say `in the hold` when
  the curve is done.

### 3. The correction resets at a change (`engine`, `tracking`)

- `Tracker` gains `reset_g: f64` (owed feed written off at rate changes,
  separate from the alarms' `forgiven_g`) and `reset_pending: Option<f64>`
  (the elapsed time of the change whose write-off waits), both
  persisted with `#[serde(default)]`. The deficit every law uses becomes one
  method, `Tracker::owed_g() = required_g - delivered_g - forgiven_g -
  reset_g` (today written out at four places).
- At a change, on a trimmed run, before the new setpoint is computed:
  - with the balance `Normal`: `reset_g += owed_g()` at once;
  - out of `Normal` (refill, touch, anchor held): `reset_pending =
    Some(elapsed_s)`;
    the first `Normal` read, once the anchor is settled, does the write-off
    (the gap then is measured, not frozen);
  - c, unless an alarm freezes it: `1/k` clamped to the bounds when `k` is
    known, else the seed c; the new rate is written with that c;
  - the windows restart: `points` cleared, `saturated_updates = 0`,
    `saturated_since_t = None`, `flow_ref = None`;
  - an alarm in force stays in force (c stays frozen); its recovery check
    sees only what follows the change.
- The written-off grams are recorded on the change's row:
  `setpoint_changes.reset_g REAL` (migration 0016, NULL on older rows),
  set by `Store::set_change_reset(run_id, elapsed_s, g)` when the write-off
  happens (at once, or at the deferred read).
- `TrackingStatus` gains `reset_ml`; its `deficit_pct` is computed from
  `owed_g()` plus `forgiven_g` (the gap since the last change, alarms'
  write-offs still shown as a gap as today).

### 4. The report per step

- `TrackingReport` gains `resets: Vec<[f64; 2]>` = `[elapsed_s, ml]` per
  change with a recorded write-off. `points` stay the true cumulative
  requested and delivered.
- `rundetail.js`: the gap at `t` is `d - r + sum(resets with t_i <= t)`
  in `gapSeries`, `gapExtremes` and `verdict` (its worst share and its
  on/off target); `requested` and `delivered` stay the true totals. The
  verdict's gap figure is the one since the last change; the Regulation
  fold lists `Written off at rate changes: 1.20 mL` (once, summed).
- The History gap chart marks each change with the same numbered dashed
  line as the Feed rate chart.

### 5. The UI for hold changes

- `canChangeRate`: the daemon's `rate_bounds` decides (an array), not the
  curve kind.
- The dialog: "New hold rate" in the hold of a non-constant curve, the text
  "From now until you stop the run, or the next change."
- Overview: under the figure, `holding since 26 h 10` once a hold rate was
  changed; the hold band and the "Curve done" line show the held value.
- History chips: a non-constant run changed in the hold gets one more chip,
  `hold 2.500 ml/min` (the last value). Run again on it opens the original
  curve, **never** the rate picker (that one is for Constant runs only).
- Smoke: Change rate is shown on a Constant run or a run in its hold,
  never on another; nothing is changed.

## The alarms and the balance logic, case by case

Each line is a test in the plan; "today" is the merged code.

| Case | Expected |
|---|---|
| 10x cut with 1 g owed | owed 0 at the change, c = 1/k at once, no `alarm_*`, gap within 0.3 g of 0 for the next 30 min |
| 10x rise with 1 g owed | same, and no overshoot past -0.3 g (today about -2 g) |
| 1 g ahead, then a cut | same with the sign reversed |
| tube ratio differs by speed (0.95 fast, 1.05 slow) | no `Saturated`; k reaches the new ratio within 2 windows |
| feed really stops 10 min after a change | `alarm_feed_stopped` still raised |
| pump really saturates after a change (ratio 0.6) | `alarm_saturated` still raised |
| change during a refill | write-off deferred to the first `Normal` read, equal to the gap measured there; no `refill` event from the change |
| 0.5 to 10 ml/min rise | not read as a refill or a perturbation |
| change during `FeedStopped` | alarm stays, c unchanged, clears as before once the feed flows |
| change before the ratio is measured | c = seed, early law continues, no alarm |
| crash with a write-off pending | resume does it at its first `Normal` read |
| change in the hold of an exponential | value from the change on, shape and duration unchanged before it |
| no change | identical status, journal and report |

## Documentation

- `CLAUDE.md`, "Rate changes": every shape in the hold, the reset rule,
  `reset_g` vs `forgiven_g`, the report per step.
- `CHANGELOG.md`; the technical docs (the write-off, the per-step verdict).
