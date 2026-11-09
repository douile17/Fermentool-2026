# Pump Links (each pump's own state on the shared cable) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** At any time, the operator sees which pump answers on the RS-485 cable (feed, refill) from a real question to that pump, never inferred from the other one, and a silent pump costs the bus a short, bounded wait instead of seconds.

**Architecture:** `fermentool-modbus` gains a per-call deadline (`Transport::transaction_within`) and a one-shot `Pump::probe`; the watchdog forwards it. The engine keeps one `PumpLink` per pump (last answer, when) plus whether the port itself is open, fed by every frame to that pump and by scheduled probes; the status frame carries them as `pumps`. The control loop probes the refill pump on its own cadence (idle and in a run), whatever the feed pump does. The UI reads `pumps` (no more refill ping loop) and names the silent pump when the other answers.

**Tech Stack:** Rust (`fermentool-modbus`, `fermentool-core`, axum), Svelte 5, vitest, headless-Edge smoke test.

**Spec:** this plan's Context section (from the 2026-10-23 bench discussion), and CLAUDE.md "RS-485 bus: one pump switched off silences the other".

## Context (the decisions)

Bench findings, 2026-10-23:
- Pump 2 switched off but plugged in: the whole bus is silent (its unpowered transceiver loads A/B). Pump 2 unplugged: pump 1 answers. Plug 2 on pump 1 works. A hardware fix (isolated RS-485 hub) is planned separately; this plan is software only and must be right with and without the hub.
- Today the code reads "feed pump silent" as "cable dead": `refill_line_up()` (`engine/refill.rs:143`) and `lineUp()` (`ui/src/lib/pumps.js:6`) stop asking the refill pump, and the UI shows it disconnected without having asked. Connect only asks the feed address.
- The refill pump is asked only by `POST /api/refill/ping` from Settings (every 15 s) and by a run that drives it. Its last answer is shown green with no age.
- **Cost of a silent pump today:** `SerialTransport` is opened with a 1.5 s whole-transaction deadline (`transport.rs` `OPEN_TIMEOUT`; the 200 ms port timeout only bounds each `read()` inside `read_until`'s loop), and `transact_retrying` sends twice: one question to a silent pump holds the bus about 3.2 s (2 x (1.5 s + 100 ms gap)).

Decisions:
- Each pump's state is `answers` / `silent` / `unknown`, set only by a frame sent to that address. `unknown`: never asked since boot or since its address changed, or the port is not open. Never derived from the other pump.
- Probes (a read of the speed register) use a short deadline `PROBE_WITHIN` and **no retry**. Commands (setpoint, start, stop, direction) keep today's 1.5 s and retry: a slow pump must never lose a command.
- Cadence: feed pump unchanged (1 s idle probe, every setpoint write in a run). Refill pump: every 3 s outside a run; every 15 s during a run that does not drive it, right after a setpoint write; during a run that drives it, its own frames (Stop reassert, fills) are enough. Never more than one probe between two setpoint writes.
- `serial_ok` keeps its meaning (the feed pump answers: runs are refused without it); `pumps.port_open` tells the port apart from the pumps.
- Colour only when a state needs an eye (docs/DESIGN.md): a silent refill pump is red only when the active run drives it, grey otherwise (a bench without a refill pump must not show a permanent alarm).

Order with `docs/superpowers/plans/2026-10-23-pump-models.md`: independent. Whichever lands second adapts: that plan's `with_target_of` replaces `with_address` in Task 3/4 here, and its `RefillStatus` fields (model, head) can be added to `PumpLinkStatus`.

## Global Constraints

- `PROBE_WITHIN` default 300 ms until Task 0 measures the real LabQ; then `clamp(4 x p99, 150 ms, 400 ms)`, rounded up to 50 ms. Commands keep `OPEN_TIMEOUT` (1.5 s) and the single retry.
- Cadences: `REFILL_PROBE_IDLE = 3 s`, `REFILL_PROBE_IN_RUN = 15 s`. `LINK_PROBE_INTERVAL` (1 s, feed) and `WRITE_SETPOINT_INTERVAL` (150 ms) unchanged.
- Never write the pump's head / tubing registers; a probe is a read of the speed register only (as `read_speed_rpm` today), never a Start.
- Every blocking call from the control thread stays behind `WatchdogTransport` (`HARD_TXN_TIMEOUT` 2 s unchanged).
- UI strings in English, "measurement" never "burst"; no em dash and no spaced hyphen as one anywhere (code, comments, commits).
- Commit messages: no `Co-Authored-By` trailer, no `Claude-Session` line (project rule).
- Build from the VS Community environment: `cmd /c '"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" && cargo ...'`. During an active run, heavy work at low priority (`start "" /b /low /wait ...`). Retry once on `STATUS_HEAP_CORRUPTION`.
- After any change under `ui/src/`: from `ui/`, `npm run check` (0 errors, 0 warnings), `npm test`, `npm run build`. A UI change starts with a mockup on a separate page (docs/DESIGN.md) and is implemented on the user's go.

## Review Focus

1. Feed pump unplugged, refill pump plugged, no run: status says feed `silent`, refill `answers`, within 3 s, and the UI names the feed pump. Test in Task 3 (`refill_is_asked_while_the_feed_pump_is_silent`) and Task 5 (`names the silent pump when the other answers`).
2. Bus silent (today's "one switched off silences both"): both `silent`, the UI blames neither and keeps the cabling hint. Test in Task 5 (`both silent blames neither`).
3. A slow but alive LabQ read as silent by a too short probe: Task 0 measures it, and commands never use the probe deadline. Test in Task 1 (`commands keep the full deadline and the retry`).
4. A run with the refill pump switched off: setpoint writes are delayed at most one `PROBE_WITHIN` per `REFILL_PROBE_IN_RUN`. Test in Task 3 (`refill_probe_due` cases) and Task 4 (`a_silent_refill_pump_costs_one_probe_per_15_s_in_a_run`).
5. USB adapter pulled (port will not open): both pumps `unknown`, never `silent`, and the UI speaks of the adapter / port, not of the pumps. Test in Task 3 (`a_closed_port_makes_both_unknown`) and Task 5.

---

### Task 0: Measure the LabQ's reply time (bench, with the user)

**Files:**
- Create: `crates/fermentool-modbus/examples/latency.rs`

- [ ] **Step 1: Write the example.** Args: `<port> <address> [count=200]`. Opens `SerialTransport::open(port, 9600, 1500 ms)`, sends `count` reads of the speed register (`frame` builders already used by `read_speed_rpm`), times each round trip from write to last byte (excluding the inter-frame gap sleep), prints `n ok / n failed, min, median, p99, max` in ms. Read only, no write frame.
- [ ] **Step 2: Run it with the user**, daemon stopped (it holds COM8; check `/api/status` has no active run right before `POST /api/shutdown`): address 1, then address 2. `cargo run -p fermentool-modbus --example latency -- COM8 1`.
- [ ] **Step 3: Record** the figures and the chosen `PROBE_WITHIN` in `docs/notes-de-session-<date>.md`; restart the daemon (`FERMENTOOL_NO_BROWSER=1`, release exe).
- [ ] **Step 4: Commit** `chore(modbus): latency example to measure a pump's reply time`

### Task 1: A per-call deadline and a one-shot probe (fermentool-modbus)

**Files:**
- Modify: `crates/fermentool-modbus/src/lib.rs` (`Transport`, `impl Transport for Box<dyn Transport + Send>`, `SerialTransport`, `Pump`)
- Test: same file, `mod tests`

**Interfaces:**
- Produces:
  - `Transport::transaction_within(&mut self, request: &[u8], within: Duration) -> Result<Vec<u8>, TransportError>`, default method calling `transaction` (simulators unchanged). `Box<dyn Transport + Send>` forwards it.
  - `SerialTransport` overrides it: deadline `now + within.min(self.timeout)`, same inter-frame gap bookkeeping as `transaction`.
  - `Pump::probe(&mut self, within: Duration) -> Result<f32, PumpError>`: one `transaction_within` of the speed-register read, no retry, parsed as `read_speed_rpm` does.

- [ ] **Step 1: Failing tests** (a `Recording` transport in `mod tests` that stores each call's `Option<Duration>` and fails on demand)

```rust
#[test]
fn probe_asks_once_with_its_deadline() {
    // failing transport: probe(250 ms) is Err, exactly 1 call, with Some(250 ms)
}

#[test]
fn commands_keep_the_full_deadline_and_the_retry() {
    // failing transport: read_speed_rpm and set_speed_rpm(10.0) each make 2 calls, both with None
}

#[test]
fn probe_reads_the_speed_register() {
    // SimPump at 1 running 42 rpm: Pump::new(sim, 1).probe(250 ms) == Ok(42.0)
}
```

- [ ] **Step 2: Run, expect failures:** `cargo test -p fermentool-modbus --no-default-features probe` (and `commands_keep`): FAIL, `probe` not defined.
- [ ] **Step 3: Implement** the three items above.
- [ ] **Step 4: Run** `cargo test -p fermentool-modbus` and `--no-default-features`: PASS.
- [ ] **Step 5: Commit** `feat(modbus): a per-call deadline and a one-shot probe`

### Task 2: The watchdog forwards the deadline

**Files:**
- Modify: `crates/fermentool-core/src/transport.rs` (`Job::Txn`, worker loop, `impl Transport for WatchdogTransport`)
- Test: same file

**Interfaces:**
- Consumes: Task 1 `transaction_within`.
- Produces: `WatchdogTransport::transaction_within` runs the inner `transaction_within` on the worker. `Job::Txn(Vec<u8>, Option<Duration>, Sender<..>)`; `None` calls `transaction`. The control thread still waits at most `HARD_TXN_TIMEOUT`.

- [ ] **Step 1: Failing test** `watchdog_forwards_the_probe_deadline`: a recording inner transport; `transaction_within(req, 250 ms)` reaches it with `Some(250 ms)`, `transaction(req)` with `None`.
- [ ] **Step 2: Run** `cargo test -p fermentool-core watchdog`: FAIL.
- [ ] **Step 3: Implement.** The existing stuck-worker tests must still pass unchanged.
- [ ] **Step 4: Run** `cargo test -p fermentool-core transport`: PASS.
- [ ] **Step 5: Commit** `feat(transport): the watchdog forwards a probe's deadline`

### Task 3: One link per pump in the engine

**Files:**
- Create: `crates/fermentool-core/src/engine/links.rs`
- Modify: `crates/fermentool-core/src/engine/mod.rs` (fields, `probe_link`, `confirm_read`, setpoint write results, `recover_serial`, `swap_transport`, `status`, `EngineStatus`), `crates/fermentool-core/src/engine/refill.rs` (`refill_answered` stamps the link; `set_refill` resets it on a moved address; `refill_line_up` becomes `self.port_open`)
- Test: `links.rs` and `mod.rs` tests

**Interfaces:**
- Consumes: Task 1 `Pump::probe`, Task 2.
- Produces:
  - `pub const PROBE_WITHIN: Duration` (Global Constraints value), `pub const REFILL_PROBE_IDLE: Duration = 3 s`, `pub const REFILL_PROBE_IN_RUN: Duration = 15 s`.
  - `#[derive(Debug, Default)] pub(super) struct PumpLink { responding: Option<bool>, last_answer: Option<Instant> }` with `fn answered(&mut self, ok: bool, now: Instant)`, `fn forget(&mut self)` (back to unknown), `fn status(&self, address: u8, port_open: bool, now: Instant) -> PumpLinkStatus`.
  - `#[derive(Debug, Clone, Serialize)] pub struct PumpLinkStatus { pub address: u8, pub state: &'static str /* "answers" | "silent" | "unknown" */, pub answered_s_ago: Option<f64> }`: `unknown` whenever `!port_open`.
  - `#[derive(Debug, Clone, Serialize)] pub struct PumpsStatus { pub port_open: bool, pub feed: PumpLinkStatus, pub refill: PumpLinkStatus }`; `EngineStatus` gains `pub pumps: PumpsStatus` (serialized as `pumps`, only on a real port; on the simulator `port_open: false` and both `unknown`).
  - `pub fn refill_probe_due(last: Option<Instant>, now: Instant, run: RefillUse) -> bool` with `pub enum RefillUse { Idle, RunNotDriving, RunDriving }`: `Idle` 3 s, `RunNotDriving` 15 s, `RunDriving` never.
  - Engine: `port_open: bool` (true once a real port opened, false on `DeadLink` / swap to the simulator), `feed_link: PumpLink`, `refill_link: PumpLink`. `Engine::probe_refill(&mut self, now: Instant) -> bool`: `with_address(refill addr, |p| p.probe(PROBE_WITHIN))`, then `refill_answered`; does nothing (returns false, link `unknown`) when `!port_open` or the address equals the feed's. `Engine::refill_use(&self) -> RefillUse`.
  - `confirm_read` while `serial_lost` uses `probe(PROBE_WITHIN)` (a recovery pass on a silent feed pump holds the thread ~0.4 s instead of ~3.2 s); the two-read confirm of a merely degrading link keeps `read_speed_rpm`.

- [ ] **Step 1: Failing tests**

```rust
#[test] fn refill_probe_due_cases() { /* Idle: None -> true, 2.9 s -> false, 3 s -> true; RunNotDriving: 14 s false, 15 s true; RunDriving: always false */ }
#[test] fn refill_is_asked_while_the_feed_pump_is_silent() { /* SimBus, address 1 unplugged: probe_link fails, probe_refill(now) == true; status().pumps: feed "silent", refill "answers" */ }
#[test] fn a_closed_port_makes_both_unknown() { /* port_open false: both "unknown", probe_refill sends no frame */ }
#[test] fn a_moved_refill_address_is_unknown_until_asked() { /* set_refill to 3: refill "unknown", answered_s_ago None */ }
#[test] fn answered_s_ago_counts_from_the_last_answer() { /* answered(true, t0); status at t0 + 4 s: Some(4.0); a later failure keeps last_answer, state "silent" */ }
```

- [ ] **Step 2: Run** `cargo test -p fermentool-core links` and `refill_is_asked`: FAIL.
- [ ] **Step 3: Implement.** Every feed frame outcome (probe, confirm, setpoint write, readback) calls `feed_link.answered`; every refill frame outcome (Stop reassert, Test, fill frames, probe) goes through `refill_answered`, which stamps `refill_link`.
- [ ] **Step 4: Run** the full `cargo test`: PASS (existing refill and recovery tests unchanged, except those asserting the refill is not asked with the feed silent: update them to the new rule and say so in the commit).
- [ ] **Step 5: Commit** `feat(engine): each pump's own link state, never inferred from the other`

### Task 4: The control loop asks the refill pump, the ping endpoint goes

**Files:**
- Modify: `crates/fermentool-core/src/control.rs` (scheduling, `Command::RefillPing` removed), `crates/fermentool-core/src/api.rs` (`/api/refill/ping` route and handler removed; `POST /api/serial/reconnect` asks both pumps before answering)
- Test: `control.rs` tests

**Interfaces:**
- Consumes: Task 3 `refill_probe_due`, `Engine::probe_refill`, `Engine::refill_use`, `REFILL_PROBE_*`.
- Produces: `next_refill_probe: Option<Instant>` in the loop. Idle branch: probe when due, independent of `serial_lost`. Run branch: only right after a setpoint write that returned, at most one probe before the next write; skipped while a fill or a Test runs. The idle wake-up (`[next_probe, next_scale_probe, engine.refill_next_due()]`) includes it. Reconnect replies once `probe_link` and `probe_refill` both ran.

- [ ] **Step 1: Failing test** `a_silent_refill_pump_costs_one_probe_per_15_s_in_a_run`: engine on a SimBus with address 2 unplugged, a run not driving the refill pump, the loop's run branch driven over 60 s of simulated writes: frames to address 2 counted, between 4 and 5; no two consecutive setpoint writes have more than one frame to address 2 between them.
- [ ] **Step 2: Run** `cargo test -p fermentool-core control`: FAIL.
- [ ] **Step 3: Implement.** Remove `RefillPing` and the route; `grep -rn "refill/ping\|RefillPing"` over `crates/` and `ui/` is empty after Task 5.
- [ ] **Step 4: Run** the full `cargo test`: PASS.
- [ ] **Step 5: Commit** `feat(control): the refill pump is asked on its own cadence, whatever the feed pump does`

### Task 5: UI logic reads `pumps` (`ui/src/lib`)

**Files:**
- Modify: `ui/src/lib/pumps.js`, `ui/src/lib/pumps.test.js`, `ui/src/lib/link.js`, `ui/src/lib/link.test.js`, `ui/src/routes/settings/PumpsSettings.svelte` (drop the ping loop, read `s.pumps`)

**Interfaces:**
- Consumes: status `pumps: { port_open, feed: { address, state, answered_s_ago }, refill: {...} }`.
- Produces:
  - `pumpTone(link, { needed })`: `''` answers, `'bad'` silent and needed, `'idle'` silent and not needed or unknown. `needed`: feed always; refill when `s.refill?.in_run`.
  - `pumpDetail(link)`: `"Answers"`, `"Not answering, last answer 2 min ago"`, `"Not answering since start"`, `"Not asked yet"` (ages through the existing `fmt.js` duration helper).
  - `linkState(status, connected)`: `!pumps.port_open` on a real port: "Port not open" (adapter / port hint, as today's "will not open"); both silent: today's "Pumps not answering" and its cabling hint; feed silent, refill answers: "Feed pump not answering", hint `Address ${feed} on ${port}: the refill pump answers, so the cable is fine. Check the feed pump's power, its plug and its address.`; feed answers, refill silent and needed: today's "Refill pump not answering".
  - `lineState` counts `answering` from `pumps` states.
  - Removed: `pingWanted`, `pingDelayMs`, `feedCameBack`, `feedResponding` (and their tests).

- [ ] **Step 1: Failing vitest cases** in `pumps.test.js` / `link.test.js`: `names the silent pump when the other answers`, `both silent blames neither`, `a closed port is the port, not the pumps`, `a silent refill pump is grey unless the run drives it`, `pumpDetail ages`.
- [ ] **Step 2: Run** `npm test` from `ui/`: FAIL.
- [ ] **Step 3: Implement**, and `PumpsSettings.svelte` reads `pumpTone` / `pumpDetail` per row (layout unchanged in this task).
- [ ] **Step 4: Run** `npm run check`, `npm test`, `npm run build`: PASS.
- [ ] **Step 5: Commit (with `ui/dist`)** `feat(ui): each pump's state from the daemon, the silent one named`

### Task 6: Always in sight (mockup first)

**Files:**
- Create: a mockup page (Artifact, per docs/DESIGN.md)
- Modify (after the user's go): `ui/src/App.svelte` (`.rail-foot`), `ui/src/routes/settings/PumpsSettings.svelte` (each row's detail line), `ui/tests/smoke.mjs`

- [ ] **Step 1: Mockup**: the rail foot with two compact rows "Feed" and "Refill", each a `StateIcon` and its word (Answers / Not answering / Not asked), colour per `pumpTone`; Settings > Pumps rows with `pumpDetail` under the address. Light and dark, the five states of Review Focus as frames. Wait for the user's choice.
- [ ] **Step 2: Smoke checks first** (looking only): the rail foot shows a Feed and a Refill state; Settings > Pumps shows a detail line per pump; no request to `/api/refill/ping` is made (`performance.getEntriesByType('resource')`).
- [ ] **Step 3: Implement the chosen mockup.**
- [ ] **Step 4: `npm run check`, `npm test`, `npm run build` pass.**
- [ ] **Step 5: Commit (with `ui/dist`)** `feat(ui): both pumps' state always in sight`

### Task 7: Docs, deploy, bench check

**Files:**
- Modify: `CLAUDE.md` (RS-485 section: the daemon now asks each pump itself; the probe deadline and cadences; `pumps` in the status frame; the 3.2 s cost before this change), `CHANGELOG.md`, `docs/notes-de-session-2026-10-21.md` section 8 (item closed)

- [ ] **Step 1: Write the docs.**
- [ ] **Step 2: Full verification:** `cargo test`; from `ui/`: `npm run check`, `npm test`, `npm run build`.
- [ ] **Step 3: Deploy per CLAUDE.md** (re-check `/api/status` for an active run right before; `POST /api/shutdown`, `cargo build --release`, start the exe with `FERMENTOOL_NO_BROWSER=1`, `/api/status` has `pumps`), then `npm run smoke`. Tell the user to Ctrl+F5.
- [ ] **Step 4: Bench check with the user**, no run: (a) both on: both "Answers"; (b) pump 1 unplugged: "Feed pump not answering", refill "Answers" within 3 s; (c) pump 2 unplugged: refill "Not answering", grey; (d) pump 2 switched off, plugged: both silent, "Pumps not answering"; (e) USB adapter pulled: "Port not open". Note the results in the session notes.
- [ ] **Step 5: Commit** `docs: each pump's own link state on the shared cable`
