# Camera, plan A: timing baseline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The daemon logs, once a minute, how late the 150 ms setpoint writes are, how long the 1 s journal tick takes, and how old the balance reading is, so the camera's effect can be measured against a baseline taken without it.

**Architecture:** A small `timing` module holds one sample series per metric behind a global mutex; the control loop and the balance poll thread record into it; the control loop flushes a per-minute summary (count, p50, p99, max) to the log under the `timing` target. A Python script reads two time ranges of the log and compares them. No camera code in this plan.

**Tech Stack:** Rust (fermentool-core), `tracing`, Python 3 (stdlib only) for the report.

**Spec:** `docs/superpowers/specs/2026-11-02-camera-timelapse-design.md`, section "Done means", first point.

Plans that follow: B (camera helper, live view, Settings > Camera), C (timelapses, player, New run, Camera > Timelapses), D (History, exports, deletion, installer).

## Global Constraints

- Recording a sample must never block or slow the control thread: a mutex held for a push only, never across I/O; the log write happens at most once a minute.
- Metrics: `setpoint_late` (ms the 150 ms write ran after its due instant), `tick` (ms spent in `Engine::tick`), `scale_age` (ms since the balance poll thread got the reading the engine just used), `scale_poll` (ms one SICS transaction took on the `scale-poll` thread).
- Summary window: 60 s. Percentiles: nearest rank.
- No em dash, no spaced hyphen in code comments, logs, docs, commit messages (`CLAUDE.md`).
- Commit messages: no `Co-Authored-By: Claude`, no `Claude-Session` (`CLAUDE.md` overrides the harness default).
- During an active run: builds and tests at low priority (`start "" /b /low /wait cargo ...`), from the VS Community environment (`CLAUDE.md`, "Working on this PC").

## Review Focus

- A minute with no run (no setpoint writes, no ticks): nothing logged for those metrics, no division by zero, no empty line.
- A paused or stuck balance thread: `scale_age` keeps being recorded as it grows (the age is what the engine saw), and stays readable as the stuck-thread signal.
- A clock step mid-run (`run_now` guard): `setpoint_late` is measured on `Instant`, never on wall time, so a step does not read as lateness.
- Very long runs: the series are cleared each window, memory stays bounded (~60 s × 7 writes/s).
- The log line format is stable: the report script parses it; a format change breaks the comparison silently.

---

### Task 1: The `timing` module

**Files:**
- Create: `crates/fermentool-core/src/timing.rs`
- Modify: `crates/fermentool-core/src/lib.rs` (add `pub mod timing;` and the doc line)

**Interfaces:**
- Produces:
  - `pub enum Metric { SetpointLate, Tick, ScaleAge, ScalePoll }` with `fn name(self) -> &'static str` returning `"setpoint_late"`, `"tick"`, `"scale_age"`, `"scale_poll"`.
  - `pub struct Summary { pub count: usize, pub p50: f64, pub p99: f64, pub max: f64 }`
  - `pub fn summarize(samples: &[f64]) -> Option<Summary>` (pure)
  - `pub struct Timing` with `fn new(now: Instant) -> Timing`, `fn record(&mut self, m: Metric, ms: f64)`, `fn take_if_due(&mut self, now: Instant) -> Option<Vec<(Metric, Summary)>>` (returns and clears once 60 s have passed since the window start; only metrics with samples; window restarts at `now`).
  - `pub fn record(m: Metric, ms: f64)` and `pub fn flush_if_due(now: Instant)` on a global `OnceLock<Mutex<Timing>>`; `flush_if_due` logs each summary as `tracing::info!(target: "timing", metric = m.name(), count, p50_ms, p99_ms, max_ms)` with the three values rounded to 0.1 ms.

- [ ] **Step 1: Write the failing tests** (in `timing.rs`, `#[cfg(test)]`)

```rust
#[test]
fn summarize_uses_nearest_rank() {
    let s: Vec<f64> = (1..=100).map(f64::from).collect();
    let r = summarize(&s).unwrap();
    assert_eq!((r.count, r.p50, r.p99, r.max), (100, 50.0, 99.0, 100.0));
}

#[test]
fn summarize_of_nothing_is_none() {
    assert!(summarize(&[]).is_none());
}

#[test]
fn take_if_due_waits_a_minute_then_clears() {
    let t0 = Instant::now();
    let mut t = Timing::new(t0);
    t.record(Metric::Tick, 2.0);
    assert!(t.take_if_due(t0 + Duration::from_secs(59)).is_none());
    let out = t.take_if_due(t0 + Duration::from_secs(60)).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0.name(), "tick");
    // cleared, window restarted at 60 s
    assert!(t.take_if_due(t0 + Duration::from_secs(121)).is_none());
}

#[test]
fn a_quiet_minute_reports_nothing() {
    let t0 = Instant::now();
    let mut t = Timing::new(t0);
    assert!(t.take_if_due(t0 + Duration::from_secs(61)).map_or(true, |v| v.is_empty()));
}
```

- [ ] **Step 2: Run them, expect a compile failure**

Run: `cargo test -p fermentool-core timing::`
Expected: FAIL, `summarize` / `Timing` not defined.

- [ ] **Step 3: Implement the module** as in Interfaces. Nearest rank: sort a copy, index `ceil(p × n) - 1`. `take_if_due` returns `Some(vec![])` or `None` for a quiet minute, either is fine for the test above; `flush_if_due` logs nothing in that case.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p fermentool-core timing::`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/fermentool-core/src/timing.rs crates/fermentool-core/src/lib.rs
git commit -m "feat(timing): per-minute summaries of loop timing"
```

### Task 2: Record from the control loop and the balance, flush, report script

**Files:**
- Modify: `crates/fermentool-core/src/control.rs` (the write branch near `if Instant::now() >= write_due`, the tick branch around `engine.tick(now)`, the top of the loop pass)
- Modify: `crates/fermentool-core/src/scale.rs` (`PolledScale::spawn` worker loop, `PolledScale::transaction`)
- Create: `scripts/timing_report.py`
- Modify: `CLAUDE.md` (one paragraph under "Serial I/O and the control thread": what the `timing` lines are and how to compare)

**Interfaces:**
- Consumes: `timing::record(Metric, f64)`, `timing::flush_if_due(Instant)` (Task 1).
- Produces: log lines `timing: metric="..." count=.. p50_ms=.. p99_ms=.. max_ms=..` in `%APPDATA%\Fermentool\logs\`; `python scripts/timing_report.py <log> --a FROM TO --b FROM TO` printing, per metric, the median over the minutes of each range of p50, p99 and max, and the B/A ratio of p99.

- [ ] **Step 1: Write the failing test for the balance age** (in `scale.rs` tests, next to the stale-age test that uses `SCALE_MAX_AGE`)

```rust
#[test]
fn a_served_reading_records_its_age() {
    // A PolledScale whose latest reading is 300 ms old: transaction() serves
    // it and records scale_age >= 300 into the timing module.
}
```

Assert through a test-only accessor `timing::peek(Metric::ScaleAge) -> Vec<f64>` (add it to `timing.rs` under `#[cfg(test)]`); the value is `>= 300.0`. Use the struct-literal construction the existing stale-age test uses.

- [ ] **Step 2: Run it, expect FAIL** (`cargo test -p fermentool-core scale::`): nothing recorded.

- [ ] **Step 3: Record the four metrics**
  - `scale.rs`: in `transaction`, the served reading's `at.elapsed()` as `ScaleAge`; in the poll loop, the duration of `port.transaction(SICS_IMMEDIATE)` as `ScalePoll`.
  - `control.rs`: in the write branch, before `apply_setpoint`, `Instant::now().saturating_duration_since(write_due)` as `SetpointLate`; around `engine.tick(now)`, its duration as `Tick`; at the start of each loop pass, `timing::flush_if_due(Instant::now())`.

- [ ] **Step 4: Run the whole crate's tests**

Run: `cargo test -p fermentool-core`
Expected: all pass, including `tick_cadence_is_independent_of_command_traffic`.

- [ ] **Step 5: Write `scripts/timing_report.py`**: stdlib only (`argparse`, `re`, `statistics`, `datetime`); parses the `tracing` file lines with `timing` target; `--a`/`--b` take ISO local times. Check it by hand on the deployed log after Step 6.

- [ ] **Step 6: Deploy and look** (per `CLAUDE.md` "Deploy": re-check `/api/status` for an active run right before stopping anything; low priority if a run is active)

Run: `curl 127.0.0.1:8730/api/status`, then after a release build and restart, wait two minutes and search the newest log for `timing`.
Expected: `setpoint_late` and `tick` lines only during a run; `scale_age` / `scale_poll` lines whenever a balance is configured.

- [ ] **Step 7: Commit**

```bash
git add crates/fermentool-core/src/control.rs crates/fermentool-core/src/scale.rs crates/fermentool-core/src/timing.rs scripts/timing_report.py CLAUDE.md
git commit -m "feat(timing): record setpoint lateness, tick and balance age; report script"
```

- [ ] **Step 8: Record the baseline.** One 6 h run on the bench (simulator pump allowed by `serial.allow_simulator`, real balance, no camera); write its start and end times and the report's output into the session notes (`docs/notes-de-session-*.md`). This is range A of the comparison in plan B's last task. Ask the user before taking the bench.
