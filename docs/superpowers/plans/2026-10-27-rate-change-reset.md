# Rate change part 2: hold changes for every curve, correction reset: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the rate of any dosing run can be changed in its hold (a Constant one any time, as today), and every rate change resets the balance correction's integral: the owed feed is written off, c stands on its feed-forward, the windows restart, and the report judges each rate step on its own.

**Architecture:** `CurveSpec` gains `overrides` (flat values from a time on, any shape, duration untouched); `with_rate_changes` uses them for non-constant curves. The engine's `Tracker` gets one deficit method (`owed_g`) and a rate write-off (`reset_g`, deferred while the balance is out of `Normal`), recorded on the change's row so the report can rebase its gap per step. The UI reads the daemon's `rate_bounds` to offer the change and the report's `resets` to draw the gap.

**Tech Stack:** Rust (fermentool-curves, fermentool-core: rusqlite, axum), Svelte 5 + vitest, headless-Edge smoke.

**Spec:** `docs/superpowers/specs/2026-10-26-change-rate-design.md` (part 2). Part 1, merged: `docs/superpowers/specs/2026-10-22-constant-rate-change-design.md`, plan `docs/superpowers/plans/2026-10-22-constant-rate-change.md`.

## Global Constraints

- Nothing that works today may break: a run with no change behaves, journals, reports and displays exactly as before. Every task ends with `cargo test -p fermentool-core -p fermentool-curves` green; UI tasks also `npm run check` (0 errors, 0 warnings) and `npm test` from `ui/`.
- `CurveSpec::value_at` stays pure in elapsed time. `runs.curve_params` is never rewritten.
- A non-constant curve's duration never changes with a rate change.
- Rate changes: dosing runs only, never calibration measurements; non-constant curves only once `curve_done`.
- The pump is asked before anything is recorded (part 1 rule, `set_rate` keeps it).
- Build from the VS Community environment: `cmd /c '"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" && cargo ...'`. With a run active, heavy work at low priority (`start "" /b /low /wait ...`).
- UI strings in English, "measurement" never "burst"; no em dash, no spaced hyphen anywhere (code, docs, commits). UI logic in `ui/src/lib/` as tested functions. After a UI change, `npm run build` from `ui/`.
- Commits: no `Co-Authored-By` and no `Claude-Session` line (CLAUDE.md). Work on branch `feat/rate-change-reset`.
- Deploy only with no active run, re-checked by `/api/status` right before `POST /api/shutdown`.

## Review Focus

1. A hold change on an Exponential run: before the change the curve is exactly the original (same duration, same values); after, the new value. Test in Task 1 (`an_override_never_reshapes_the_curve`) and Task 3 (`an_exponential_changes_in_its_hold_only`).
2. A change while the balance is out of `Normal` (refill, touch): the write-off waits for the first `Normal` read and equals the gap measured there. Test in Task 4 (`a_change_during_a_refill_writes_off_after_it`).
3. Run again on a non-constant run changed in its hold opens the original curve, not the Constant rate picker. Test in Task 6 (`runAgainPicks` cases).
4. A real stopped feed or saturated pump after a change is still alarmed. Tests in Task 4 (`a_feed_stopped_after_a_change_is_still_seen`, `a_saturation_after_a_change_is_still_seen`).
5. A crash with a write-off pending: the resumed run writes it off at its first `Normal` read. Test in Task 4 (`a_pending_reset_survives_a_crash`).

---

### Task 1: overrides on the curve (pure)

**Files:**
- Modify: `crates/fermentool-curves/src/lib.rs` (`CurveSpec`, `value_at`, `peak_value`, `with_rate_changes`, `base`, tests)

**Interfaces:**
- Produces: `CurveSpec.overrides: Vec<StepSegment>` (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`, empty in `base`). `with_rate_changes(&self, changes: &[(f64, f64)]) -> CurveSpec`: Constant unchanged from today; any other shape with changes: `self.clone()` with `overrides` = the changes as `StepSegment`s, `duration` unchanged.

- [ ] **Step 1: Write the failing tests** in the curves test module:
  - `an_override_holds_from_its_time`: `exponential_endpoints(1.0, 8.0, 3600 s).with_rate_changes(&[(4000.0, 2.5)])`: `value_at(3600 s) == 8.0`, `value_at(3999 s) == 8.0`, `value_at(4000 s) == 2.5`, `value_at(90_000 s) == 2.5`; two changes `(4000, 2.5), (5000, 3.0)` give 3.0 at 5000 s.
  - `an_override_never_reshapes_the_curve`: same spec, `duration` equal before and after, and `value_at(t)` identical for every `t` in `0..=3600` step 60.
  - `an_override_is_clamped`: `.with_clamp(0.5, 10.0)` and an override of 20.0 gives 10.0.
  - `peak_counts_the_overrides`: override 12.0 past the end of a 1 to 8 linear curve: `peak_value() == 12.0`.
  - `old_json_has_no_overrides`: a spec serialised without the field deserialises with `overrides.is_empty()`, and a spec with none serialises without the key.
  - The existing `rate_changes_make_steps` and `no_change_is_the_same_curve` stay green, except its line `assert_eq!(l.with_rate_changes(&[(60.0, 3.0)]), l)`, which now expects `overrides == [StepSegment { at_seconds: 60.0, value: 3.0 }]`.
- [ ] **Step 2: Run** `cargo test -p fermentool-curves` : the new tests fail (no field).
- [ ] **Step 3: Implement.** In `value_at`, before clamping `t` to the duration: the last override with `at_seconds <= elapsed` (raw seconds) replaces `raw`, then the existing NaN guard and clamps apply. `peak_value` folds in each override's clamped value. Update every `CurveSpec { .. }` literal in the workspace (`grep -rn "CurveSpec {" crates`) with `overrides: Vec::new()` where needed.
- [ ] **Step 4: Run** `cargo test -p fermentool-curves -p fermentool-core`: PASS.
- [ ] **Step 5: Commit** `feat(curves): a rate held past the end of any curve, its shape untouched`.

### Task 2: the write-off on the change's row

**Files:**
- Create: `crates/fermentool-core/src/store/migrations/0016_setpoint_change_reset.sql`
- Modify: `crates/fermentool-core/src/store/mod.rs` (migration list, `SetpointChange`, `setpoint_changes`, new `set_change_reset`, tests)

**Interfaces:**
- Produces: `SetpointChange.reset_g: Option<f64>` (serialised as `reset_g`, null when none); `Store::set_change_reset(&self, run_id: i64, elapsed_s: f64, reset_g: f64) -> Result<()>` sets it on the run's change at that `elapsed_s` (the latest one at or before it).

- [ ] **Step 1: Write the failing tests**: `a_change_records_its_write_off` (add a change at 600 s, `set_change_reset(id, 600.0, 1.25)`, `setpoint_changes(id)[0].reset_g == Some(1.25)`); `a_change_without_write_off_reads_none`; `migrate_creates_schema_at_the_latest_version` updated to 16.
- [ ] **Step 2: Run** `cargo test -p fermentool-core store::` : FAIL.
- [ ] **Step 3: Implement**: `ALTER TABLE setpoint_changes ADD COLUMN reset_g REAL;` and the store code.
- [ ] **Step 4: Run** the store tests: PASS.
- [ ] **Step 5: Commit** `feat(store): a rate change keeps the feed it wrote off`.

### Task 3: a rate change in the hold of any curve

**Files:**
- Modify: `crates/fermentool-core/src/engine/mod.rs` (`EngineError::NotConstant` → `RateNotYet`, `rate_bounds`, `set_constant_rate` → `set_rate`, the event detail, tests), `crates/fermentool-core/src/control.rs:611`, `crates/fermentool-core/src/api.rs` (`set_active_rate` doc, its tests), `crates/fermentool-core/src/notify.rs` (message text, test)

**Interfaces:**
- Consumes: Task 1's `with_rate_changes` for every shape.
- Produces: `Engine::set_rate(&mut self, value: f64, elapsed_s: f64, now: Timestamp) -> Result<(f64, f64)>`; `EngineError::RateNotYet` (code `rate_not_yet`, `Display` `the rate can change only on a Constant run, or once the curve is done`, a `hint()`); `ActiveStatus.rate_bounds` is `Some` for a dosing run that is Constant or `curve_done`. The `setpoint_change` detail ends with `, in the hold` when `curve_done`.

- [ ] **Step 1: Write the failing tests** (engine module):
  - `an_exponential_changes_in_its_hold_only`: an exponential 1 to 8 ml/min over 1800 s; `set_rate(3.0, 900.0, ..)` is `Err(RateNotYet)` and records nothing; past 1800 s ticks, `set_rate(3.0, 2000.0, ..)` is `Ok((8.0, 3.0))`; the pump's setpoint is 3.0; `status().active.duration_s == 1800`; `effective_curve()` of the stored run gives 8.0 at 1800 s and 3.0 at 2000 s.
  - `a_hold_change_survives_a_crash`: same run, crash after the hold change, resume at 2600 s drives 3.0.
  - `rate_bounds_follow_the_hold`: `None` for a running linear curve, `Some` once `curve_done`, always `Some` for a Constant, `None` for a calibration measurement.
  - The rename: every existing `set_constant_rate` test calls `set_rate`; the `NotConstant` assertion becomes `RateNotYet` / `rate_not_yet`.
  - `notify`: `a_rate_change_is_one_info_message` gains a hold case whose message contains `in the hold`.
- [ ] **Step 2: Run** `cargo test -p fermentool-core rate`: FAIL.
- [ ] **Step 3: Implement.** `rate_bounds` checks `a.kind == Dosing && (Constant || a.curve_done)`. `set_rate` keeps part 1's order (bounds, lost link, pump write, row, event).
- [ ] **Step 4: Run** `cargo test -p fermentool-core`: PASS.
- [ ] **Step 5: Commit** `feat(engine): change the rate in the hold of any curve`.

### Task 4: the correction resets at a change

**Files:**
- Modify: `crates/fermentool-core/src/engine/mod.rs` (`Tracker`, `set_rate`, `scale_tick`, the four deficit sites at today's lines ~2663, 2693, 2732, 2904, `TrackingStatus`, tests)

**Interfaces:**
- Consumes: Task 2's `Store::set_change_reset`; Task 3's `set_rate`.
- Produces: `Tracker::owed_g(&self) -> f64` (`required_g - delivered_g - forgiven_g - reset_g`); `Tracker.reset_g: f64`, `Tracker.reset_pending: Option<f64>` (the `elapsed_s` of the change whose write-off waits), both `#[serde(default)]`; `TrackingStatus.reset_ml: f64`; `TrackingStatus.deficit_pct` from `required_g - delivered_g - reset_g`.

Behaviour at a change on a trimmed run, inside `set_rate`, before the target is computed:
- balance `Normal` and no anchor: `reset_g += owed_g()`, `set_change_reset(id, elapsed_s, that)`;
- otherwise `reset_pending = Some(elapsed_s)`; in `scale_tick`, right after the anchor is settled into `delivered_g`, a pending reset writes off `owed_g()` then, records it, clears the flag, saves the trim state;
- no alarm in force: `trim_c = bounds.clamp(1/k)` when `k` is known, else `c_seed` when known, else unchanged;
- always: `points.clear()`, `saturated_updates = 0`, `saturated_since_t = None`, `flow_ref = None`; `alarm` untouched;
- `save_trim_state()`.

- [ ] **Step 1: Write a test bench and the failing tests.** Bench: `trim_bench(spec: CurveSpec, changes: &[(i64, f64)], deliver: impl Fn(i64, f64) -> f64, end: i64) -> (Engine<SimPump>, i64, Vec<f64> /* c each second */, Vec<f64> /* owed_g each second */)`, where `deliver(i, commanded_g_per_s)` is the grams the bottle loses that second (generalises `constant_trimmed_bench`, which then calls it). Tests, one per row of the spec's table:
  - `a_cut_with_feed_owed_starts_clean`: Constant 5 ml/min, pump ratio 0.97, start 1 g behind by delivering 0 g for the first 12 s; cut to 0.5 at 1800 s: `owed_g` within ±0.05 g just after the change, `trim_c == 1/0.97` (±0.005) at the change, no `alarm_*` event, `|owed| <= 0.3` g over the next 1800 s; `setpoint_changes[0].reset_g ≈ 1.0` (±0.1).
  - `a_rise_with_feed_owed_does_not_overshoot`: 0.5 to 5 ml/min with 1 g owed: `owed_g >= -0.3` g at every second after the change.
  - `a_cut_with_feed_ahead_starts_clean`: 1 g ahead, same checks with the sign reversed.
  - `a_ratio_that_varies_with_speed_is_followed`: ratio 0.95 at 5 ml/min, 1.05 at 0.5; no `alarm_saturated`; `tracker k` within 0.01 of 1.05 by 900 s after the change.
  - `a_feed_stopped_after_a_change_is_still_seen`: deliver 0 g from 600 s after a cut to 2 ml/min: `alarm_feed_stopped` raised before 600 + 400 s.
  - `a_saturation_after_a_change_is_still_seen`: ratio 0.6 after the change (limit 25 %): `alarm_saturated` raised.
  - `a_change_during_a_refill_writes_off_after_it`: a +300 g jump starts a refill, the rate changes while `RefillSettling`, the level settles: `reset_g` recorded only after the first `Normal` read, equal to `owed_g` measured there (±0.1 g); no `refill` event other than the real one.
  - `a_big_rise_is_not_a_refill`: 0.5 to 10 ml/min: no `refill` event, scale state stays `Normal`.
  - `a_change_during_an_alarm_keeps_it`: during `FeedStopped`, a change leaves `alarm == Some(FeedStopped)` and `trim_c` unchanged; delivery back gives `alarm_cleared` as before.
  - `a_change_before_the_ratio_starts_from_the_seed`: change at 30 s on a run with calibration c0 1.05: `trim_c == 1.05` after it, no alarm.
  - `a_pending_reset_survives_a_crash`: change during a refill, crash before settling, resume: the first `Normal` read writes it off and records it.
  - `the_regulation_rides_a_change` (existing) stays green unchanged.
- [ ] **Step 2: Run** `cargo test -p fermentool-core engine::tests::a_` : the new tests FAIL.
- [ ] **Step 3: Implement** `owed_g` first (replace the four inline deficits; run the full suite, still green), then the reset as above.
- [ ] **Step 4: Run** `cargo test -p fermentool-core -p fermentool-curves`: PASS.
- [ ] **Step 5: Commit** `feat(trim): a rate change writes off the owed feed and restarts the windows`.

### Task 5: the report rebases its gap per step

**Files:**
- Modify: `crates/fermentool-core/src/engine/mod.rs` (`TrackingReport`, `tracking_report`, test), `crates/fermentool-core/src/api.rs` (test on `/api/runs/{id}/tracking`)

**Interfaces:**
- Consumes: Task 2's `SetpointChange.reset_g`.
- Produces: `TrackingReport.resets: Vec<[f64; 2]>` = `[elapsed_s, reset_g / rho]` for each change with `reset_g` set, in time order (empty otherwise, still serialised as `[]`).

- [ ] **Step 1: Write the failing tests**: `the_report_lists_each_write_off` (Task 4's cut bench: one entry at 1800 s, ≈1.0 mL at density 1); `a_run_without_change_reports_no_resets`; the API test checks the `resets` key.
- [ ] **Step 2: Run**: FAIL.
- [ ] **Step 3: Implement** from `run.setpoint_changes`.
- [ ] **Step 4: Run** `cargo test -p fermentool-core`: PASS.
- [ ] **Step 5: Commit** `feat(report): the write-off of each rate change`.

### Task 6: the UI logic

**Files:**
- Modify: `ui/src/lib/rundetail.js` (+ test), `ui/src/lib/ratechange.js` (+ test), `ui/src/lib/runview.js` (+ test)

**Interfaces:**
- Consumes: report `resets`, run `setpoint_changes`, status `active.rate_bounds`, `active.curve_done`.
- Produces:
  - `resetAt(resets, t) -> number` (sum of `ml` with `t_i <= t`); `verdict(report)`, `gapSeries(points, spans, windowS, resets)`, `gapExtremes(points, spans, resets)` use `d - r + resetAt(resets, t)`; `verdict` returns `resetMl` (the sum) and its `gap` is the rebased last gap; `requested`/`delivered` unchanged.
  - `canChangeRate(status, run)`: `Array.isArray(a.rate_bounds)` with the existing run/calibration checks, no curve-kind test.
  - `rateDialogText(run, curveDone) -> { title, scope }`: Constant: `New constant rate`, `From now until the end of the run, or the next change.`; otherwise `New hold rate`, `From now until you stop the run, or the next change.`
  - `holdChip(run) -> { value, unit } | null`: non-constant with changes: `{ value: '2.500', unit: 'ml/min' }` labelled `hold` by the caller; else null.
  - `runAgainPicks(run) -> boolean`: true only for a Constant run with changes.
  - `rateSince(run, curveDone)`: unchanged for Constant; non-constant with changes: `holding since 26 h 10` (run time, `atRun`).
- [ ] **Step 1: Write the failing vitest cases** for each function above, including: a report with `resets: [[1800, 1.0]]` and a 1.0 mL gap at 1800 s gives a rebased gap of 0 after it and `resetMl == 1`; a report without `resets` gives today's numbers exactly; `runAgainPicks` false for an exponential with hold changes; `canChangeRate` true for a linear run with `rate_bounds` in its hold.
- [ ] **Step 2: Run** `npm test` from `ui/`: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm test` and `npm run check`: PASS, 0 warnings.
- [ ] **Step 5: Commit** `feat(ui): hold changes for any curve, the gap judged per rate step (logic)`.

### Task 7: the UI wiring, smoke, bundle

**Files:**
- Modify: `ui/src/components/ChangeRate.svelte` (title and scope from `rateDialogText`), `ui/src/routes/Overview.svelte` (`rateSince`, the held value in the hold band and line), `ui/src/routes/History.svelte` (`runAgainPicks` in `runAgain`, the hold chip, numbered change marks on the gap chart, `Written off at rate changes: X mL` in the Regulation fold when `resetMl > 0`), `ui/tests/smoke.mjs`, `ui/dist/`

- [ ] **Step 1: Smoke check first**: replace `no Change rate on a run that is not Constant` by `Change rate only on a Constant run or a run in its hold` (status `rate_bounds` present iff the button is), and on History, a run whose report has `resets` shows the `Written off at rate changes` line when the Regulation fold opens. Read-only, as every smoke check.
- [ ] **Step 2: Wire the components.**
- [ ] **Step 3: Run** from `ui/`: `npm run check`, `npm test`, `npm run build`.
- [ ] **Step 4: Deploy** (CLAUDE.md "Deploy": `/api/status` re-checked right before, no active run; `POST /api/shutdown`; `cargo build --release -p fermentool-core`; start the exe with `FERMENTOOL_NO_BROWSER=1`; `/api/status`; the served bundle contains `New hold rate`), then `npm run smoke`: all checks pass.
- [ ] **Step 5: Commit** `feat(ui): change the hold rate of any curve; the written-off feed in History` and `build(ui): bundle for the hold rate and the per-step gap`.

### Task 8: documentation

**Files:**
- Modify: `CLAUDE.md` ("Rate changes" section: every shape in the hold, `overrides`, the reset rule and its deferral, `reset_g` vs `forgiven_g`, the report's `resets`; "Gravimetric trim" gets one line pointing there), `CHANGELOG.md`, the technical docs' trim section.

- [ ] **Step 1: Write** the sections; `grep -n "—\| - " CLAUDE.md CHANGELOG.md` shows no new hits outside list bullets.
- [ ] **Step 2: Commit** `docs: the hold rate of any curve and the correction reset at a change`.
