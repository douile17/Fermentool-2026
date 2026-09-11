# Gravimetric feed trim (balance-corrected feed pump)

**Date:** 2026-09-11
**Status:** approved, Step 0 spike confirmed (SICS), ready for implementation plan

## Problem

Fermentool drives the feed pump open loop: `Engine::tick` writes
`curve.value_at(elapsed)` and never questions whether the pump actually
delivered it. Over a ~100 h run the real flow drifts from the commanded one
(peristaltic tube wear, head/tubing calibration, tube relaxation), and nothing
in the daemon notices.

The operator has: a scale under the feed reservoir (currently an Ohaus Ranger
7000, RS232, may be swapped for a different model), 10 L feed bottles, and a
chemostat/continuous-culture setup where **volume is fixed mechanically** by
an overflow tube plus an outlet pump running well above any feed rate, the
outlet side needs no software and is out of scope here (operated manually).
What needs closing the loop on is the **feed rate itself**: accelerostat
(ramping dilution rate up), decelerostat (ramping down), chemostat (constant
dilution rate), or any of the existing curve shapes.

A hand-written plan already exists (`docs/plan_controle_volume_chemostat.md`):
a state machine, Theil-Sen slope estimation, a cumulative mass-balance check,
and a PID with anti-windup, written against a standalone Python/pymodbus
script. This spec ports that design into Fermentool's actual architecture and
supersedes that file (deleted once this spec is committed).

## Goal

1. **Manual mode (default, unchanged).** Exactly today's behavior:
   `target = curve.value_at(elapsed)`, written without regard for calibration.
   Nothing about this path changes.
2. **Automatic mode (new, opt-in per run).** The same `target` is corrected by
   a slowly-adapting factor learned from the scale:
   `target_sent(t) = curve.value_at(t) * c(t)`.
   Works identically regardless of curve shape (`Linear` up = accelerostat,
   `Linear` down = decelerostat, `Constant` = chemostat, `Exponential`,
   `Sigmoid`, ...) and regardless of `control_var` (rpm or ml/min), `c(t)` is
   a dimensionless measured/commanded ratio.
3. **The balance model is swappable.** A new balance is a new small driver, not
   a rewrite. Ohaus Ranger 7000 (RS232, supports **MT-SICS** command mode) is
   the first target; MT-SICS is a de facto standard many other bench/platform
   scales speak too, so it becomes the default protocol rather than a
   vendor-specific one.
4. **A lost or wedged scale never stops the pump.** It degrades to manual
   (frozen `c`) and raises an alarm, confirmed operator decision, matching
   how every other sensor loss in this daemon already behaves
   (`serial_ok`, `journal_ok`, `pump_confirmed`).
5. **`c(t)` survives a crash-resume**, like everything else in this daemon.
   A trim that converged over 50 h must not reset to 1.0 on a restart.

**Non-goals (v1):**

- The outlet ("chemo-out") pump. Manual, external to Fermentool, out of scope.
- Mechanical volume control. The overflow tube does this; no software involved.
- Exposing the trim's tuning constants (bands, gain, window bounds) in the UI.
  They're hardcoded with a documented rationale, like `REOPEN_AFTER_WRITE_FAILS`
  and friends already are. A follow-up can make them configurable.
- Automatic density calibration (a 2-point measured-volume calibration at run
  start). v1 takes a configured `density_g_per_ml`; the calibration workflow
  is a real improvement but not required to ship the trim.
- An accelerometer for perturbation detection (the plan mentions one as
  optional). v1 detects perturbation from the weight signal alone (a brief,
  moderate jump that doesn't cross the refill threshold); the accelerometer
  input is a clean future addition to the same state machine, not a redesign.

## Implementation order

**Step 0 is a spike, and it happens before any Rust code.** A throwaway
script (not part of the crate) that opens the balance's serial port and
determines, for the operator's actual Ranger 7000:

1. Does `S<CR><LF>` (MT-SICS "send stable weight") get a parseable reply
   (`S S <weight> <unit><CR><LF>`)? What about `SI` (immediate, unstable
   allowed)? Confirm SICS mode is actually enabled on the unit
   (§5.9.10 in the manual) and the exact framing (`<CR><LF>` vs `<CR>` only,
   whether the unit suffix is always `g`).
2. If SICS doesn't answer: fall back to reading the continuous-output stream
   (§5.9.12 "Continu OHAUS", 17/18-byte frames per §"Sortie en continu
   standard") and dump raw bytes, the manual's status-byte bit tables are too
   OCR-damaged to trust; decode them empirically against known weights instead
   of from the doc.
3. Record: which mode works, exact request/reply bytes, baud/parity/stop bits
   in use, and how many decimal places / what unit the reply carries.

This determines which driver (§1 below) gets built for real, and whether a
second one is worth it. **Do not write the `SicsScale`/`OhausContinuousScale`
drivers until this comes back.**

### Spike result: SICS confirmed

Run against the real Ranger 7000 (USB, FT230X USB-serial bridge -> Windows
picked it up as a plain COM port with the in-box driver, no Ohaus CD needed),
factory settings, no configuration changed on the balance:

```
--port <COMn> --baud 9600 (8N1)
S\r\n   ->  S S      740.5 g\r\n
SI\r\n  ->  S S      740.6 g\r\n
```

- MT-SICS works out of the box; `S` and `SI` both reply in the standard
  `<cmd-echo> <status> <value> <unit>` shape, status `S` = stable.
- Framing is `\r\n` (CRLF), not bare `\r`.
- Resolution is **0.1 g** (one decimal digit) at these factory settings, unit
  always `g`.
- Continuous-output mode was never needed. **Driver to build: `SicsScale`
  only.** `OhausContinuousScale` is dropped from scope (§1, §9 unaffected:
  still generic over any `Transport` impl, so adding it later if a different
  balance model needs it is still just a new small driver, not a redesign).

## Design

### 1. Balance link: reuse `Transport`, not a new trait

`fermentool_modbus::Transport::transaction(&mut self, request: &[u8]) ->
Result<Vec<u8>, TransportError>` has no MODBUS-specific shape: it's "send
these bytes, get a response, within a timeout." A balance protocol is the
same shape:

```rust
// crates/fermentool-core/src/scale.rs (new)
pub struct SicsScale { port: SerialTransport /* from fermentool_modbus::serial */ }

impl fermentool_modbus::Transport for SicsScale {
    fn transaction(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportError> {
        // request is the SICS command (e.g. b"S\r\n"); reuse the raw
        // byte-transaction the pump's SerialTransport already provides if it
        // turns out not to be MODBUS-specific below the framing layer
        // (confirm while implementing): otherwise a ~20-line raw
        // write-then-read-until-terminator over `serialport` directly.
        self.port.transaction(request)
    }
}

fn parse_sics_weight(reply: &[u8]) -> Result<f64, ScaleError> { /* "S S  123.45 g" -> 123.45 */ }
```

This is a **local type implementing a foreign trait** (allowed under the
orphan rules): zero changes to `fermentool-modbus`.

**`WatchdogTransport` (`crates/fermentool-core/src/transport.rs`) is already
generic over `Box<dyn Transport + Send>`.** Wrap the scale in one exactly like
the pump: a wedged read on the balance's line can only block that worker
thread, never the control loop, for free. No new resilience code needed.

If the spike shows SICS doesn't work, `OhausContinuousScale` implements the
same `Transport` trait by parsing the continuous frame instead: the rest of
the design (state machine, trim, persistence) doesn't care which one is
underneath.

### 2. Config

```toml
[scale]
path = ""                    # "" / absent = no scale, automatic mode unavailable
baud = 9600
protocol = "sics"            # "sics" | "ohaus_continuous"
density_g_per_ml = 1.0       # feed density; 500 g/L glucose feed ~= 1.18
```

Same shape as `[serial]` for the pump: a `WatchdogTransport::spawn` at boot,
`serial_ok`-style flag surfaced as `scale_ok`.

### 3. State machine (ported from the existing plan, unchanged logic)

```
NORMAL -> PERTURBATION -> NORMAL                       (transient: freeze, don't reset)
NORMAL -> REFILL_PENDING -> REFILL_SETTLING -> NORMAL   (freeze + reset the cumulative reference)
```

| State | Trigger | Behavior |
|---|---|---|
| `Normal` | default | readings feed the rolling buffer; trim updates run |
| `Perturbation` | a weight jump too big to be noise but well under the refill threshold | buffer untouched but **not appended**, `c` frozen, pump keeps its current setpoint |
| `RefillPending` | jump > refill threshold (default 50 g) OR operator `POST /api/scale/refill` | buffer cleared, `c` frozen, pump keeps its current setpoint |
| `RefillSettling` | weight variance below a threshold for N seconds | short additional wait for mechanical settling |
| back to `Normal` | settling elapsed | **new cumulative reference** (`weight_at_refill`, `t_refill`), this reset is the one detail the original plan calls out as make-or-break: skip it and a refill reads as a giant negative flow and wrecks the trim |

Refill detection is **manual-first** (`POST /api/scale/refill_mode` before
opening the bottle, `POST /api/scale/refill_done` after) with the automatic
weight-jump threshold as the safety net, exactly as the plan specifies.

### 4. Flow estimate and adaptive window

- 1 reading/1-5 s, median filter (window ~5-10 points) against isolated spikes.
- In `Normal` only: rolling buffer, slope via Theil-Sen (robust to outliers
  without hand-tuned thresholds) -> measured mass rate -> `Q_meas` (mL/min)
  via `density_g_per_ml`.
- **Window is adaptive to the commanded rate**, not fixed: a fixed short
  window is noise at a low F₀, a fixed long one wastes minutes of reaction
  time at a high one.
  ```
  window = clamp(k * sigma_dm / (rho * p * F_cmd), 5 min, 2 h)   // k ~= 1.5-2, p = 0.10
  ```
  Below the point where `window` hits the 2 h clamp (roughly F_cmd < ~0.5
  mL/min on a coarse platform scale), don't attempt a rate comparison at all,
  fall back to the cumulative check (§5) only.

### 5. Cumulative mass balance (the stabilizing signal)

Since the last refill (or run start):

```
mass_theoretical(t) = integral of curve.value_at * density over [t_refill, t]
mass_measured(t)    = weight_at_refill - weight_now
error_frac          = (mass_theoretical - mass_measured) / mass_theoretical
```

This is what actually drives `c` (§6), it's smoother than the instantaneous
rate and gives a real total-volume guarantee even while the instantaneous
signal is noisy.

### 6. Trim update: bounded, slow, two-tier

```
|error_frac| < 3%              -> no-op (inside the noise floor)
3% <= |error_frac| <= 15%      -> c *= (1 + gamma * error_frac)   gamma ~= 0.2-0.5
|error_frac| > 15-20%          -> ALARM (clog / empty / gross miscalibration); freeze c
```

- `c` clamped to `[0.80, 1.25]`; if the update would push it outside, that's
  not calibration drift, it's a fault -> alarm instead of correcting further.
- Max `|Delta c|` per update also capped (~2%), independent of `gamma`, so a
  single bad window can't jump `c` far.
- `c` freezes (no update, old value kept) whenever the state machine isn't
  `Normal`, or the scale is unreachable (§8).

### 7. Persistence (crash-resume)

Extend the tick journal row (or a lightweight periodic event, whichever is
cheaper to add to `store::NewTick`) with: `trim_c`, `scale_state`,
`weight_at_refill`, `t_refill`. On `Engine::new` / resume, the last tick's
values seed the in-memory trim state, the same pattern `holding` already
uses (`app_state`) for surviving a restart. Manual-mode runs write nothing new
here (`trim_c` stays `1.0`, unused).

### 8. Safety: scale unreachable

`WatchdogTransport`'s existing timeout (`HARD_TXN_TIMEOUT`) already bounds a
stuck read. On top of that, the control loop tracks consecutive scale-read
failures; past a threshold (mirrors `REOPEN_AFTER_WRITE_FAILS`):

- `scale_ok = false` (new status field, same family as `serial_ok` /
  `pump_confirmed` / `journal_ok`, same UI alarm-banner pattern).
- `c` freezes at its last value.
- The pump **keeps running** on `curve.value_at(t) * c_frozen`, i.e. it
  degrades to manual mode with whatever correction was last trusted, rather
  than stopping. Confirmed behavior (this session, explicit choice over
  "pause the pump" from the original plan): a lost sensor must never silently
  stop the feed.
- `scale_ok` recovering (reconnect / auto-retry, same backoff shape as the
  pump's `WatchdogTransport::swap_strict`) resumes trim updates from `Normal`.

### 9. RunConfig / API / UI

- `RunConfig.gravimetric_trim: bool` (default `false`).
- `/api/status` gains `scale_ok`, `scale_state`, `trim_c` (all `null`/absent
  when no scale is configured or the run is manual).
- `POST /api/scale/refill_mode` / `POST /api/scale/refill_done` (manual
  refill trigger, §3).
- `NewRun.svelte`: a checkbox "Enable gravimetric trim (requires a configured
  scale)", enabled only when `/api/config` reports `scale.path` set.
  Independent of curve shape / control_var, no UI branching per shape.
- `Overview.svelte`: when a run has `gravimetric_trim`, show `scale_state` and
  `trim_c` next to the existing pump-link indicators.

## Testing

- Spike (Step 0): manual, against the real balance. Gate for everything else.
- `parse_sics_weight` / continuous-frame parser: unit tests against captured
  byte strings from the spike.
- Theil-Sen slope helper: unit tests with synthetic noisy series (known slope
  + outliers) against a hand-computed expected slope.
- State machine: unit tests per transition (`Normal` -> `Perturbation` ->
  `Normal` without a buffer reset; `Normal` -> `RefillPending` ->
  `RefillSettling` -> `Normal` **with** the cumulative reference reset;
  manual and automatic refill triggers).
- Trim bounds: property-style tests that `c` never leaves `[0.80, 1.25]` and
  never moves more than the per-window cap regardless of how extreme
  `error_frac` is.
- Engine integration: a fake `Transport` scale double (mirrors `SimPump`) that
  can be scripted to return a sequence of weights, refills, and read failures,
  exercised through `Engine::tick` to prove `target_sent = curve * c` and the
  freeze-on-failure behavior end to end. No real hardware needed for this
  layer once the parser is unit-tested against real captured bytes.
- Persistence: a resumed engine (same pattern as
  `resumes_a_running_run_after_a_simulated_crash`) must pick up the same
  `trim_c` / `scale_state` / refill reference the crashed one had.

## Risks / notes

- **The manual's continuous-frame status-byte tables are OCR-damaged** and
  not to be trusted verbatim; if that path is needed, decode it empirically
  (known weights, dump raw bytes) rather than from the doc. MT-SICS avoids
  this risk entirely, which is why it's the default target.
- **Coarse balance resolution vs. low feed rates.** A 10 L bottle needs a
  ≥15 kg-capacity scale, typically 1-5 g resolution. At F₀ in the tenths of
  mL/min, that's only a cumulative check for hours, no real-time trim is
  possible there, by physics, not by a software limitation. Documented in §4;
  not a bug to "fix" later.
- **Tube tension / vibration coupling into the balance** is a hardware
  problem (service loop, anchor point above the bottle, decoupled table),
  software (median filter + Theil-Sen + adaptive window) mitigates it but
  can't fully substitute for the physical setup already noted in the original
  plan doc (kept, ported as-is into whatever wiring doc ends up covering it).
- **`SerialTransport` reuse for the scale is not yet confirmed** to be free of
  MODBUS-specific framing below the transaction layer; if it turns out to be,
  a small dedicated raw-serial opener (write, read-until-terminator-or-timeout)
  is a similarly small addition, not a redesign.
- **This is additive, not a rework.** Every existing test, every manual run,
  every curve shape keeps behaving exactly as today when `gravimetric_trim`
  is `false` (the default). Nothing here touches `CurveSpec::value_at`,
  `Engine::tick`'s existing write path is only wrapped, not replaced.
