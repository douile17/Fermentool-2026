# Change the rate of a constant run: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** an operator changes the rate of a running Constant run; it applies at once, survives a crash, is counted in the report, shown in History, and offered back by Run again.

**Architecture:** the changes are rows (`setpoint_changes`); a pure function turns the run's Constant curve plus its changes into a `Step` curve (the "effective curve"), used wherever the daemon drives or counts (engine, resume, report). `run.curve` stays the curve the run was started with. The UI draws the effective curve the API gives, and reads the changes for the chips, the journal and Run again.

**Tech Stack:** Rust (fermentool-curves, fermentool-core: rusqlite, axum), Svelte 5 + vitest, headless-Edge smoke.

**Spec:** `docs/superpowers/specs/2026-10-22-constant-rate-change-design.md`. Mockup: https://claude.ai/artifact/2JRodCC5X7CbAALsE5Mhau (approved 2026-10-22).

## Global Constraints

- **Nothing that works today may break** (the user's words: "veille à ne pas faire bug les fonctions déjà existantes"). Every task ends with the full suites green: `cargo test -p fermentool-core -p fermentool-curves`, and from `ui/`: `npm run check` (0 errors, 0 warnings), `npm test`. A run with no change must behave, journal, report and display exactly as before.
- `CurveSpec::value_at` stays pure in elapsed time; no clock, no I/O.
- Constant runs only: `CurveParams::Constant`. Dosing runs only, never calibration runs.
- Limits: value finite and > 0, within `limits::FLOW_MIN..=FLOW_MAX` (ml/min) or `RPM_MIN..=RPM_MAX` (rpm); an ml/min run driven in rpm is refused past `limits::RPM_MAX` rpm (same message shape as `start_run`'s).
- UI strings in English, no em dash, no spaced hyphen; logic in `ui/src/lib/` as tested plain functions; after a UI change `npm run build`.
- During an active run, heavy builds at low priority (CLAUDE.md); deploy only with no active run, re-checked right before shutdown.
- Commits without a Co-Authored-By or Claude-Session line; commit only when the user asks.

## Review Focus

1. A change made in the hold phase (past `duration_s`): the setpoint before the change keeps the old value, after it the new one, in `value_at` and in the report's required volume. Test in Task 1 and Task 3.
2. A crash after a change: resume drives the last rate, not the start one. Test in Task 3.
3. The balance regulation across a change: no alarm, `c` does not jump. Test in Task 3.
4. A run with no change: identical report and identical UI (effective curve == run.curve, chips and Run again unchanged). Tests in Tasks 1, 2, 5.
5. A value refused (0, negative, NaN, over the tube's ceiling, a non-constant run, no run): a structured error with a hint, nothing recorded. Tests in Tasks 3 and 4.

---

### Task 1: the effective curve (pure)

**Files:**
- Modify: `crates/fermentool-curves/src/lib.rs` (method on `CurveSpec`, tests in its test module)

**Interfaces:**
- Produces: `impl CurveSpec { pub fn with_rate_changes(&self, changes: &[(f64, f64)]) -> CurveSpec }`, `changes` = `(elapsed_s, value)` in time order. Empty, or `self` not `CurveParams::Constant`: returns `self.clone()`. Otherwise a `CurveParams::Step` spec: `segments` = `[(0, self.start)] + changes`, `start` = `self.start`, `end` = last value, `duration` = `max(self.duration, last change elapsed)`, same `mode`, `clamp_min`, `clamp_max`; it passes `validate()`.

- [ ] **Step 1: Write the failing tests** `rate_changes_make_steps`, `a_change_in_the_hold_counts_from_its_time_only`, `no_change_is_the_same_curve`:
  - constant 5.0 over 24 h, changes `[(22_800, 3.0), (65_100, 4.0)]`: `value_at` at 1 h = 5.0, 7 h = 3.0, 19 h = 4.0, 30 h = 4.0; `validate()` is `Ok`.
  - constant 5.0 over 3 h, change `(14_400, 3.0)`: `value_at` at 3.5 h = 5.0, 4.5 h = 3.0.
  - empty changes, and a linear 1 to 9 with a change: equal to `self`.
- [ ] **Step 2:** `cargo test -p fermentool-curves rate_changes` fails (no method).
- [ ] **Step 3:** implement `with_rate_changes`.
- [ ] **Step 4:** `cargo test -p fermentool-curves` passes, all of it.

### Task 2: the changes in the journal (store)

**Files:**
- Create: `crates/fermentool-core/src/store/migrations/0014_setpoint_changes.sql`
- Modify: `crates/fermentool-core/src/store/mod.rs` (migration list, `RunRow`, `run`, `running_run`, new methods, tests)

**Interfaces:**
- Consumes: `CurveSpec::with_rate_changes` (Task 1).
- Produces:
  - table `setpoint_changes (id INTEGER PRIMARY KEY, run_id INTEGER NOT NULL REFERENCES runs(id) ON DELETE CASCADE, elapsed_s REAL NOT NULL, value REAL NOT NULL, wall_time TEXT NOT NULL)`, index on `run_id`. Deleting a run deletes its changes (check how `delete_run` removes events and do the same if no cascade is active).
  - `#[derive(Debug, Clone, Serialize)] pub struct SetpointChange { pub elapsed_s: f64, pub value: f64, pub wall_time: Timestamp }`
  - `RunRow.setpoint_changes: Vec<SetpointChange>` (serialized; filled by `run()` and `running_run()`, empty from list queries).
  - `impl RunRow { pub fn effective_curve(&self) -> CurveSpec }`
  - `Store::add_setpoint_change(&self, run_id: i64, elapsed_s: f64, value: f64, wall_time: Timestamp) -> Result<()>`, `Store::setpoint_changes(&self, run_id: i64) -> Result<Vec<SetpointChange>>` (ordered by `elapsed_s`, then `id`).

- [ ] **Step 1: Write the failing tests** `setpoint_changes_round_trip` (two changes added, `run(id)` returns them in order, `effective_curve().value_at` reads the second after its time), `a_run_without_changes_has_its_own_curve` (`effective_curve() == curve`), `deleting_a_run_deletes_its_changes`.
- [ ] **Step 2:** they fail.
- [ ] **Step 3:** migration, struct, methods, fill in `run()` / `running_run()`.
- [ ] **Step 4:** `cargo test -p fermentool-core` passes, all of it.

### Task 3: the engine applies, resumes and counts the changes

**Files:**
- Modify: `crates/fermentool-core/src/engine/mod.rs` (new method, `EngineError` variants and `hint()`, `ActiveStatus`, resume, recovery info, `tracking_report`, tests)

**Interfaces:**
- Consumes: Task 2's store methods and `RunRow::effective_curve`.
- Produces:
  - `pub fn set_constant_rate(&mut self, value: f64, elapsed_s: f64, now: Timestamp) -> Result<(f64, f64)>` returning `(from, to)`; refusals: `EngineError::Idle` (no run), `EngineError::NotConstant` (new: curve not Constant or a calibration run; `Display` one line, `hint()` the why), `EngineError::Config` for a value out of bounds (message names the bounds).
  - On success: row added, `active.spec` = the base curve `with_rate_changes(all changes)`, event `setpoint_change` (Info) with detail `"{from:.3} -> {to:.3} {unit}"` (unit `ml/min` or `rpm`), the next `apply_setpoint` writes it; `trim_c` and the tracker are left as they are.
  - `ActiveStatus.rate_changes: usize` (count of changes) and `ActiveStatus.rate_bounds: Option<[f64; 2]>` (min, max the pump can be asked for this run: the control var's limits, the max lowered to `RPM_MAX * r` when driven in rpm; `None` when not a Constant dosing run).
  - Everywhere a stored run's curve drives or counts, `run.effective_curve()` instead of `run.curve`: `Engine::resume` (target and `ActiveRun.spec`), the recovery info's `resume_target`, `tracking_report`'s required volume. `start_run` unchanged.

- [ ] **Step 1: Write the failing tests**
  - `a_constant_rate_changes_at_once`: a Constant 5.0 ml/min dosing run, change to 3.0 at 600 s: the pump's written setpoint is 3.0 (times c) at the next ticks, a `setpoint_change` event reads `5.000 -> 3.000 ml/min`, `status().active.rate_changes == 1`.
  - `refusals_record_nothing`: 0.0, -1.0, NaN, above the bound, a Linear run, no run: each refused with its variant, no row, no event.
  - `a_crash_after_a_change_resumes_on_it`: change to 3.0, then a fresh engine on the same store resumes: target 3.0, not 5.0.
  - `the_report_counts_each_rate_for_its_time`: Constant 6.0 ml/min, change to 3.0 at 1800 s, ticks to 3600 s: the report's last required ≈ 6×30 + 3×30 mL (±1 %).
  - `a_change_in_the_hold_counts_from_its_time`: duration 1800 s, change at 2400 s, required at 3000 s ≈ 6×40 + 3×10 mL.
  - `the_regulation_rides_a_change`: with the trimmed bench used by `after_a_refill_the_regulation_resumes_without_a_catch_up_burst` (a scale fed by the commanded flow), change mid-run: no `alarm_*` event, `trim_c` within 0.5 % across the change.
- [ ] **Step 2:** they fail.
- [ ] **Step 3:** implement.
- [ ] **Step 4:** `cargo test -p fermentool-core` passes, all of it (the existing resume, report, refill and trim tests included).

### Task 4: the command, the API, the notification

**Files:**
- Modify: `crates/fermentool-core/src/control.rs` (`Command::SetConstantRate`), `crates/fermentool-core/src/api.rs` (route, run detail field, tests), `crates/fermentool-core/src/notify.rs` (message, test)

**Interfaces:**
- Consumes: `Engine::set_constant_rate`, `run_now()` for `elapsed_s`.
- Produces:
  - `POST /api/runs/active/rate` body `{ "value": f64 }` → `200 { "from": f64, "to": f64 }`; refusals as the other engine errors (`{ message, code, hint }`, status 409 / 422 as `start_run`'s mapping does).
  - `GET /api/runs/:id` gains `effective_curve` (= `run.effective_curve()`), next to `curve` and `setpoint_changes`.
  - `notify::message_for`: `setpoint_change` → `Info`, title `"Rate changed to {to} {unit}"` read from the detail.
- [ ] **Step 1: Write the failing tests** `the_rate_of_a_constant_run_changes_through_the_api` (start a Constant simulator run, POST 3.0, 200 with from 5.0 / to 3.0, `GET /api/runs/:id` has one `setpoint_changes` and an `effective_curve` of kind step), `a_rate_change_without_a_run_is_refused`, `a_rate_change_is_one_info_message`.
- [ ] **Step 2:** fail. **Step 3:** implement. **Step 4:** `cargo test -p fermentool-core` all green.

### Task 5: the UI

**Files:**
- Create: `ui/src/lib/ratechange.js`, `ui/src/lib/ratechange.test.js`, `ui/src/components/ChangeRate.svelte`
- Modify: `ui/src/routes/Overview.svelte`, `ui/src/routes/History.svelte`, `ui/src/lib/rundetail.js` (`curveChips`, `journalLines` titles), `ui/src/lib/rundetail.test.js`, `ui/tests/smoke.mjs`

**Interfaces:**
- Consumes: Task 4's API fields.
- Produces (in `ratechange.js`):
  - `rateSteps(run) -> [{ value, wallMs, atStart: bool, last: bool }]`: the start rate (wall time = `started_at`) then each change; one entry when no change.
  - `rateLabel(step, unit) -> string`: e.g. `"Mon 20:30"` for the day and time, via the app's locale helpers.
  - `canChangeRate(status, run) -> bool`: a dosing run, curve kind `constant`, active.
  - `checkRate(value, bounds) -> null | string`: the refusal text shown under the field, or null.
- [ ] **Step 1: Write the failing tests** for the four functions (run 214 of the mockup: 5.000 at Mon 14:10, 3.000 Mon 20:30, 4.000 Tue 08:15; `last` on the 4.000), `curveChips` with changes gives `"5.000 ml/min, then 3.000, 4.000"` and without changes exactly what it gives today, `journalLines` titles `setpoint_change` as `Rate changed` with `5.000 → 3.000 ml/min`.
- [ ] **Step 2:** fail. **Step 3:** implement as in the mockup:
  - Overview: under the feed rate, `constant since the start` or `since Mon 20:30`; a `Change rate` ghost button (sliders icon) when `canChangeRate`; `ChangeRate.svelte` dialog (current rate, number input in the run's unit, bounds line from `status.active.rate_bounds`, the sentence, primary `Change to X unit`, Cancel; Escape closes); the planned curve from `run.effective_curve`, reloaded when `status.active.rate_changes` changes.
  - History: planned from `effective_curve`; a numbered marker per change with its line under the chart (`1 at 6 h 20, rate changed to 3.000 ml/min`); chips from `curveChips`; Run again: with more than one step, the `Start from which rate?` dialog (radio list, the last preselected, primary `Open New run at X unit`), the chosen value put in the prefill's constant curve; with one step, Run again as today.
- [ ] **Step 4:** from `ui/`: `npm run check`, `npm test`, `npm run build` all green. Smoke: a check that the Overview's `Change rate` is absent when no run or a non-constant run is shown (the smoke changes nothing), and that the History detail of a run without changes shows no rate dialog on Run again (it lands on New run as before).

### Task 6: deploy, verify for real, document

- [ ] Re-check `/api/status`: no active run. Shut down, `cargo build --release`, start, bundle served.
- [ ] `npm run smoke` all green.
- [ ] On the simulator (`allow_simulator` is off on this PC: run the API tests' simulator path, or ask the user before turning it on), or with the user's go on the real pump: a short Constant run, change the rate, check the Overview, History, Run again; stop it. Do not start a run on the real pump without the user's go.
- [ ] CHANGELOG, session notes, CLAUDE.md (a "Rate change" paragraph: table, effective curve, where it is used, Constant only).
- [ ] Ask the user to commit and push.
