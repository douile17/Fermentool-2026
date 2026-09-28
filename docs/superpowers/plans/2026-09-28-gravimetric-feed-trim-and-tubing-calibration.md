# Gravimetric Feed Trim + Tubing Calibration: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**This file replaces and supersedes three earlier documents, deleted when
this plan was committed:**
- `docs/superpowers/specs/2026-09-11-gravimetric-feed-trim-design.md` (design rationale, folded into "Design context" below)
- `docs/superpowers/plans/2026-09-11-gravimetric-feed-trim.md` (Tasks 1-10 below are that plan, unchanged except where the calibration feature touches the same files)
- `synthese_chemostat_calibration_compensation.md` (an alternate RPM<->mL/min regression + `Kcorr` approach; superseded by the single-scalar trim below, its useful ideas, mainly the learning-conditions checklist and the full data-logging column list, are folded into Task 5 and Task 9)

**Status:** design approved (both the original trim and the tubing
calibration addendum), Step 0 hardware spikes already done (MT-SICS
confirmed against the real Ranger 7000; real DB15 RS485 pinout confirmed
against the LabQ manual, see `docs/wiring.md`). **No code has been written
yet** (`scale.rs`, `trim.rs` do not exist; no `ScaleConfig`; no
`gravimetric_trim` anywhere in `engine/mod.rs`).

**Goal:** Two features that ship together because the second exists to fix a
real gap in the first:

1. **Ongoing gravimetric trim.** A slowly-adapting, bounded correction factor
   `c` multiplies the feed pump's setpoint throughout a run, driven by a
   scale under the feed bottle, so the real delivered volume tracks the
   curve even as the tube ages or backpressure changes. Opt-in per run
   (`gravimetric_trim = true`), manual mode (`false`, the default) is
   byte-for-byte unchanged.
2. **Pre-autoclave tubing calibration.** `c` starting at `1.0` on a brand-new
   tube means the first minutes/hours of a run run open-loop and possibly
   far from target while the slow trim (capped at 2%/update) catches up.
   Calibrating a tube once, before it's sterilized and installed, gives an
   accurate starting `c0` instead, and in **rpm** control mode it is not
   just an accuracy nicety, it is a **hard requirement**: the ongoing
   trim's mass-balance math has no rpm-to-mL/min relationship to work from
   without it.

## Design context

**Problem:** Fermentool drives the feed pump open loop:
`Engine::tick` writes `curve.value_at(elapsed)` and never checks whether the
pump actually delivered it. Over a ~100 h run the real flow drifts from the
commanded one (peristaltic tube wear, tube relaxation, backpressure), and
nothing in the daemon notices. Separately, a fresh tube's very first delivery
is exactly as uncalibrated as one that's been running for 80 hours, there is
no accurate starting point without ever having measured this specific tube.

**Setup assumed:** a scale under the feed reservoir (currently an Ohaus
Ranger 7000, RS232, MT-SICS confirmed), a chemostat/continuous-culture setup
where volume is fixed mechanically by an overflow tube plus a manually
operated outlet pump (out of scope, no software involved). What needs
closing the loop is the feed rate itself, for any of the existing curve
shapes and either `control_var` (rpm or ml/min).

**Non-goals (v1):**
- The outlet ("chemo-out") pump; manual, out of scope.
- Mechanical volume control; the overflow tube does this.
- Exposing tuning constants (bands, gain, window bounds, CV% threshold) in
  the UI. Hardcoded with a documented rationale, like
  `REOPEN_AFTER_WRITE_FAILS` and friends already are.
- An accelerometer for perturbation detection. v1 detects perturbation from
  the weight signal alone.
- A full multi-point RPM<->mL/min regression curve (the `synthese_chemostat`
  doc's `a`, `b`, `R²` approach). Explicitly decided against: it needs a
  whole calibration-sequence subsystem for something the single-scalar `c0`
  already solves for this project's actual need (start close, let the
  ongoing trim handle drift). A regression-based calibration is a clean
  future addition if diagnostic-grade tube characterization is ever needed,
  it does not block or conflict with anything here.

## Global Constraints

- **Manual mode (`gravimetric_trim = false`, the default) is byte-for-byte
  unchanged.** `c` is pinned to `1.0` whenever trim is off, so
  `target = curve.value_at(elapsed) * 1.0` is exactly today's `target`. No
  existing test may need to change because of this feature.
- **Driver: `SicsScale` only.** No `OhausContinuousScale` (dropped after the
  spike). Confirmed protocol: send `b"S\r\n"` or `b"SI\r\n"`, reply
  `b"S <status> <value> g\r\n"` where status is `S` (stable) or `D`
  (dynamic/unstable).
- **`SerialTransport` (the pump's transport) is not reusable for the scale
  as-is**: it hardcodes 8E1 framing (`.parity(Parity::Even)`) and
  `read_until` expects a caller-known fixed byte count, both wrong for a
  variable-length ASCII line at whatever parity the balance is configured
  for. The scale gets its own small raw-serial opener (Task 2).
- **`WatchdogTransport` is reused unmodified** except promoting its
  `#[cfg(test)]`-only generic constructor to a real public one (Task 3): its
  worker-thread-plus-hard-timeout design already doesn't care what
  `Transport` impl it's wrapping.
- **Trim bounds** (every task agrees on these): ignore `|error_frac| < 3%`;
  trim for `3% <= |error_frac| <= 15%` via `c *= (1 + gamma * error_frac)`,
  `gamma = 0.3`; alarm (freeze, don't correct) for `|error_frac| > 20%`
  sustained; `c` clamped to `[0.80, 1.25]`; max `|delta c|` per update `2%`;
  `c` freezes outside the `Normal` state or when the scale is unreachable.
- **Refill threshold:** a weight jump `> 50.0 g` is a refill; `5.0..=50.0 g`
  is a perturbation; `< 5.0 g` is noise (`PERTURBATION_MIN_G = 5.0`,
  `REFILL_THRESHOLD_G = 50.0`).
- **A lost/wedged scale never stops the pump.** It freezes `c`, sets
  `scale_ok = false`, and the run keeps going on `curve.value_at(t) *
  c_frozen` (manual-equivalent behavior with the last trusted correction).
- **The balance is never read live during a tubing calibration.** Weight is
  typed in by the operator by hand (a bench calibration jig is not
  necessarily wired to the same software instance as the eventual run).
  This is intentional, not a gap: document it plainly so it isn't
  "discovered" and re-litigated later.
- **Calibration is mandatory before enabling gravimetric trim in rpm mode**,
  optional (recommended) in ml/min mode. See Task 12.
- **No UI exposure of tuning constants in v1** (bands, gamma, window bounds,
  thresholds, CV% acceptance threshold are all Rust constants, not config).
- **Commits:** no `Co-Authored-By: Claude` trailer, no `Claude-Session` line
  (project rule). **No em dash (Unicode U+2014) and no spaced-hyphen (` - `)
  anywhere**: comma, colon, or period only (project rule, `CLAUDE.md`).
- **After any `ui/src/` change, run `npm run build` from `ui/` without being
  asked.**

## File Structure

**New:**
- `crates/fermentool-core/src/scale.rs`: `SicsScale` (implements
  `Transport`), `parse_sics_weight`, the raw serial line opener.
- `crates/fermentool-core/src/trim.rs`: pure logic, no I/O: `TheilSen` slope
  estimator, `ScaleState` enum + transition function, the trim-update
  function, `compute_calibration` (the tubing-calibration math).
- `ui/src/routes/TubingCalibration.svelte`: the 3-replicate calibration
  workflow screen.

**Modified:**
- `crates/fermentool-core/src/config.rs`: `ScaleConfig`, `Config.scale`.
- `crates/fermentool-core/src/transport.rs`: promote `from_opener`/
  `respawn_with` off `#[cfg(test)]`.
- `crates/fermentool-core/src/engine/mod.rs`: `Engine` gains
  `scale: Option<Box<dyn Transport + Send>>`, `trim_c: f64`,
  `scale_state: ScaleState`, refill reference fields, a rolling weight
  buffer, `rpm_to_ml_min: Option<f64>`; `ActiveRun.gravimetric_trim: bool`;
  `RunConfig.gravimetric_trim: bool`, `RunConfig.tubing_calibration_id:
  Option<i64>`; `tick()`/`apply_setpoint()` multiply by `trim_c`;
  `scale_tick(now)`; a new `EngineError::GravimetricTrimNeedsCalibration`.
- `crates/fermentool-core/src/control.rs`: `DaemonStatus` gains `scale_ok`,
  `scale_state`, `trim_c`, `rate_g_per_min`; control loop calls `scale_tick`;
  two new `Command`s for manual refill trigger.
- `crates/fermentool-core/src/main.rs`: boot the scale's transport.
- `crates/fermentool-core/src/store/mod.rs` +
  `crates/fermentool-core/src/store/migrations/0002_tubing_calibration.sql`:
  `runs.kind`, `runs.tubing_calibration_id`, new `tubing_calibrations` table,
  `NewCalibration`/`CalibrationRow`, `insert_calibration`,
  `list_calibrations`, `calibration`.
- `crates/fermentool-core/src/api.rs`: `POST /api/scale/refill_mode`,
  `POST /api/scale/refill_done`, `POST /api/calibrations`,
  `GET /api/calibrations`.
- `config.example.toml`: document `[scale]`.
- `ui/src/routes/NewRun.svelte`: the trim checkbox, the calibration picker.
- `ui/src/routes/Overview.svelte`: `scale_state`/`trim_c` display.

---

## Task 1: `ScaleConfig`

**Goal:** A `[scale]` section in `config.toml`, same shape as `[serial]`.

**Files:** Modify `crates/fermentool-core/src/config.rs`, `config.example.toml`

**Interfaces:** `pub struct ScaleConfig { pub path: String, pub baud: u32, pub density_g_per_ml: f64 }`, `Default` (`path: ""`, `baud: 9600`, `density_g_per_ml: 1.0`), `ScaleConfig::configured(&self) -> bool`, `Config.scale: ScaleConfig`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn scale_defaults_to_unconfigured() {
    let cfg = Config::default();
    assert_eq!(cfg.scale.path, "");
    assert!(!cfg.scale.configured());
    assert_eq!(cfg.scale.baud, 9600);
    assert_eq!(cfg.scale.density_g_per_ml, 1.0);
}

#[test]
fn scale_config_round_trips_through_toml() {
    let toml = "[scale]\npath = \"COM5\"\nbaud = 9600\ndensity_g_per_ml = 1.18\n";
    let cfg = Config::from_toml(toml).unwrap();
    assert!(cfg.scale.configured());
    assert_eq!(cfg.scale.path, "COM5");
    assert_eq!(cfg.scale.density_g_per_ml, 1.18);
}
```

- [ ] **Step 2: Run, verify they fail.** `cargo test -p fermentool-core scale_`, expect `no field \`scale\`` compile error.

- [ ] **Step 3: Add `ScaleConfig` and wire it into `Config`**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScaleConfig {
    /// Serial device path. Empty = no scale configured; automatic
    /// (gravimetric-trim) runs are unavailable until this is set.
    pub path: String,
    /// Whatever baud the scale's own Communications menu is set to.
    pub baud: u32,
    /// Feed density (g/mL), used to convert a measured mass rate to a volume
    /// rate. Water ~= 1.0; a 500 g/L glucose feed ~= 1.18.
    pub density_g_per_ml: f64,
}

impl Default for ScaleConfig {
    fn default() -> Self {
        Self { path: String::new(), baud: 9600, density_g_per_ml: 1.0 }
    }
}

impl ScaleConfig {
    pub fn configured(&self) -> bool {
        !self.path.trim().is_empty()
    }
}
```
Add `pub scale: ScaleConfig` to `Config`.

- [ ] **Step 4: Run, verify pass.** `cargo test -p fermentool-core scale_`.

- [ ] **Step 5: Document in `config.example.toml`**

```toml
[scale]
path = ""                     # empty = no scale; e.g. Windows: "COM5"
baud = 9600                    # match the balance's own Communications menu
density_g_per_ml = 1.0         # feed density; water ~1.0, 500 g/L glucose ~1.18
# MT-SICS only for now (send "S"/"SI", parse "S <status> <value> g"). A
# balance without SICS needs its own small driver alongside SicsScale.
```

- [ ] **Step 6: Full suite + commit**

```bash
git add crates/fermentool-core/src/config.rs config.example.toml
git commit -m "feat(core): add [scale] config section"
```

---

## Task 2: `SicsScale` transport + parser

**Goal:** A `Transport` impl that speaks MT-SICS over a raw serial line, plus a well-tested parser for the reply format confirmed in the spike.

**Files:** Create `crates/fermentool-core/src/scale.rs`; modify `crates/fermentool-core/src/lib.rs`

**Interfaces:**
- `pub struct SicsScale { .. }`, `pub fn open(path: &str, baud: u32, timeout: Duration) -> Result<Self, TransportError>`.
- `impl Transport for SicsScale`.
- `pub fn parse_sics_weight(reply: &[u8]) -> Result<(f64, bool), ScaleError>`, `true` = stable (`S`), `false` = dynamic (`D`).
- `pub const SICS_STABLE: &[u8] = b"S\r\n";` / `pub const SICS_IMMEDIATE: &[u8] = b"SI\r\n";`
- `#[derive(Debug)] pub enum ScaleError { Transport(TransportError), Parse(String) }` with `Display`/`Error`.

- [ ] **Step 1: Write the failing parser tests (no hardware needed)**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_stable_reply() {
        let (w, stable) = parse_sics_weight(b"S S      740.5 g\r\n").unwrap();
        assert!((w - 740.5).abs() < 1e-9);
        assert!(stable);
    }

    #[test]
    fn parses_a_dynamic_reply() {
        let (w, stable) = parse_sics_weight(b"S D      740.6 g\r\n").unwrap();
        assert!((w - 740.6).abs() < 1e-9);
        assert!(!stable);
    }

    #[test]
    fn rejects_a_malformed_reply() {
        assert!(parse_sics_weight(b"garbage\r\n").is_err());
        assert!(parse_sics_weight(b"").is_err());
    }

    #[test]
    fn rejects_an_overload_reply() {
        assert!(parse_sics_weight(b"S +\r\n").is_err());
    }
}
```

- [ ] **Step 2: Run, verify they fail.** `cargo test -p fermentool-core --lib scale::tests`, compile error (module doesn't exist yet).

- [ ] **Step 3: Write `scale.rs`**

```rust
//! MT-SICS balance link. Confirmed against a real Ohaus Ranger 7000:
//! `S`/`SI` -> `S <status> <value> g\r\n`, status S = stable, D = dynamic.
//!
//! Deliberately NOT built on fermentool_modbus::serial::SerialTransport: that
//! type hardcodes 8E1 framing and reads a caller-known fixed byte count,
//! neither of which fits a variable-length ASCII line at whatever parity the
//! balance's own Communications menu is set to.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use fermentool_modbus::{Transport, TransportError};
use serialport::{DataBits, Parity, StopBits};

pub const SICS_STABLE: &[u8] = b"S\r\n";
pub const SICS_IMMEDIATE: &[u8] = b"SI\r\n";

pub struct SicsScale {
    port: Box<dyn serialport::SerialPort>,
    timeout: Duration,
}

impl SicsScale {
    pub fn open(path: &str, baud: u32, timeout: Duration) -> Result<Self, TransportError> {
        let port = serialport::new(path, baud)
            .data_bits(DataBits::Eight)
            .parity(Parity::None)
            .stop_bits(StopBits::One)
            .timeout(Duration::from_millis(200))
            .open()
            .map_err(|e| TransportError::Io(e.to_string()))?;
        Ok(Self { port, timeout })
    }
}

impl Transport for SicsScale {
    fn transaction(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportError> {
        self.port
            .clear(serialport::ClearBuffer::Input)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        self.port.write_all(request).map_err(|e| TransportError::Io(e.to_string()))?;
        self.port.flush().map_err(|e| TransportError::Io(e.to_string()))?;

        let deadline = Instant::now() + self.timeout;
        let mut buf = Vec::with_capacity(32);
        let mut chunk = [0u8; 64];
        loop {
            if buf.ends_with(b"\r\n") {
                return Ok(buf);
            }
            if Instant::now() >= deadline {
                return Err(TransportError::Timeout);
            }
            match self.port.read(&mut chunk) {
                Ok(0) => return Err(TransportError::Timeout),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(TransportError::Io(e.to_string())),
            }
        }
    }
}

#[derive(Debug)]
pub enum ScaleError {
    Transport(TransportError),
    Parse(String),
}

impl std::fmt::Display for ScaleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScaleError::Transport(e) => write!(f, "scale: {e}"),
            ScaleError::Parse(s) => write!(f, "scale: could not parse reply: {s}"),
        }
    }
}
impl std::error::Error for ScaleError {}

impl From<TransportError> for ScaleError {
    fn from(e: TransportError) -> Self {
        ScaleError::Transport(e)
    }
}

/// Parse a MT-SICS "S"/"SI" reply: `"S <status> <value> <unit>\r\n"`.
pub fn parse_sics_weight(reply: &[u8]) -> Result<(f64, bool), ScaleError> {
    let text = std::str::from_utf8(reply)
        .map_err(|e| ScaleError::Parse(e.to_string()))?
        .trim();
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() < 3 {
        return Err(ScaleError::Parse(format!("too few fields: {text:?}")));
    }
    let stable = match parts[1] {
        "S" => true,
        "D" => false,
        other => return Err(ScaleError::Parse(format!("unexpected status {other:?}"))),
    };
    let value: f64 = parts[2]
        .parse()
        .map_err(|_| ScaleError::Parse(format!("bad numeric value: {:?}", parts[2])))?;
    Ok((value, stable))
}
```
Add `pub mod scale;` to `lib.rs`. Add `serialport` to `fermentool-core`'s `Cargo.toml` explicitly (check `cargo tree -p fermentool-core -i serialport` first, matching whatever version `fermentool-modbus` pins).

- [ ] **Step 4: Run, verify pass.** `cargo test -p fermentool-core --lib scale::tests`, 4 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/fermentool-core/src/scale.rs crates/fermentool-core/src/lib.rs crates/fermentool-core/Cargo.toml Cargo.lock
git commit -m "feat(core): SicsScale transport + MT-SICS reply parser"
```

---

## Task 3: Reuse `WatchdogTransport` for the scale

**Goal:** Boot (or skip, if unconfigured) a watchdog-wrapped scale link, so a wedged balance read can never freeze the daemon, exactly like the pump.

**Files:** Modify `crates/fermentool-core/src/transport.rs`, `crates/fermentool-core/src/main.rs`

- [ ] **Step 1: Promote the generic constructor.** Remove `#[cfg(test)]` from `from_opener`, rename `spawn_with`, update its two existing test call sites. Make `Opener` `pub(crate)`.

- [ ] **Step 2: Run the existing transport tests under the rename.** `cargo test -p fermentool-core --lib transport::tests`.

- [ ] **Step 3: Boot the scale in `main.rs`**

```rust
use fermentool_core::scale::SicsScale;
use fermentool_core::transport::WatchdogTransport;

const SCALE_OPEN_TIMEOUT: Duration = Duration::from_millis(1500);

fn build_scale(cfg: &config::ScaleConfig) -> Option<WatchdogTransport> {
    if !cfg.configured() {
        return None;
    }
    let path = cfg.path.clone();
    let baud = cfg.baud;
    let (watchdog, _kind) = WatchdogTransport::spawn_with(Box::new(move || {
        match SicsScale::open(&path, baud, SCALE_OPEN_TIMEOUT) {
            Ok(s) => (Box::new(s) as Box<dyn fermentool_modbus::Transport + Send>, None),
            Err(e) => {
                tracing::error!("cannot open scale {path} ({e}); gravimetric trim unavailable");
                (Box::new(fermentool_modbus::SimPump::new(1)) as Box<dyn fermentool_modbus::Transport + Send>, None)
            }
        }
    }));
    Some(watchdog)
}
```
Wire `let scale = build_scale(&config.scale);` into `main()`, threaded into `Engine::new`'s construction path as `Some(Box::new(scale))` (`WatchdogTransport` itself implements `Transport`, so `Engine.scale: Option<Box<dyn Transport + Send>>` accepts it directly, no separate field needed for the real vs. test path, see Task 7).

- [ ] **Step 4: Build.** `cargo build -p fermentool-core`.

- [ ] **Step 5: Commit**

```bash
git add crates/fermentool-core/src/transport.rs crates/fermentool-core/src/main.rs
git commit -m "feat(core): boot the scale link through WatchdogTransport"
```

---

## Task 4: Theil-Sen slope estimator (pure)

**Goal:** A robust slope-of-noisy-points function, decimated so it stays cheap even over a 2 h window.

**Files:** Create `crates/fermentool-core/src/trim.rs`; modify `crates/fermentool-core/src/lib.rs`

**Interfaces:** `pub const MAX_SLOPE_POINTS: usize = 200;`, `pub fn decimate(points: &[(f64, f64)], max_len: usize) -> Vec<(f64, f64)>`, `pub fn theil_sen_slope(points: &[(f64, f64)]) -> Option<f64>`.

- [ ] **Step 1: Write the failing tests**

```rust
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
```

- [ ] **Step 2: Run, verify they fail.** `cargo test -p fermentool-core --lib trim::tests`, compile error.

- [ ] **Step 3: Implement**

```rust
//! Pure math for the gravimetric feed trim: no I/O, no clocks (matches the
//! fermentool-curves convention -- feed it data, get a number back).

pub fn decimate(points: &[(f64, f64)], max_len: usize) -> Vec<(f64, f64)> {
    if points.len() <= max_len || max_len < 2 {
        return points.to_vec();
    }
    let stride = (points.len() - 1) as f64 / (max_len - 1) as f64;
    (0..max_len)
        .map(|i| points[((i as f64 * stride).round() as usize).min(points.len() - 1)])
        .collect()
}

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
```
Add `pub mod trim;` to `lib.rs`.

- [ ] **Step 4: Run, verify pass.** `cargo test -p fermentool-core --lib trim::tests`, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/fermentool-core/src/trim.rs crates/fermentool-core/src/lib.rs
git commit -m "feat(core): Theil-Sen slope estimator for the feed trim"
```

---

## Task 5: State machine (pure)

**Goal:** `Normal`/`Perturbation`/`RefillPending`/`RefillSettling`, exactly the transitions below, as a pure function so every edge is trivial to test.

Learning/update is allowed to run **only** in `Normal`. This folds in the
`synthese_chemostat` doc's "conditions d'autorisation de l'auto-apprentissage"
checklist (pump running, scale stable, no tank handling, no refill in
progress, no active alarm): every one of those conditions is already implied
by "currently `Normal`" in this state machine, they don't need to be
re-checked as a separate list.

**Files:** Modify `crates/fermentool-core/src/trim.rs`

**Interfaces:**
- `pub const PERTURBATION_MIN_G: f64 = 5.0;` `pub const REFILL_THRESHOLD_G: f64 = 50.0;` `pub const REFILL_SETTLE_VARIANCE_G: f64 = 0.5;` `pub const REFILL_SETTLE_SECONDS: f64 = 10.0;`
- `#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)] pub enum ScaleState { Normal, Perturbation, RefillPending, RefillSettling }`
- `pub struct StateInput { pub weight_g: f64, pub prev_weight_g: f64, pub recent_variance_g: f64, pub seconds_in_settling: f64, pub manual_refill_mode: bool, pub manual_refill_done: bool }`
- `pub fn next_state(current: ScaleState, input: &StateInput) -> ScaleState`

- [ ] **Step 1: Write the failing tests**

```rust
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
```

- [ ] **Step 2: Run, verify they fail.** `cargo test -p fermentool-core --lib trim::state_tests`.

- [ ] **Step 3: Implement**

```rust
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
```

- [ ] **Step 4: Run, verify pass.** `cargo test -p fermentool-core --lib trim::`, 14 tests total.

- [ ] **Step 5: Commit**

```bash
git add crates/fermentool-core/src/trim.rs
git commit -m "feat(core): perturbation/refill state machine for the feed trim"
```

---

## Task 6: Trim update function (pure)

**Goal:** The bounded EWMA-style correction, exactly the bands and clamps in Global Constraints.

**Files:** Modify `crates/fermentool-core/src/trim.rs`

**Interfaces:** `pub const TRIM_IGNORE_BAND: f64 = 0.03;` `pub const TRIM_ALARM_BAND: f64 = 0.20;` `pub const TRIM_GAMMA: f64 = 0.3;` `pub const TRIM_MAX_DELTA: f64 = 0.02;` `pub const TRIM_MIN: f64 = 0.80;` `pub const TRIM_MAX: f64 = 1.25;` `pub enum TrimOutcome { Unchanged, Trimmed { new_c: f64 }, Alarm }` `pub fn update_trim(prev_c: f64, error_frac: f64) -> TrimOutcome`

- [ ] **Step 1: Write the failing tests**

```rust
mod trim_tests {
    use super::*;

    #[test]
    fn inside_the_noise_band_is_a_no_op() {
        assert!(matches!(update_trim(1.0, 0.01), TrimOutcome::Unchanged));
        assert!(matches!(update_trim(1.0, -0.02), TrimOutcome::Unchanged));
    }

    #[test]
    fn a_moderate_error_trims_by_a_fraction_of_it() {
        match update_trim(1.0, 0.10) {
            TrimOutcome::Trimmed { new_c } => assert!((new_c - 1.02).abs() < 1e-9, "got {new_c}"),
            other => panic!("expected Trimmed, got {other:?}"),
        }
    }

    #[test]
    fn a_negative_moderate_error_trims_down() {
        match update_trim(1.0, -0.10) {
            TrimOutcome::Trimmed { new_c } => assert!((new_c - 0.98).abs() < 1e-9, "got {new_c}"),
            other => panic!("expected Trimmed, got {other:?}"),
        }
    }

    #[test]
    fn a_gross_error_alarms_instead_of_correcting() {
        assert!(matches!(update_trim(1.0, 0.30), TrimOutcome::Alarm));
        assert!(matches!(update_trim(1.0, -0.25), TrimOutcome::Alarm));
    }

    #[test]
    fn c_never_leaves_its_clamp_regardless_of_repeated_extreme_input() {
        let mut c = 1.0;
        for _ in 0..500 {
            match update_trim(c, 0.15) {
                TrimOutcome::Trimmed { new_c } => c = new_c,
                TrimOutcome::Alarm => break,
                TrimOutcome::Unchanged => {}
            }
            assert!((TRIM_MIN..=TRIM_MAX).contains(&c), "c escaped its clamp: {c}");
        }
    }

    #[test]
    fn a_single_update_never_moves_more_than_the_per_window_cap() {
        if let TrimOutcome::Trimmed { new_c } = update_trim(1.0, 0.15) {
            assert!((new_c - 1.0).abs() <= TRIM_MAX_DELTA + 1e-9, "moved too far: {new_c}");
        }
    }
}
```

- [ ] **Step 2: Run, verify they fail.** `cargo test -p fermentool-core --lib trim::trim_tests`.

- [ ] **Step 3: Implement**

```rust
pub const TRIM_IGNORE_BAND: f64 = 0.03;
pub const TRIM_ALARM_BAND: f64 = 0.20;
pub const TRIM_GAMMA: f64 = 0.3;
pub const TRIM_MAX_DELTA: f64 = 0.02;
pub const TRIM_MIN: f64 = 0.80;
pub const TRIM_MAX: f64 = 1.25;

#[derive(Debug, Clone, Copy)]
pub enum TrimOutcome {
    Unchanged,
    Trimmed { new_c: f64 },
    Alarm,
}

/// One bounded, slow correction step. `error_frac = (theoretical - measured)
/// / theoretical` from the cumulative mass balance: positive means the pump
/// is under-delivering (needs to speed up), negative means it's over.
pub fn update_trim(prev_c: f64, error_frac: f64) -> TrimOutcome {
    let mag = error_frac.abs();
    if mag > TRIM_ALARM_BAND {
        return TrimOutcome::Alarm;
    }
    if mag < TRIM_IGNORE_BAND {
        return TrimOutcome::Unchanged;
    }
    let clamped_delta = (TRIM_GAMMA * error_frac).clamp(-TRIM_MAX_DELTA, TRIM_MAX_DELTA);
    let new_c = (prev_c * (1.0 + clamped_delta)).clamp(TRIM_MIN, TRIM_MAX);
    TrimOutcome::Trimmed { new_c }
}
```

- [ ] **Step 4: Run, verify pass.** `cargo test -p fermentool-core --lib trim::`, ~20 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/fermentool-core/src/trim.rs
git commit -m "feat(core): bounded trim-update function"
```

---

## Task 7: Wire the ongoing trim into `Engine`

**Goal:** Tasks 2, 4, 5, 6 actually drive the pump's setpoint, gated by a per-run flag, with the scale-unreachable degrade-to-manual behavior.

**Files:** Modify `crates/fermentool-core/src/engine/mod.rs`, `crates/fermentool-core/src/control.rs`

**Interfaces:**
- `RunConfig.gravimetric_trim: bool` (new, `#[serde(default)]`).
- `ActiveRun.gravimetric_trim: bool` (copied from `RunConfig` in `start_run`/`resume`).
- `Engine<T>` new fields: `scale: Option<Box<dyn Transport + Send>>`, `trim_c: f64` (default `1.0`), `scale_state: ScaleState` (default `Normal`), `scale_read_fails: u32`, `scale_ok: bool` (default `true`), `refill_weight_g: Option<f64>`, `refill_at: Option<Timestamp>`, `weight_buffer: Vec<(f64, f64)>`, `last_weight_g: Option<f64>`, `settling_since: Option<Timestamp>`, `manual_refill_mode: bool`, `manual_refill_done_flag: bool`, `last_rate_g_per_min: Option<f64>`, `scale_density_g_per_ml: f64`.
- `pub fn scale_tick(&mut self, now: Timestamp)`.
- `pub fn trigger_refill_mode(&mut self)` / `pub fn trigger_refill_done(&mut self)`.
- `tick()`/`apply_setpoint()`: `target = quantize(active.spec.value_at(elapsed) * self.trim_c, grid)`.

- [ ] **Step 1: Write the failing engine-level tests with a fake scale**

```rust
struct ScriptedScale {
    replies: std::collections::VecDeque<Result<Vec<u8>, TransportError>>,
}
impl ScriptedScale {
    fn new(weights: &[f64]) -> Self {
        let replies = weights.iter().map(|w| Ok(format!("S S {w:>10.1} g\r\n").into_bytes())).collect();
        Self { replies }
    }
}
impl Transport for ScriptedScale {
    fn transaction(&mut self, _req: &[u8]) -> Result<Vec<u8>, TransportError> {
        self.replies.pop_front().unwrap_or(Err(TransportError::Timeout))
    }
}

#[test]
fn gravimetric_trim_multiplies_the_curve_target() {
    let mut e = engine();
    e.attach_scale_for_test(Box::new(ScriptedScale::new(&[1000.0; 50])));
    let cfg = RunConfig { gravimetric_trim: true, ..linear_cfg() };
    e.start_run(cfg, t0()).unwrap();
    e.scale_tick(t0());
    let o = e.tick(at(1)).unwrap();
    assert!(matches!(o, TickOutcome::Applied { .. }));
    assert_eq!(e.trim_c(), 1.0);
}

#[test]
fn scale_failure_freezes_c_and_keeps_the_pump_running() {
    let mut e = engine();
    e.attach_scale_for_test(Box::new(ScriptedScale { replies: Default::default() }));
    let cfg = RunConfig { gravimetric_trim: true, ..linear_cfg() };
    e.start_run(cfg, t0()).unwrap();
    for i in 0..10 {
        e.scale_tick(at(i));
    }
    assert!(!e.status().scale_ok);
    let o = e.tick(at(20)).unwrap();
    assert!(matches!(o, TickOutcome::Applied { .. }));
}
```
Add `pub(crate) fn attach_scale_for_test(&mut self, t: Box<dyn Transport + Send>)` and `pub(crate) fn trim_c(&self) -> f64` under `#[cfg(test)]`.

- [ ] **Step 2: Run, verify they fail.** `cargo test -p fermentool-core --lib engine:: gravimetric` and `scale_failure`, compile errors.

- [ ] **Step 3: Implement.** Change the target computation in `tick()`/`apply_setpoint()`:
```rust
let raw = active.spec.value_at(Duration::from_secs_f64(elapsed_s)) * self.trim_c;
let target = quantize(raw, setpoint_grid(control_var));
```
Implement `scale_tick`:
```rust
pub fn scale_tick(&mut self, now: Timestamp) {
    let wants_trim = self.active.as_ref().is_some_and(|a| a.gravimetric_trim);
    let Some(scale) = self.scale.as_mut() else { return };
    if !wants_trim {
        return;
    }
    let reply = scale
        .transaction(scale::SICS_IMMEDIATE)
        .map_err(scale::ScaleError::from)
        .and_then(|r| scale::parse_sics_weight(&r));

    let weight_g = match reply {
        Ok((w, _stable)) => {
            self.scale_read_fails = 0;
            self.scale_ok = true;
            w
        }
        Err(_) => {
            self.scale_read_fails = self.scale_read_fails.saturating_add(1);
            if self.scale_read_fails >= REOPEN_AFTER_WRITE_FAILS {
                self.scale_ok = false;
            }
            return;
        }
    };

    let prev = self.last_weight_g.unwrap_or(weight_g);
    self.last_weight_g = Some(weight_g);
    let seconds_in_settling = self.settling_since
        .map(|s| now.duration_since(s).as_secs_f64())
        .unwrap_or(0.0);
    let input = trim::StateInput {
        weight_g,
        prev_weight_g: prev,
        recent_variance_g: 0.0, // v1 simplification, documented below
        seconds_in_settling,
        manual_refill_mode: self.manual_refill_mode,
        manual_refill_done: self.manual_refill_done_flag,
    };
    self.manual_refill_done_flag = false;
    let next = trim::next_state(self.scale_state, &input);

    if next == trim::ScaleState::RefillSettling && self.scale_state != trim::ScaleState::RefillSettling {
        self.settling_since = Some(now);
    }
    if next == trim::ScaleState::Normal && self.scale_state != trim::ScaleState::Normal {
        self.refill_weight_g = Some(weight_g);
        self.refill_at = Some(now);
        self.weight_buffer.clear();
    }
    self.scale_state = next;

    if self.scale_state != trim::ScaleState::Normal {
        return;
    }

    let t_refill = self.refill_at.unwrap_or(now);
    let elapsed_since_refill = now.duration_since(t_refill).as_secs_f64().max(0.0);
    self.weight_buffer.push((elapsed_since_refill, weight_g));
    self.last_rate_g_per_min = trim::theil_sen_slope(
        &trim::decimate(&self.weight_buffer, trim::MAX_SLOPE_POINTS)
    ).map(|s| s * 60.0);

    if let (Some(w0), Some(active)) = (self.refill_weight_g, self.active.as_ref()) {
        let density = self.scale_density_g_per_ml;
        let elapsed = now.duration_since(active.started_at).as_secs_f64().max(0.0);
        let refill_elapsed = now.duration_since(t_refill).as_secs_f64().max(0.0);
        let start_elapsed = (elapsed - refill_elapsed).max(0.0);
        let ml_per_unit = self.rpm_to_ml_min.unwrap_or(1.0); // 1.0 when curve is already ml/min
        let mass_theoretical = integrate_curve_mass(&active.spec, start_elapsed, elapsed, density, ml_per_unit) * self.trim_c;
        let mass_measured = w0 - weight_g;
        if mass_theoretical.abs() > 1e-6 {
            let error_frac = (mass_theoretical - mass_measured) / mass_theoretical;
            match trim::update_trim(self.trim_c, error_frac) {
                trim::TrimOutcome::Unchanged => {}
                trim::TrimOutcome::Trimmed { new_c } => self.trim_c = new_c,
                trim::TrimOutcome::Alarm => self.scale_ok = false,
            }
        }
    }
}
```
`integrate_curve_mass(spec, start, end, density, ml_per_unit)`: numeric integration (Simpson's rule, ~50 sub-intervals) of `spec.value_at(t) * ml_per_unit * density` over `[start, end]`, pure, testable on its own against a `Constant` curve's closed form.

`StateInput.recent_variance_g: 0.0` is a documented v1 simplification (no rolling variance of the settling window yet, `REFILL_SETTLE_SECONDS` alone gates `RefillSettling -> Normal`, functionally safe, slightly less adaptive; a follow-up computing it from `weight_buffer`'s tail is a one-function addition once observed on a real refill).

Add `trigger_refill_mode`/`trigger_refill_done` as one-line setters. Add `scale_ok`, `scale_state`, `trim_c`, `rate_g_per_min` to `EngineStatus`.

- [ ] **Step 4: Run, verify pass.** `cargo test -p fermentool-core --lib engine::`, no regressions plus the 2 new tests.

- [ ] **Step 5: Wire `scale_tick` into the control loop.** In `control.rs`, a `next_scale_probe: Option<Instant>` ticking at `LINK_PROBE_INTERVAL` (1 s) whenever the active run has `gravimetric_trim` (add that bool to `ActiveStatus`, needed for the UI in Task 9 anyway).

- [ ] **Step 6: Full suite + commit**

```bash
git add crates/fermentool-core/src/engine/mod.rs crates/fermentool-core/src/control.rs
git commit -m "feat(core): wire the gravimetric trim into the tick loop"
```

---

## Task 8: Persistence across crash-resume

**Goal:** `trim_c`, `scale_state`, and the refill reference survive a restart, via `app_state` (same mechanism `holding` uses), not a schema migration.

**Files:** Modify `crates/fermentool-core/src/engine/mod.rs`

**Interfaces:** `const TRIM_STATE_KEY: &str = "trim_state";`, `struct PersistedTrim { trim_c: f64, scale_state: ScaleState, refill_weight_g: Option<f64>, refill_at: Option<Timestamp> }`, `fn save_trim_state(&self)`, restored in `Engine::new`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn trim_state_survives_a_restart() {
    let db = TempDb::new();
    {
        let mut a = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
        a.attach_scale_for_test(Box::new(ScriptedScale::new(&[1000.0])));
        let cfg = RunConfig { gravimetric_trim: true, ..linear_cfg() };
        a.start_run(cfg, t0()).unwrap();
        a.scale_tick(t0());
        a.set_trim_c_for_test(1.05);
        a.save_trim_state();
    }
    let b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
    assert_eq!(b.trim_c(), 1.05);
}
```

- [ ] **Step 2: Run, verify it fails.** Resets to `1.0`.

- [ ] **Step 3: Implement**

```rust
const TRIM_STATE_KEY: &str = "trim_state";

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedTrim {
    trim_c: f64,
    scale_state: trim::ScaleState,
    refill_weight_g: Option<f64>,
    refill_at: Option<Timestamp>,
}

impl<T: Transport> Engine<T> {
    fn save_trim_state(&self) {
        let p = PersistedTrim {
            trim_c: self.trim_c,
            scale_state: self.scale_state,
            refill_weight_g: self.refill_weight_g,
            refill_at: self.refill_at,
        };
        if let Ok(json) = serde_json::to_string(&p) {
            let _ = self.store.set_state(TRIM_STATE_KEY, &json);
        }
    }
}
```
In `Engine::new`, alongside the existing `holding` restore:
```rust
let persisted_trim = store.get_state(TRIM_STATE_KEY).ok().flatten()
    .and_then(|s| serde_json::from_str::<PersistedTrim>(&s).ok());
let (trim_c, scale_state, refill_weight_g, refill_at) = match persisted_trim {
    Some(p) => (p.trim_c, p.scale_state, p.refill_weight_g, p.refill_at),
    None => (1.0, trim::ScaleState::Normal, None, None),
};
```
Call `self.save_trim_state()` at the end of `scale_tick` whenever `trim_c` or `scale_state` actually changed (cheap before/after guard, at most once per adaptive window in steady state).

- [ ] **Step 4: Run, verify pass.**

- [ ] **Step 5: Full suite + commit**

```bash
git add crates/fermentool-core/src/engine/mod.rs
git commit -m "feat(core): persist the gravimetric trim across a restart"
```

---

## Task 9: API + status for the ongoing trim

**Goal:** `gravimetric_trim` flows from `POST /api/runs` to the engine; `/api/status` exposes `scale_ok`/`scale_state`/`trim_c`/`rate_g_per_min`; manual refill trigger endpoints.

**Files:** Modify `crates/fermentool-core/src/control.rs`, `crates/fermentool-core/src/api.rs`

**Interfaces:** `DaemonStatus` gains `pub scale_ok: bool, pub scale_state: Option<String>, pub trim_c: Option<f64>, pub rate_g_per_min: Option<f64>`; `Command::TriggerRefillMode(oneshot::Sender<()>)`, `Command::TriggerRefillDone(oneshot::Sender<()>)`; routes `POST /api/scale/refill_mode`, `POST /api/scale/refill_done`.

- [ ] **Step 1: Write the failing API tests**

```rust
#[tokio::test]
async fn refill_endpoints_toggle_the_manual_flags() {
    let app = router(test_state());
    let res = app.clone().oneshot(post_json("/api/scale/refill_mode", json!({}))).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = app.oneshot(post_json("/api/scale/refill_done", json!({}))).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn status_reports_no_scale_fields_when_unconfigured() {
    let app = router(test_state());
    let res = app.oneshot(get("/api/status")).await.unwrap();
    let body = body_json(res).await;
    assert!(body["scale_ok"].as_bool().unwrap());
    assert!(body["scale_state"].is_null());
    assert!(body["trim_c"].is_null());
}
```

- [ ] **Step 2: Run, verify they fail.**

- [ ] **Step 3: Implement.** `control.rs`: add the two `Command` variants, handled by calling `engine.trigger_refill_mode()`/`trigger_refill_done()`; extend `DaemonStatus`/`current_status`. `api.rs`:
```rust
async fn scale_refill_mode(State(s): State<AppState>) -> ApiResult<Response> {
    s.control.call(Command::TriggerRefillMode).await.map_err(|_| ApiError::Down)?;
    Ok(Json(json!({ "ok": true })).into_response())
}
async fn scale_refill_done(State(s): State<AppState>) -> ApiResult<Response> {
    s.control.call(Command::TriggerRefillDone).await.map_err(|_| ApiError::Down)?;
    Ok(Json(json!({ "ok": true })).into_response())
}
```

- [ ] **Step 4: Run, verify pass.**

- [ ] **Step 5: Full suite + commit**

```bash
git add crates/fermentool-core/src/control.rs crates/fermentool-core/src/api.rs
git commit -m "feat(core): expose the gravimetric trim over the API"
```

---

## Task 10: UI for the ongoing trim

**Goal:** A checkbox to opt in per run, and a live status readout, following the codebase's existing indicator patterns.

**Files:** Modify `ui/src/routes/NewRun.svelte`, `ui/src/routes/Overview.svelte`

- [ ] **Step 1: `NewRun.svelte`, the opt-in checkbox.** Add `gravimetric_trim: false` to run-config state; `scaleConfigured = cfg?.scale?.path?.length > 0`.
```svelte
{#if scaleConfigured}
  <label class="field check">
    <input type="checkbox" bind:checked={f.gravimetric_trim} />
    <span>Enable gravimetric trim (uses the configured scale to correct the feed rate over time)</span>
  </label>
{/if}
```
Include `gravimetric_trim: f.gravimetric_trim` in the `POST /api/runs` payload.

- [ ] **Step 2: `Overview.svelte`, live status.** Where the existing pump-link indicators render, add, only when `app.status?.scale_state` is non-null:
```svelte
{#if app.status?.scale_state}
  <span class="pumpstat {app.status.scale_ok ? 'ok' : 'bad'}">
    <span class="dot" aria-hidden="true"></span>
    scale: {app.status.scale_state} (c={app.status.trim_c?.toFixed(3)})
  </span>
{/if}
```
Match whichever existing class names the connection bar already uses for tone coloring.

- [ ] **Step 3: Build.** `cd ui && npm run build`.

- [ ] **Step 4: Manual smoke.** With `[scale] path = ""`, confirm the checkbox is hidden; set a `path`, restart, confirm it appears (simulator is fine, no real scale needed).

- [ ] **Step 5: Commit**

```bash
git add ui/src/routes/NewRun.svelte ui/src/routes/Overview.svelte ui/dist
git commit -m "feat(ui): gravimetric trim opt-in and live status"
```

---

## Task 11: Tubing calibration schema

**Goal:** A `tubing_calibrations` table, a `kind` discriminator on `runs` so a calibration burst can reuse the real run machinery without polluting the dosing-run history, and a `tubing_calibration_id` link from a dosing run back to the calibration that seeded its `trim_c`.

**Files:** Create `crates/fermentool-core/src/store/migrations/0002_tubing_calibration.sql`; modify `crates/fermentool-core/src/store/mod.rs`

**Interfaces:**
```rust
pub struct NewCalibration {
    pub tubing_lot_id: String,
    pub tubing_size: String,
    pub control_var: ControlVar,
    pub setpoint: f64,
    pub density_g_per_ml: f64,
    pub run_ids: [i64; 3],
    pub weights_g: [f64; 3],
    pub operator: Option<String>,
    pub note: Option<String>,
}
pub struct CalibrationRow { /* mirrors the table, plus derived measured_i_ml_min, mean_measured_ml_min, cv_pct, c0 */ }
impl Store {
    pub fn insert_calibration(&self, c: &NewCalibration) -> Result<i64>;
    pub fn calibration(&self, id: i64) -> Result<Option<CalibrationRow>>;
    pub fn list_calibrations(&self, tubing_lot_id: Option<&str>, tubing_size: Option<&str>) -> Result<Vec<CalibrationRow>>;
}
```

- [ ] **Step 1: Write the migration**

```sql
ALTER TABLE runs ADD COLUMN kind TEXT NOT NULL DEFAULT 'dosing' CHECK (kind IN ('dosing','calibration'));
ALTER TABLE runs ADD COLUMN tubing_calibration_id INTEGER REFERENCES tubing_calibrations(id);

CREATE TABLE tubing_calibrations (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at            TEXT    NOT NULL,             -- RFC-3339 UTC
    tubing_lot_id         TEXT    NOT NULL,              -- lot/paquet, saisi par l'operateur
    tubing_size           TEXT    NOT NULL,              -- taille/diametre, saisi par l'operateur
    control_var           TEXT    NOT NULL CHECK (control_var IN ('rpm','ml_min')),
    setpoint               REAL   NOT NULL,              -- consigne commandee pendant les 3 salves
    density_g_per_ml       REAL   NOT NULL,
    run_1_id INTEGER NOT NULL REFERENCES runs(id),
    run_2_id INTEGER NOT NULL REFERENCES runs(id),
    run_3_id INTEGER NOT NULL REFERENCES runs(id),
    weight_1_g REAL NOT NULL,
    weight_2_g REAL NOT NULL,
    weight_3_g REAL NOT NULL,
    measured_1_ml_min REAL NOT NULL,   -- poids / (densite * duree reelle du run)
    measured_2_ml_min REAL NOT NULL,
    measured_3_ml_min REAL NOT NULL,
    mean_measured_ml_min   REAL   NOT NULL,
    cv_pct                 REAL   NOT NULL,
    c0                     REAL   NOT NULL,              -- = setpoint / mean_measured, meme convention que error_frac
    operator                TEXT,
    note                    TEXT
);
CREATE INDEX ix_tubing_cal_lot_size ON tubing_calibrations(tubing_lot_id, tubing_size, created_at);
```
`c0`'s convention matches `trim.rs`'s `error_frac` exactly (positive = under-delivering = speed up), so it plugs directly into `Engine.trim_c` at run start with no conversion. `run_1_id`/`run_2_id`/`run_3_id` give full traceability back to the raw tick data of each calibration burst, not just the summary numbers, useful for a batch record.

- [ ] **Step 2: Run existing store tests, verify the migration applies cleanly.** `cargo test -p fermentool-core --lib store::`.

- [ ] **Step 3: Implement `NewCalibration`/`CalibrationRow`/`insert_calibration`/`calibration`/`list_calibrations`** in `store/mod.rs`, mirroring `NewRun`/`RunRow`/`insert_run`'s existing shape. The server computes `measured_i_ml_min`, `mean_measured_ml_min`, `cv_pct`, `c0` itself from the 3 `(run_id, weight_g)` pairs (each run's actual elapsed time comes from that run's own `started_at`/`ended_at`), never trusting a client-computed value for a traceable record.

- [ ] **Step 4: Write and run store tests**

```rust
#[test]
fn insert_and_fetch_a_calibration_round_trips() { /* insert, calibration(id), assert fields */ }

#[test]
fn list_calibrations_filters_by_lot_and_size() { /* two lots, two sizes, assert filtering */ }
```
`cargo test -p fermentool-core --lib store::`.

- [ ] **Step 5: Commit**

```bash
git add crates/fermentool-core/src/store/migrations/0002_tubing_calibration.sql crates/fermentool-core/src/store/mod.rs
git commit -m "feat(core): tubing_calibrations schema"
```

---

## Task 12: `compute_calibration` (pure) + engine gating

**Goal:** The pure math behind a calibration record, plus the rule that gravimetric trim in rpm mode cannot start without one.

**Files:** Modify `crates/fermentool-core/src/trim.rs`, `crates/fermentool-core/src/engine/mod.rs`

**Interfaces:**
- `pub struct CalibrationInput { pub setpoint: f64, pub density_g_per_ml: f64, pub samples: [(f64 /* duration_min */, f64 /* weight_g */); 3] }`
- `pub struct CalibrationResult { pub measured_ml_min: [f64; 3], pub mean_measured_ml_min: f64, pub cv_pct: f64, pub c0: f64 }`
- `pub fn compute_calibration(input: &CalibrationInput) -> CalibrationResult`
- `pub const CALIBRATION_CV_WARN_PCT: f64 = 5.0;` (warn, non-blocking, per the approved design)
- New `EngineError::GravimetricTrimNeedsCalibration` (message: "Calibration du tuyau requise en mode rpm pour le trim gravimetrique"; hint: explains `integrate_curve_mass` has no rpm-to-mL/min relationship without it, per the project's `EngineError::detail()` convention, `Display` stays short, the explanation goes in `hint()`).

- [ ] **Step 1: Write the failing tests**

```rust
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
        // commanded 10 mL/min, measured ~9 mL/min -> pump needs to speed up.
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
}
```

- [ ] **Step 2: Run, verify they fail.**

- [ ] **Step 3: Implement**

```rust
pub const CALIBRATION_CV_WARN_PCT: f64 = 5.0;

pub struct CalibrationInput {
    pub setpoint: f64,
    pub density_g_per_ml: f64,
    pub samples: [(f64, f64); 3], // (duration_min, weight_g)
}

pub struct CalibrationResult {
    pub measured_ml_min: [f64; 3],
    pub mean_measured_ml_min: f64,
    pub cv_pct: f64,
    pub c0: f64,
}

pub fn compute_calibration(input: &CalibrationInput) -> CalibrationResult {
    let measured_ml_min = input.samples.map(|(dur_min, weight_g)| {
        weight_g / (input.density_g_per_ml * dur_min)
    });
    let mean = measured_ml_min.iter().sum::<f64>() / 3.0;
    let variance = measured_ml_min.iter().map(|m| (m - mean).powi(2)).sum::<f64>() / 3.0;
    let cv_pct = if mean.abs() > 1e-9 { 100.0 * variance.sqrt() / mean } else { 0.0 };
    let c0 = if mean.abs() > 1e-9 { input.setpoint / mean } else { 1.0 };
    CalibrationResult { measured_ml_min, mean_measured_ml_min: mean, cv_pct, c0 }
}
```
In `engine/mod.rs`, `start_run` (and `resume`, same check): if `cfg.control_var == ControlVar::Rpm && cfg.gravimetric_trim && cfg.tubing_calibration_id.is_none()`, return `Err(EngineError::GravimetricTrimNeedsCalibration)` before any state mutation. When a `tubing_calibration_id` is given, `Engine.trim_c` seeds from that calibration's `c0` (looked up via `self.store.calibration(id)`) instead of `1.0`, and for rpm-mode runs, `Engine.rpm_to_ml_min = Some(calibration.mean_measured_ml_min / calibration.setpoint)` (used by `integrate_curve_mass` in Task 7). For ml/min-mode runs, `rpm_to_ml_min` stays `None` (curve output is already volumetric, `integrate_curve_mass` treats the missing factor as `1.0`).

- [ ] **Step 4: Run, verify pass.** `cargo test -p fermentool-core --lib trim::calibration_tests`.

- [ ] **Step 5: Engine-level tests for the gate**

```rust
#[test]
fn rpm_mode_trim_without_a_calibration_is_refused() {
    let mut e = engine();
    let cfg = RunConfig { control_var: ControlVar::Rpm, gravimetric_trim: true, tubing_calibration_id: None, ..linear_cfg() };
    let err = e.start_run(cfg, t0()).unwrap_err();
    assert!(matches!(err, EngineError::GravimetricTrimNeedsCalibration));
}

#[test]
fn ml_min_mode_trim_without_a_calibration_still_starts() {
    let mut e = engine();
    let cfg = RunConfig { control_var: ControlVar::MlMin, gravimetric_trim: true, tubing_calibration_id: None, ..linear_cfg() };
    assert!(e.start_run(cfg, t0()).is_ok());
    assert_eq!(e.trim_c(), 1.0);
}
```
`cargo test -p fermentool-core --lib engine::`.

- [ ] **Step 6: Commit**

```bash
git add crates/fermentool-core/src/trim.rs crates/fermentool-core/src/engine/mod.rs
git commit -m "feat(core): tubing calibration math + mandatory gate in rpm mode"
```

---

## Task 13: Calibration API

**Goal:** Drive a calibration burst as an ordinary `kind='calibration'` run, then finalize the 3 replicates into a `tubing_calibrations` row; list existing calibrations for the `NewRun.svelte` picker; persist an in-progress draft across a page refresh or daemon restart.

**Files:** Modify `crates/fermentool-core/src/api.rs`, `crates/fermentool-core/src/control.rs`

**Interfaces:**
- `POST /api/runs` already accepts `kind` (default `'dosing'`); calibration bursts pass `kind: 'calibration'`, `curve: Constant`, `gravimetric_trim: false`. No new start/stop endpoint, the existing ones are reused as-is: this is the whole point of Task 1's approach.
- `POST /api/calibrations` body: `{ tubing_lot_id, tubing_size, control_var, setpoint, density_g_per_ml, run_ids: [i64;3], weights_g: [f64;3], operator?, note? }` -> `201 { id, ...CalibrationResult }`. Recomputes everything server-side from the 3 run rows' real `started_at`/`ended_at` (never trusts a client-supplied duration).
- `GET /api/calibrations?lot_id=&size=` -> `200 [CalibrationRow, ...]`, newest first.
- `POST /api/calibrations/draft` / `GET /api/calibrations/draft` / `DELETE /api/calibrations/draft`: the in-progress session (lot, size, setpoint, density, which run ids and weights have been collected so far), stored as one JSON blob under a single `app_state` key (`calibration_draft`), cleared once `POST /api/calibrations` finalizes it. A single key is sufficient because only one calibration session (like only one run) is ever in progress at a time.

- [ ] **Step 1: Write the failing API tests**

```rust
#[tokio::test]
async fn calibration_round_trip() {
    let app = router(test_state());
    // three short calibration runs already created and stopped via the
    // existing /api/runs endpoints in test setup (kind=calibration)...
    let res = app.oneshot(post_json("/api/calibrations", json!({
        "tubing_lot_id": "LOT-42", "tubing_size": "1.6mm",
        "control_var": "ml_min", "setpoint": 10.0, "density_g_per_ml": 1.0,
        "run_ids": [r1, r2, r3], "weights_g": [50.0, 50.0, 50.0],
    }))).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body = body_json(res).await;
    assert!((body["c0"].as_f64().unwrap() - 1.0).abs() < 1e-6);
}

#[tokio::test]
async fn draft_persists_across_a_reload() {
    let app = router(test_state());
    app.clone().oneshot(post_json("/api/calibrations/draft", json!({"tubing_lot_id": "LOT-1", "tubing_size": "1.6mm"}))).await.unwrap();
    let res = app.oneshot(get("/api/calibrations/draft")).await.unwrap();
    let body = body_json(res).await;
    assert_eq!(body["tubing_lot_id"], "LOT-1");
}
```

- [ ] **Step 2: Run, verify they fail.**

- [ ] **Step 3: Implement** the four routes in `api.rs`, wiring to `store::insert_calibration`/`list_calibrations` and `store::set_state`/`get_state` for the draft.

- [ ] **Step 4: Run, verify pass.**

- [ ] **Step 5: Full suite + commit**

```bash
git add crates/fermentool-core/src/api.rs crates/fermentool-core/src/control.rs
git commit -m "feat(core): tubing calibration API + draft persistence"
```

---

## Task 14: Calibration UI

**Goal:** `TubingCalibration.svelte`, and the calibration picker in `NewRun.svelte`.

**Files:** Create `ui/src/routes/TubingCalibration.svelte`; modify `ui/src/routes/NewRun.svelte`, router config for the new route.

- [ ] **Step 1: `TubingCalibration.svelte`**
  - Form: lot ID, taille, `control_var`, consigne, densite, duree cible (minuteur seulement, jamais stockee comme donnee de mesure).
  - Bouton "Demarrer salve N/3" -> `POST /api/runs` (`kind: 'calibration'`, `curve: Constant{value: setpoint}`, `gravimetric_trim: false`) + `POST /api/calibrations/draft` to save progress. Client-side countdown from the target duration.
  - "Arreter et peser" -> `POST /api/runs/:id/stop` (existing), then a weight input (g), saved into the draft.
  - After 3 replicates: `GET`-equivalent local computation for immediate feedback is fine for the preview (`compute_calibration`'s logic re-implemented client-side purely for display, exactly like the curve preview already does for `CurveSpec`), but the **authoritative** numbers always come back from `POST /api/calibrations`'s response. CV% styled red above `CALIBRATION_CV_WARN_PCT`, `c0` styled red outside `[0.80, 1.25]`, both non-blocking with an explanatory note (mirrors the trim's own alarm-vs-freeze philosophy: warn, let a human decide).
  - "Enregistrer la calibration" -> `POST /api/calibrations`, then `DELETE /api/calibrations/draft`.
  - On mount: `GET /api/calibrations/draft`, if present, resume mid-session instead of starting blank (survives a refresh).

- [ ] **Step 2: `NewRun.svelte`, the calibration picker.** When `gravimetric_trim` is checked: text inputs for lot ID / taille, `GET /api/calibrations?lot_id=&size=` on change, a dropdown of matches (date, CV%, `c0`) sorted newest first. Selecting one sets `f.tubing_calibration_id`. No match found: show the rpm-mode-blocking warning inline ("aucune calibration pour ce lot/cette taille, le trim ne pourra pas demarrer en mode rpm sans en faire une d'abord", with a link to `TubingCalibration.svelte`) or, in ml/min mode, a softer "demarrage sans calibration, trim_c commencera a 1.0" note. Include `tubing_calibration_id: f.tubing_calibration_id` in the run payload.

- [ ] **Step 3: Build.** `cd ui && npm run build`.

- [ ] **Step 4: Manual smoke.** Full calibration cycle against the simulator (3 fake replicates, typed-in weights), confirm the record appears in the `NewRun.svelte` dropdown afterward, confirm starting an rpm-mode trimmed run without a calibration is refused with the right message.

- [ ] **Step 5: Commit**

```bash
git add ui/src/routes/TubingCalibration.svelte ui/src/routes/NewRun.svelte ui/dist
git commit -m "feat(ui): tubing calibration workflow + picker in NewRun"
```

---

## Self-Review

**1. Spec coverage (ongoing trim, Tasks 1-10):** unchanged from the original plan's self-review, every design section (balance link, config, state machine, adaptive window, cumulative mass balance, trim bounds, persistence, scale-unreachable safety, RunConfig/API/UI) maps to a task above. `theil_sen_slope`/`decimate` (Task 4) are used in Task 7's `scale_tick` for the diagnostic `rate_g_per_min` field, not to drive `c` itself (the cumulative mass balance does that), matching the approved design.

**2. Spec coverage (tubing calibration, Tasks 11-14):**
| Decision made with the user | Task |
|---|---|
| Reuse the run engine (Constant curve + `kind` column), not a separate jog endpoint | Task 11 (schema), Task 13 (API reuses `/api/runs`) |
| 3 manually-weighed replicates at one setpoint, real elapsed time from the run's own clock | Task 12 (`compute_calibration`), Task 13 |
| CV% <= 5%, `c0` in `[0.80, 1.25]`: warn, never block | Task 12, Task 14 |
| Calibration reusable by tubing lot + size, not one-shot per run | Task 11 (index on lot+size), Task 14 (picker) |
| Mandatory in rpm mode, recommended in ml/min mode | Task 12 (`EngineError::GravimetricTrimNeedsCalibration`), Task 14 (UI messaging) |
| Traceable record, exportable, tubing lot linked | Task 11 (schema with `run_*_id` back-references), CSV export is a client-side rendering of `GET /api/calibrations`'s response, no new backend format needed (YAGNI) |
| Draft survives a refresh / restart | Task 13 (`app_state`-backed draft) |
| `synthese_chemostat` doc's "conditions d'apprentissage" checklist | Task 5 (already implied by "state is `Normal`", no separate checklist needed) |
| `synthese_chemostat` doc's data-logging column list | Covered by the existing `ticks` table (`target`, `written_ok`, `readback`) plus the new `tubing_calibrations` columns; no separate logging table needed |

**3. Placeholder scan:** `StateInput.recent_variance_g: 0.0` (Task 7) is the one deliberate v1 simplification, stated with its consequence, not a bare TODO. Everything else in this plan is either copied verbatim from the already-reviewed original plan or newly written against decisions the user made explicitly in conversation (see the "Decision made with the user" column above).

**4. Type consistency:** `ScaleState` used identically across Tasks 5, 7, 8. `TrimOutcome` matched exactly in Task 7. `CalibrationResult`'s fields used identically in Task 11's `CalibrationRow`, Task 12's tests, and Task 13's API response. `EngineError::GravimetricTrimNeedsCalibration` follows the project's existing `message`/`hint` split (`CLAUDE.md`), never put explanation text in `Display`.

**5. Scope check:** this is one implementation plan for two features that ship together (the second exists to fix a real gap in the first), not two independently shippable units. Tasks 1-10 are usable on their own if calibration is deferred (ml/min mode, `trim_c` starts at 1.0); Tasks 11-14 have no independent value without 1-10. Sequenced accordingly, in order, above.
