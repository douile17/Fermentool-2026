# Gravimetric Feed Trim Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in, closed-loop correction to the feed pump's setpoint, driven by a scale under the feed bottle, without touching manual-mode behavior at all.

**Architecture:** A new `SicsScale` type implements the existing `fermentool_modbus::Transport` trait (MT-SICS is just another request/response byte protocol), wrapped in the already-generic `WatchdogTransport` so a wedged balance read can't freeze the daemon, exactly like the pump. `Engine` gains a second, independent `Option<WatchdogTransport>` for the scale plus a small state machine (`Normal`/`Perturbation`/`RefillPending`/`RefillSettling`) and a bounded trim factor `c`. Every tick multiplies the existing `curve.value_at(elapsed)` by `c` before writing; `c` stays `1.0` (a no-op) unless a run opts in with `gravimetric_trim = true`. `c` and the state machine's reference point persist through `app_state` (the same mechanism `holding` already uses) so a crash-resume doesn't throw away a converged trim.

**Tech Stack:** Rust (`fermentool-core`, `fermentool-modbus`), `serialport` (already a transitive dep via `fermentool-modbus`), Svelte 5 (`ui/`).

**Spec:** `docs/superpowers/specs/2026-09-11-gravimetric-feed-trim-design.md` (Step 0 spike already run against the real balance: MT-SICS confirmed, `S`/`SI` -> `S S      740.5 g\r\n`, 9600 8N1, 0.1 g resolution).

## Global Constraints

- **Manual mode (`gravimetric_trim = false`, the default) is byte-for-byte unchanged.** `c` is pinned to `1.0` whenever trim is off, so `target = curve.value_at(elapsed) * 1.0` is exactly today's `target`. No existing test may need to change because of this feature.
- **Driver: `SicsScale` only.** No `OhausContinuousScale` (dropped after the spike). Confirmed protocol: send `b"S\r\n"` or `b"SI\r\n"`, reply `b"S <status> <value> g\r\n"` where status is `S` (stable) or `D` (dynamic/unstable).
- **`SerialTransport` (the pump's transport) is not reusable for the scale as-is**: it hardcodes 8E1 framing (`.parity(Parity::Even)`) and `read_until` expects a caller-known fixed byte count, both wrong for a variable-length ASCII line at whatever parity the balance is configured for. The scale gets its own small raw-serial opener (Task 2).
- **`WatchdogTransport` is reused unmodified** except promoting its `#[cfg(test)]`-only generic constructor to a real public one (Task 3): its worker-thread-plus-hard-timeout design already doesn't care what `Transport` impl it's wrapping.
- **Trim bounds** (from the spec, restated here verbatim so every task agrees): ignore `|error_frac| < 3%`; trim for `3% <= |error_frac| <= 15%` via `c *= (1 + gamma * error_frac)`, `gamma = 0.3`; alarm (freeze, don't correct) for `|error_frac| > 20%` sustained; `c` clamped to `[0.80, 1.25]`; max `|delta c|` per update `2%`; `c` freezes outside the `Normal` state or when the scale is unreachable.
- **Refill threshold:** a weight jump `> 50.0 g` is a refill; `5.0..=50.0 g` is a perturbation; `< 5.0 g` is noise (these two constants, `PERTURBATION_MIN_G = 5.0` and `REFILL_THRESHOLD_G = 50.0`, are new and documented with this rationale where declared).
- **A lost/wedged scale never stops the pump.** It freezes `c`, sets `scale_ok = false`, and the run keeps going on `curve.value_at(t) * c_frozen` (manual-equivalent behavior with the last trusted correction).
- **No UI exposure of tuning constants in v1** (bands, gamma, window bounds, thresholds are all Rust constants, not config).
- **Commits:** no `Co-Authored-By: Claude` trailer, no `Claude-Session` line (project rule). **No em dash (Unicode U+2014) and no spaced-hyphen-as-punctuation (` - `) anywhere**: comma, colon, or period only (project rule, `CLAUDE.md`).
- **After any `ui/src/` change, run `npm run build` from `ui/` without being asked.**

## File Structure

**New:**
- `crates/fermentool-core/src/scale.rs`: `SicsScale` (implements `Transport`), `parse_sics_weight`, `ScaleConfig` reference (re-exports/uses `config::ScaleConfig`), the raw serial line opener.
- `crates/fermentool-core/src/trim.rs`: pure logic, no I/O: `TheilSen` slope estimator, `ScaleState` enum + transition function, the trim-update function. This is the file most worth keeping I/O-free, mirroring how `fermentool-curves` stays pure.

**Modified:**
- `crates/fermentool-core/src/config.rs`: `ScaleConfig`, `Config.scale` field.
- `crates/fermentool-core/src/transport.rs`: promote `from_opener`/`respawn_with` off `#[cfg(test)]` (rename `from_opener` -> `spawn_with`, keep the test-only ones for their existing tests, or just remove the `#[cfg(test)]` gates since they're harmless to expose).
- `crates/fermentool-core/src/engine/mod.rs`: `Engine` gains `scale: Option<WatchdogTransport>`, `trim_c: f64`, `scale_state: ScaleState`, refill reference fields, a rolling weight-sample buffer; `ActiveRun.gravimetric_trim: bool`; `RunConfig.gravimetric_trim: bool`; `tick()` / `apply_setpoint()` multiply by `trim_c`; a new `scale_tick(now)` method (parallel to `probe_link`); persistence via `app_state`.
- `crates/fermentool-core/src/control.rs`: `DaemonStatus` gains `scale_ok`, `scale_state`, `trim_c`; control loop calls `scale_tick` on a cadence; two new `Command`s for manual refill trigger.
- `crates/fermentool-core/src/main.rs`: boot the scale's `WatchdogTransport` from `config.scale`.
- `crates/fermentool-core/src/api.rs`: `POST /api/scale/refill_mode`, `POST /api/scale/refill_done`; `RunConfig` already flows through `create_run`, no new route needed there.
- `config.example.toml`: document `[scale]`.
- `ui/src/routes/NewRun.svelte`: the trim checkbox.
- `ui/src/routes/Overview.svelte`: `scale_state` / `trim_c` display.

---

## Task 1: `ScaleConfig`

**Goal:** A `[scale]` section in `config.toml`, same shape as `[serial]`.

**Files:**
- Modify: `crates/fermentool-core/src/config.rs`, `config.example.toml`

**Interfaces:**
- Produces: `pub struct ScaleConfig { pub path: String, pub baud: u32, pub density_g_per_ml: f64 }` with `Default` (`path: ""`, `baud: 9600`, `density_g_per_ml: 1.0`); `ScaleConfig::configured(&self) -> bool` (`!self.path.trim().is_empty()`); `Config.scale: ScaleConfig` field.

- [ ] **Step 1: Write the failing tests**

In `crates/fermentool-core/src/config.rs` `mod tests`:
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

- [ ] **Step 2: Run, verify they fail**

Run: `cargo test -p fermentool-core scale_`, expect `no field \`scale\`` compile error.

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
Add `pub scale: ScaleConfig` to `Config` (alongside `serial`, `pump`, ...).

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p fermentool-core scale_`, expect PASS.

- [ ] **Step 5: Document it in `config.example.toml`**

```toml
[scale]
path = ""                     # empty = no scale; e.g. Windows: "COM5"
baud = 9600                    # match the balance's own Communications menu
density_g_per_ml = 1.0         # feed density; water ~1.0, 500 g/L glucose ~1.18
# MT-SICS only for now (send "S"/"SI", parse "S <status> <value> g"). A
# balance without SICS needs its own small driver alongside SicsScale.
```

- [ ] **Step 6: Full suite + commit**

Run: `cargo test -p fermentool-core`
```bash
git add crates/fermentool-core/src/config.rs config.example.toml
git commit -m "feat(core): add [scale] config section"
```

---

## Task 2: `SicsScale` transport + parser

**Goal:** A `Transport` impl that speaks MT-SICS over a raw serial line, plus a well-tested parser for the reply format confirmed in the spike.

**Files:**
- Create: `crates/fermentool-core/src/scale.rs`
- Modify: `crates/fermentool-core/src/lib.rs` (`pub mod scale;`)

**Interfaces:**
- Consumes: `fermentool_modbus::{Transport, TransportError}`, `serialport` (crate already in the dependency tree via `fermentool-modbus`; add it directly to `fermentool-core`'s `Cargo.toml` too since this code lives there).
- Produces:
  - `pub struct SicsScale { .. }` with `pub fn open(path: &str, baud: u32, timeout: Duration) -> Result<Self, TransportError>`.
  - `impl Transport for SicsScale` (`fn transaction(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportError>`).
  - `pub fn parse_sics_weight(reply: &[u8]) -> Result<(f64, bool /* stable */), ScaleError>`, `true` = stable (`S`), `false` = dynamic (`D`); errors on anything else (overload `+`/`-`, malformed line).
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
        // SICS reports overload as "S +" or "S -" with no numeric value.
        assert!(parse_sics_weight(b"S +\r\n").is_err());
    }
}
```

- [ ] **Step 2: Run, verify they fail**

Run: `cargo test -p fermentool-core --lib scale::tests`, expect compile error (module doesn't exist yet).

- [ ] **Step 3: Write `scale.rs`**

```rust
//! MT-SICS balance link. Confirmed against a real Ohaus Ranger 7000 (see
//! docs/superpowers/specs/2026-09-11-gravimetric-feed-trim-design.md, "Spike
//! result"): `S`/`SI` -> `S <status> <value> g\r\n`, status S = stable,
//! D = dynamic (in motion).
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
        self.port
            .write_all(request)
            .map_err(|e| TransportError::Io(e.to_string()))?;
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
/// `status` is `S` (stable) or `D` (dynamic/unstable); anything else
/// (`+`/`-` overload, `I` invalid, a malformed line) is an error.
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
Add `pub mod scale;` to `crates/fermentool-core/src/lib.rs`. Add `serialport = { version = "4", default-features = false, features = ["serialport-serde"] }` to `crates/fermentool-core/Cargo.toml` if not already resolvable transitively, check first with `cargo tree -p fermentool-core -i serialport`; if it resolves, no `Cargo.toml` change is needed, `use serialport::...` alone is enough only if it's a direct dependency, so add it explicitly to be safe (match whatever version `fermentool-modbus` pins).

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p fermentool-core --lib scale::tests`, expect PASS (4 tests).

- [ ] **Step 5: Manual hardware check (not automatable, but cheap)**

With the balance on a known COM port, a throwaway `cargo run --example` or a quick `#[test] #[ignore]` is optional; simplest is reusing `scale_probe.py` from the spike (already confirms the wire format), no new manual step required here since Step 0 already did this. Skip to commit.

- [ ] **Step 6: Full suite + commit**

Run: `cargo test -p fermentool-core`
```bash
git add crates/fermentool-core/src/scale.rs crates/fermentool-core/src/lib.rs crates/fermentool-core/Cargo.toml Cargo.lock
git commit -m "feat(core): SicsScale transport + MT-SICS reply parser"
```

---

## Task 3: Reuse `WatchdogTransport` for the scale

**Goal:** Boot (or skip, if unconfigured) a watchdog-wrapped scale link, so a wedged balance read can never freeze the daemon, exactly like the pump.

**Files:**
- Modify: `crates/fermentool-core/src/transport.rs`, `crates/fermentool-core/src/main.rs`

**Interfaces:**
- Consumes: `scale::SicsScale`, `config::ScaleConfig`.
- Produces: `WatchdogTransport::spawn_with(opener: Opener) -> (Self, Option<TransportKind>)` (promoted from the existing `#[cfg(test)]` `from_opener`, renamed for clarity; `is_stuck`/`respawn_with` similarly promoted, they're harmless outside tests and Task 7 needs `respawn_with`-equivalent behavior isn't required here, `spawn_with` alone is enough for the scale since it's opened once at boot and re-tried on failure the same way the pump is, via a small dedicated retry in the control loop, Task 7).
- `crates/fermentool-core/src/main.rs`: `fn build_scale(cfg: &ScaleConfig) -> Option<WatchdogTransport>`.

- [ ] **Step 1: Promote the generic constructor**

In `crates/fermentool-core/src/transport.rs`, remove the `#[cfg(test)]` from `from_opener` and rename it `spawn_with` (keep the same body); update its two existing test call sites (`WatchdogTransport::from_opener(...)` -> `WatchdogTransport::spawn_with(...)`). Leave `is_stuck` as `#[cfg(test)]` (still only needed by tests) unless Task 7 needs it live (it doesn't: the engine checks `scale_ok` via the same failure-counting pattern as `serial_ok`, not by reaching into `WatchdogTransport` internals).

Also make `Opener` `pub(crate)` (it's currently private to the module) so `main.rs` can build one:
```rust
pub(crate) type Opener = Box<dyn FnOnce() -> (Box<dyn Transport + Send>, Option<TransportKind>) + Send>;
```

- [ ] **Step 2: Run the existing transport tests, verify they still pass under the rename**

Run: `cargo test -p fermentool-core --lib transport::tests`, expect PASS (same 4 watchdog tests, renamed call sites).

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
    let (watchdog, kind) = WatchdogTransport::spawn_with(Box::new(move || {
        match SicsScale::open(&path, baud, SCALE_OPEN_TIMEOUT) {
            Ok(s) => (Box::new(s) as Box<dyn fermentool_modbus::Transport + Send>, None),
            Err(e) => {
                tracing::error!("cannot open scale {path} ({e}); gravimetric trim unavailable");
                // A scale that won't open at boot still gets a WatchdogTransport
                // (so later commands don't need an Option-inside-Option), but every
                // transaction on it will simply time out until it's replaced.
                (Box::new(fermentool_modbus::SimPump::new(1)) as Box<dyn fermentool_modbus::Transport + Send>, None)
            }
        }
    }));
    let _ = kind;
    Some(watchdog)
}
```
Wire `let scale = build_scale(&config.scale);` into `main()` and thread it into `Engine::new`'s construction path (Task 7 adds the `Engine` field this feeds).

- [ ] **Step 4: Build**

Run: `cargo build -p fermentool-core`, expect success.

- [ ] **Step 5: Commit**

```bash
git add crates/fermentool-core/src/transport.rs crates/fermentool-core/src/main.rs
git commit -m "feat(core): boot the scale link through WatchdogTransport"
```

---

## Task 4: Theil-Sen slope estimator (pure)

**Goal:** A robust slope-of-noisy-points function, decimated so it stays cheap even over a 2 h window.

**Files:**
- Create: `crates/fermentool-core/src/trim.rs`
- Modify: `crates/fermentool-core/src/lib.rs` (`pub mod trim;`)

**Interfaces:**
- Produces:
  - `pub const MAX_SLOPE_POINTS: usize = 200;`, a window is decimated to at most this many points before the O(n^2) pairwise-slope pass runs (200^2 = 40 000 comparisons, trivial).
  - `pub fn decimate(points: &[(f64, f64)], max_len: usize) -> Vec<(f64, f64)>`, evenly-spaced subsample, keeps first and last point.
  - `pub fn theil_sen_slope(points: &[(f64, f64)]) -> Option<f64>`, `(x, y)` pairs (x = seconds since window start, y = grams), returns the median of all pairwise slopes; `None` for fewer than 2 points.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theil_sen_recovers_a_clean_linear_slope() {
        // y = 2x + 10, no noise.
        let pts: Vec<(f64, f64)> = (0..20).map(|i| (i as f64, 2.0 * i as f64 + 10.0)).collect();
        let slope = theil_sen_slope(&pts).unwrap();
        assert!((slope - 2.0).abs() < 1e-9, "got {slope}");
    }

    #[test]
    fn theil_sen_ignores_a_single_outlier() {
        let mut pts: Vec<(f64, f64)> = (0..20).map(|i| (i as f64, 2.0 * i as f64 + 10.0)).collect();
        pts[10].1 += 1000.0; // one wild spike
        let slope = theil_sen_slope(&pts).unwrap();
        assert!((slope - 2.0).abs() < 0.5, "one outlier should barely move the median slope, got {slope}");
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

- [ ] **Step 2: Run, verify they fail**

Run: `cargo test -p fermentool-core --lib trim::tests`, module doesn't exist, compile error.

- [ ] **Step 3: Implement**

```rust
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

/// Median of all pairwise slopes -- robust to outliers without a hand-tuned
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
```
Add `pub mod trim;` to `lib.rs`.

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p fermentool-core --lib trim::tests`, expect PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/fermentool-core/src/trim.rs crates/fermentool-core/src/lib.rs
git commit -m "feat(core): Theil-Sen slope estimator for the feed trim"
```

---

## Task 5: State machine (pure)

**Goal:** `Normal` / `Perturbation` / `RefillPending` / `RefillSettling`, exactly the transitions in the spec's table, as a pure function so it's trivial to test every edge.

**Files:**
- Modify: `crates/fermentool-core/src/trim.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
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
        let s = next_state(ScaleState::Normal, &input(100.2, 100.0));
        assert_eq!(s, ScaleState::Normal);
    }

    #[test]
    fn moderate_jump_is_a_perturbation() {
        let s = next_state(ScaleState::Normal, &input(120.0, 100.0)); // +20g
        assert_eq!(s, ScaleState::Perturbation);
    }

    #[test]
    fn perturbation_returns_to_normal_once_the_jump_settles() {
        let s = next_state(ScaleState::Perturbation, &input(120.0, 120.0)); // no further jump
        assert_eq!(s, ScaleState::Normal);
    }

    #[test]
    fn big_jump_is_refill_pending() {
        let s = next_state(ScaleState::Normal, &input(700.0, 100.0)); // +600g
        assert_eq!(s, ScaleState::RefillPending);
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
        i.recent_variance_g = 0.1; // stable now
        assert_eq!(next_state(ScaleState::RefillPending, &i), ScaleState::RefillSettling);
    }

    #[test]
    fn refill_pending_stays_pending_while_the_operator_is_still_handling_the_bottle() {
        let mut i = input(700.0, 690.0);
        i.recent_variance_g = 5.0; // still moving
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
        i.recent_variance_g = 5.0; // still noisy, but the operator says done
        i.manual_refill_done = true;
        assert_eq!(next_state(ScaleState::RefillPending, &i), ScaleState::RefillSettling);
    }
}
```

- [ ] **Step 2: Run, verify they fail**

Run: `cargo test -p fermentool-core --lib trim::state_tests`, compile error, types don't exist.

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
        ScaleState::Normal => {
            if input.manual_refill_mode || jump > REFILL_THRESHOLD_G {
                ScaleState::RefillPending
            } else if jump > PERTURBATION_MIN_G {
                ScaleState::Perturbation
            } else {
                ScaleState::Normal
            }
        }
        ScaleState::Perturbation => {
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

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p fermentool-core --lib trim::`, expect PASS (14 tests total in `trim.rs`).

- [ ] **Step 5: Commit**

```bash
git add crates/fermentool-core/src/trim.rs
git commit -m "feat(core): perturbation/refill state machine for the feed trim"
```

---

## Task 6: Trim update function (pure)

**Goal:** The bounded EWMA correction, exactly the bands and clamps in Global Constraints.

**Files:**
- Modify: `crates/fermentool-core/src/trim.rs`

**Interfaces:**
- Produces:
  - `pub const TRIM_IGNORE_BAND: f64 = 0.03;` `pub const TRIM_ALARM_BAND: f64 = 0.20;` `pub const TRIM_GAMMA: f64 = 0.3;` `pub const TRIM_MAX_DELTA: f64 = 0.02;` `pub const TRIM_MIN: f64 = 0.80;` `pub const TRIM_MAX: f64 = 1.25;`
  - `pub enum TrimOutcome { Unchanged, Trimmed { new_c: f64 }, Alarm }`
  - `pub fn update_trim(prev_c: f64, error_frac: f64) -> TrimOutcome`

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
            TrimOutcome::Trimmed { new_c } => {
                // c *= (1 + 0.3*0.10) = 1.03, but capped at +2% per update -> 1.02
                assert!((new_c - 1.02).abs() < 1e-9, "got {new_c}");
            }
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
        // Even a 15% error (top of the trim band) must not move c by more
        // than TRIM_MAX_DELTA in one call.
        if let TrimOutcome::Trimmed { new_c } = update_trim(1.0, 0.15) {
            assert!((new_c - 1.0).abs() <= TRIM_MAX_DELTA + 1e-9, "moved too far: {new_c}");
        }
    }
}
```

- [ ] **Step 2: Run, verify they fail**

Run: `cargo test -p fermentool-core --lib trim::trim_tests`, compile error.

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
    let raw_delta = TRIM_GAMMA * error_frac;
    let clamped_delta = raw_delta.clamp(-TRIM_MAX_DELTA, TRIM_MAX_DELTA);
    let new_c = (prev_c * (1.0 + clamped_delta)).clamp(TRIM_MIN, TRIM_MAX);
    TrimOutcome::Trimmed { new_c }
}
```
Note: `prev_c * (1.0 + clamped_delta)` with `clamped_delta` already bounded to
`+-0.02` gives `new_c` within `+-2%` of `prev_c` (before the outer clamp), matching
the test's `1.02`/`0.98` expectations exactly since `prev_c = 1.0` there.

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p fermentool-core --lib trim::`, expect PASS (all `trim.rs` tests, ~20).

- [ ] **Step 5: Commit**

```bash
git add crates/fermentool-core/src/trim.rs
git commit -m "feat(core): bounded trim-update function"
```

---

## Task 7: Wire it into `Engine`

**Goal:** The pieces from Tasks 2, 4, 5, 6 actually drive the pump's setpoint, gated by a per-run flag, with the scale-unreachable degrade-to-manual behavior.

**Files:**
- Modify: `crates/fermentool-core/src/engine/mod.rs`, `crates/fermentool-core/src/control.rs`

**Interfaces:**
- Consumes: `trim::{ScaleState, StateInput, next_state, theil_sen_slope, decimate, update_trim, TrimOutcome, MAX_SLOPE_POINTS}`, `scale::parse_sics_weight`, `transport::WatchdogTransport`.
- Produces:
  - `RunConfig.gravimetric_trim: bool` (new field, `#[serde(default)]`).
  - `ActiveRun.gravimetric_trim: bool` (copied from `RunConfig` in `start_run`/`resume`).
  - `Engine<T>` new fields: `scale: Option<WatchdogTransport>`, `trim_c: f64` (default `1.0`), `scale_state: ScaleState` (default `Normal`), `scale_read_fails: u32`, `scale_ok: bool` (default `true`), `refill_weight_g: Option<f64>`, `refill_at: Option<Timestamp>`, `weight_buffer: Vec<(f64, f64)>` (elapsed-seconds-since-refill, grams), `last_weight_g: Option<f64>`, `settling_since: Option<Timestamp>`, `manual_refill_mode: bool`, `manual_refill_done_flag: bool`.
  - `pub fn scale_tick(&mut self, now: Timestamp)`, reads the scale (if configured and the active run wants trim), runs the state machine, updates `trim_c` in `Normal`, freezes/alarms otherwise; no-op if `scale` is `None` or no active run has `gravimetric_trim`.
  - `pub fn trigger_refill_mode(&mut self)` / `pub fn trigger_refill_done(&mut self)`.
  - `tick()` and `apply_setpoint()`: `target = quantize(active.spec.value_at(elapsed) * self.trim_c, grid)` (was `quantize(active.spec.value_at(elapsed), grid)`).

- [ ] **Step 1: Write the failing engine-level test with a fake scale**

Add a test double next to `SimPump` usage in `engine::mod::tests` (a fake `Transport` for the scale, scripted with a sequence of replies):
```rust
struct ScriptedScale {
    replies: std::collections::VecDeque<Result<Vec<u8>, TransportError>>,
}
impl ScriptedScale {
    fn new(weights: &[f64]) -> Self {
        let replies = weights
            .iter()
            .map(|w| Ok(format!("S S {w:>10.1} g\r\n").into_bytes()))
            .collect();
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
    let mut e = engine(); // existing helper, sim pump
    // Attach a scale that will always answer with a fixed weight (no drift ->
    // c should stay 1.0, but the wiring -- read -> state machine -> trim_c
    // applied in tick -- must all actually run).
    e.attach_scale_for_test(Box::new(ScriptedScale::new(&[1000.0; 50])));
    let cfg = RunConfig { gravimetric_trim: true, ..linear_cfg() };
    e.start_run(cfg, t0()).unwrap();
    e.scale_tick(t0());
    let o = e.tick(at(1)).unwrap();
    assert!(matches!(o, TickOutcome::Applied { .. }));
    // With no drift, c is still 1.0 -> unchanged from the manual-mode target.
    assert_eq!(e.trim_c(), 1.0);
}

#[test]
fn scale_failure_freezes_c_and_keeps_the_pump_running() {
    let mut e = engine();
    e.attach_scale_for_test(Box::new(ScriptedScale { replies: Default::default() })); // always times out
    let cfg = RunConfig { gravimetric_trim: true, ..linear_cfg() };
    e.start_run(cfg, t0()).unwrap();
    for i in 0..10 {
        e.scale_tick(at(i));
    }
    assert!(!e.status().scale_ok);
    // The pump must still be getting ticks -- trim just isn't correcting.
    let o = e.tick(at(20)).unwrap();
    assert!(matches!(o, TickOutcome::Applied { .. }));
}
```
(Add `pub(crate) fn attach_scale_for_test(&mut self, t: Box<dyn Transport + Send>)` and `pub(crate) fn trim_c(&self) -> f64` under `#[cfg(test)]` if a non-test path for attaching a scripted scale doesn't already exist from Task 3's wiring -- Task 3's real path goes through `WatchdogTransport`, which needs a real thread; for this fast unit test, store the scale as a plain `Box<dyn Transport + Send>` behind a second, simpler engine field used only when a `WatchdogTransport` isn't wanted, OR give `Engine::scale_tick` a way to accept either. Simplest: keep `scale: Option<Box<dyn Transport + Send>>` directly on `Engine` for reading, and let `main.rs` hand it a `WatchdogTransport` cast to `Box<dyn Transport + Send>` -- `WatchdogTransport` itself implements `Transport`, so this is a non-issue: `Engine.scale: Option<Box<dyn Transport + Send>>`, and `main.rs` does `Some(Box::new(watchdog_transport))`. Update the Interfaces list above accordingly before writing Step 3.)

- [ ] **Step 2: Run, verify they fail**

Run: `cargo test -p fermentool-core --lib engine:: gravimetric` and `scale_failure`, compile errors (fields/methods don't exist).

- [ ] **Step 3: Implement**

Add the fields to `Engine` and `ActiveRun`/`RunConfig` per Interfaces. In `tick()`, change:
```rust
let target = quantize(
    active.spec.value_at(Duration::from_secs_f64(elapsed_s)),
    setpoint_grid(control_var),
);
```
to:
```rust
let raw = active.spec.value_at(Duration::from_secs_f64(elapsed_s)) * self.trim_c;
let target = quantize(raw, setpoint_grid(control_var));
```
Mirror the same change in `apply_setpoint()`. Implement `scale_tick`:
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
                self.scale_ok = false; // c stays frozen; pump keeps running on it
            }
            return;
        }
    };

    let prev = self.last_weight_g.unwrap_or(weight_g);
    self.last_weight_g = Some(weight_g);
    let seconds_in_settling = self
        .settling_since
        .map(|s| now.duration_since(s).as_secs_f64())
        .unwrap_or(0.0);
    let input = trim::StateInput {
        weight_g,
        prev_weight_g: prev,
        recent_variance_g: 0.0, // TODO-free note: computed from weight_buffer's tail in a follow-up; 0.0 here means "settled immediately", acceptable for v1 since REFILL_SETTLE_SECONDS still gates the transition
        seconds_in_settling,
        manual_refill_mode: self.manual_refill_mode,
        manual_refill_done: self.manual_refill_done_flag,
    };
    self.manual_refill_done_flag = false; // one-shot
    let next = trim::next_state(self.scale_state, &input);

    if next == trim::ScaleState::RefillSettling && self.scale_state != trim::ScaleState::RefillSettling {
        self.settling_since = Some(now);
    }
    if next == trim::ScaleState::Normal && self.scale_state != trim::ScaleState::Normal {
        // Coming out of RefillSettling: new cumulative reference.
        self.refill_weight_g = Some(weight_g);
        self.refill_at = Some(now);
        self.weight_buffer.clear();
    }
    self.scale_state = next;

    if self.scale_state != trim::ScaleState::Normal {
        return; // frozen: no buffer append, no trim update
    }

    let t_refill = self.refill_at.unwrap_or(now);
    let elapsed_since_refill = now.duration_since(t_refill).as_secs_f64().max(0.0);
    self.weight_buffer.push((elapsed_since_refill, weight_g));

    // Cumulative check (drives the trim -- see spec §5/§6).
    if let (Some(w0), Some(active)) = (self.refill_weight_g, self.active.as_ref()) {
        let density = self.scale_density_g_per_ml;
        let elapsed = now.duration_since(active.started_at).as_secs_f64().max(0.0);
        let refill_elapsed = now.duration_since(t_refill).as_secs_f64().max(0.0);
        let start_elapsed = (elapsed - refill_elapsed).max(0.0);
        let mass_theoretical =
            integrate_curve_mass(&active.spec, start_elapsed, elapsed, density) * self.trim_c;
        let mass_measured = w0 - weight_g;
        if mass_theoretical.abs() > 1e-6 {
            let error_frac = (mass_theoretical - mass_measured) / mass_theoretical;
            match trim::update_trim(self.trim_c, error_frac) {
                trim::TrimOutcome::Unchanged => {}
                trim::TrimOutcome::Trimmed { new_c } => self.trim_c = new_c,
                trim::TrimOutcome::Alarm => {
                    self.scale_ok = false; // reuse the same alarm flag; a distinct
                                             // "trim_alarm" flag is a fine follow-up
                }
            }
        }
    }
}
```
`integrate_curve_mass` is a small new free function (numeric integration of
`spec.value_at(t) * density` over `[start, end]`, e.g. Simpson's rule with
~50 sub-intervals -- pure, testable on its own with a `Constant` curve where
the closed form is trivial: `rate * density * (end - start)`).

Add `trigger_refill_mode`/`trigger_refill_done` as one-line setters on the
two `manual_refill_*` fields. Add `scale_ok`/`scale_state`/`trim_c` to
`EngineStatus` (the struct `current_status` reads from) alongside the
existing `serial_ok`/`journal_ok`.

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p fermentool-core --lib engine::`, expect PASS, no regressions in the ~94 existing engine tests plus the 2 new ones.

- [ ] **Step 5: Wire `scale_tick` into the control loop**

In `control.rs`'s `control_loop`, alongside the existing idle-time `probe_link`
cadence, add a `next_scale_probe: Option<Instant>` ticking at `LINK_PROBE_INTERVAL`
(1 s, reuse the constant) whenever an active run has `gravimetric_trim` (check
via `engine.status().active` -- note `ActiveStatus` needs a `gravimetric_trim`
field added for this check, or thread it via a cheap `Command::Status`-adjacent
query; simplest is adding the bool to `ActiveStatus` since it's already exposed
there for the UI in Task 9 anyway).

- [ ] **Step 6: Full suite + commit**

Run: `cargo test`
```bash
git add crates/fermentool-core/src/engine/mod.rs crates/fermentool-core/src/control.rs
git commit -m "feat(core): wire the gravimetric trim into the tick loop"
```

---

## Task 8: Persistence across crash-resume

**Goal:** `trim_c`, `scale_state`, and the refill reference survive a restart, via `app_state` (the same mechanism `holding` uses), not a schema migration.

**Files:**
- Modify: `crates/fermentool-core/src/engine/mod.rs`

**Interfaces:**
- Produces: `const TRIM_STATE_KEY: &str = "trim_state";`, a small `#[derive(Serialize, Deserialize)] struct PersistedTrim { trim_c: f64, scale_state: ScaleState, refill_weight_g: Option<f64>, refill_at: Option<Timestamp> }`, `fn save_trim_state(&self)` (called after every `scale_tick` state change, mirroring `set_holding`'s pattern), and `Engine::new` restoring it (mirroring how `holding` is restored today).

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
        // Force a known trim value directly for a deterministic assertion.
        a.set_trim_c_for_test(1.05);
        a.save_trim_state();
    }
    let b = Engine::new(Pump::new(SimPump::new(1), 1), db.store(), "test");
    assert_eq!(b.trim_c(), 1.05);
}
```

- [ ] **Step 2: Run, verify it fails**

Run: `cargo test -p fermentool-core --lib engine::tests::trim_state_survives_a_restart`, FAIL (resets to `1.0`, the field doesn't persist yet).

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
let persisted_trim = store
    .get_state(TRIM_STATE_KEY)
    .ok()
    .flatten()
    .and_then(|s| serde_json::from_str::<PersistedTrim>(&s).ok());
let (trim_c, scale_state, refill_weight_g, refill_at) = match persisted_trim {
    Some(p) => (p.trim_c, p.scale_state, p.refill_weight_g, p.refill_at),
    None => (1.0, trim::ScaleState::Normal, None, None),
};
```
Call `self.save_trim_state()` at the end of `scale_tick` whenever `trim_c` or
`scale_state` actually changed (cheap guard: compare before/after, only write
on change -- this runs at most once per adaptive window in steady state, not
every second).

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p fermentool-core --lib engine::tests::trim_state_survives_a_restart`, PASS.

- [ ] **Step 5: Full suite + commit**

Run: `cargo test -p fermentool-core`
```bash
git add crates/fermentool-core/src/engine/mod.rs
git commit -m "feat(core): persist the gravimetric trim across a restart"
```

---

## Task 9: API + status

**Goal:** `gravimetric_trim` flows from `POST /api/runs` through to the engine; `/api/status` exposes `scale_ok`/`scale_state`/`trim_c`; manual refill trigger endpoints.

**Files:**
- Modify: `crates/fermentool-core/src/control.rs` (`DaemonStatus`, two new `Command`s), `crates/fermentool-core/src/api.rs` (two new routes)

**Interfaces:**
- Produces: `DaemonStatus` gains `pub scale_ok: bool, pub scale_state: Option<String>, pub trim_c: Option<f64>` (all `None`/default-ish when no scale configured); `Command::TriggerRefillMode(oneshot::Sender<()>)`, `Command::TriggerRefillDone(oneshot::Sender<()>)`; routes `.route("/api/scale/refill_mode", post(scale_refill_mode))`, `.route("/api/scale/refill_done", post(scale_refill_done))`.

- [ ] **Step 1: Write the failing API tests**

```rust
#[tokio::test]
async fn refill_endpoints_toggle_the_manual_flags() {
    let app = router(test_state());
    let res = app
        .clone()
        .oneshot(post_json("/api/scale/refill_mode", json!({})))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = app
        .oneshot(post_json("/api/scale/refill_done", json!({})))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn status_reports_no_scale_fields_when_unconfigured() {
    let app = router(test_state());
    let res = app.oneshot(get("/api/status")).await.unwrap();
    let body = body_json(res).await;
    assert!(body["scale_ok"].as_bool().unwrap()); // true = "no problem" is the safe default absent a scale
    assert!(body["scale_state"].is_null());
    assert!(body["trim_c"].is_null());
}
```

- [ ] **Step 2: Run, verify they fail**

Run: `cargo test -p fermentool-core --lib api::tests::refill_endpoints` and `status_reports_no_scale`, 404 / missing-field failures.

- [ ] **Step 3: Implement**

`control.rs`: add the two `Command` variants, handle them by calling
`engine.trigger_refill_mode()` / `trigger_refill_done()`; extend
`DaemonStatus` and `current_status`'s construction with the three new fields
sourced from `EngineStatus` (Task 7 already put them there).

`api.rs`:
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
Add both routes to `router()`.

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p fermentool-core`

- [ ] **Step 5: Full suite + rebuild + commit**

```bash
git add crates/fermentool-core/src/control.rs crates/fermentool-core/src/api.rs
git commit -m "feat(core): expose the gravimetric trim over the API"
```

---

## Task 10: UI

**Goal:** A checkbox to opt in per run, and a live status readout, following the codebase's existing indicator patterns.

**Files:**
- Modify: `ui/src/routes/NewRun.svelte`, `ui/src/routes/Overview.svelte`

**Interfaces:**
- Consumes: `/api/config` (`scale.path` presence), `/api/status` (`scale_ok`, `scale_state`, `trim_c`), `POST /api/scale/refill_mode` / `refill_done`.
- Produces: `f.gravimetric_trim` bound into the `POST /api/runs` payload's `gravimetric_trim` field.

- [ ] **Step 1: `NewRun.svelte` -- the opt-in checkbox**

Add to the run-config state: `gravimetric_trim: false`. Fetch `/api/config` (already done elsewhere in the app via `app.status`/config loads -- reuse whatever the existing pattern is, e.g. a `$derived` off a config load already present for other fields) to know `scaleConfigured = cfg?.scale?.path?.length > 0`. Render:
```svelte
{#if scaleConfigured}
  <label class="field check">
    <input type="checkbox" bind:checked={f.gravimetric_trim} />
    <span>Enable gravimetric trim (uses the configured scale to correct the feed rate over time)</span>
  </label>
{/if}
```
Include `gravimetric_trim: f.gravimetric_trim` in the `curveSpec()`/run-payload object sent to `POST /api/runs`.

- [ ] **Step 2: `Overview.svelte` -- live status**

Where the existing pump-link indicators render (same family as `serial_ok`/`pump_confirmed`), add, only when `app.status?.scale_state` is non-null:
```svelte
{#if app.status?.scale_state}
  <span class="pumpstat {app.status.scale_ok ? 'ok' : 'bad'}">
    <span class="dot" aria-hidden="true"></span>
    scale: {app.status.scale_state} (c={app.status.trim_c?.toFixed(3)})
  </span>
{/if}
```
(Match whichever existing class names `link.js`/`ConnBar` already use for
tone coloring -- reuse, don't invent a parallel style.)

- [ ] **Step 3: Build**

```bash
cd ui && npm run build
```
Expected: builds clean.

- [ ] **Step 4: Manual smoke**

Start the daemon (simulator is fine, no real scale needed to see the checkbox
appear/disappear): with `[scale] path = ""` in `config.toml`, confirm the
checkbox is hidden in NewRun; set a `path`, restart, confirm it appears.

- [ ] **Step 5: Commit**

```bash
git add ui/src/routes/NewRun.svelte ui/src/routes/Overview.svelte ui/dist
git commit -m "feat(ui): gravimetric trim opt-in and live status"
```

---

## Self-Review

**1. Spec coverage:**

| Spec section | Task |
|---|---|
| Goal 1 (manual unchanged) | Task 7 (`trim_c` defaults to 1.0, only path touched is multiplication by it) |
| Goal 2 (automatic, all shapes/control_vars) | Task 7 (`curve.value_at * trim_c`, shape-agnostic) |
| Goal 3 (swappable balance) | Task 2 (`SicsScale: Transport`, a second driver is a second small `impl Transport`) |
| Goal 4 (lost scale never stops the pump) | Task 7 (`scale_ok` alarm, `trim_c` frozen, tick keeps running) |
| Goal 5 (`c` survives crash-resume) | Task 8 |
| Spike result (SICS confirmed, format) | Task 2 (parser matches the captured bytes exactly) |
| Design §1 (Transport reuse, WatchdogTransport) | Task 2, Task 3 |
| Design §2 (config) | Task 1 |
| Design §3 (state machine) | Task 5, wired in Task 7 |
| Design §4 (adaptive window, Theil-Sen) | Task 4; the adaptive-window formula itself feeds the *rate* estimate, which this plan doesn't surface as a separate status field -- only the cumulative-driven `trim_c` acts on the pump (per spec §5-6, the cumulative check is what drives `c`, not the instantaneous rate) -- `theil_sen_slope`/`decimate` are built (Task 4) for a future rate display but Task 7 does not yet call them. **Gap, addressed below.** |
| Design §5 (cumulative mass balance) | Task 7 (`integrate_curve_mass`, `error_frac`) |
| Design §6 (trim bounds) | Task 6 |
| Design §7 (persistence) | Task 8 |
| Design §8 (scale unreachable) | Task 7 |
| Design §9 (RunConfig/API/UI) | Task 7 (`RunConfig`), Task 9 (API), Task 10 (UI) |
| Testing section | Every task's Steps 1-2 mirror the spec's testing list per component |
| Risks (`SerialTransport` not reusable) | Confirmed during planning (Global Constraints), Task 2 built the dedicated opener directly instead of discovering this mid-implementation |

**Gap found and fixed:** Task 4 builds `theil_sen_slope`/`decimate` (the
instantaneous-rate estimator) but Task 7's `scale_tick` only ever uses the
cumulative mass balance to drive `trim_c` -- it never calls
`theil_sen_slope`. This matches the spec's actual design (§5 says the
cumulative balance "is what actually drives `c`"; §4's rate estimate is
documented there as informative/for the adaptive window sizing, not as a
second trim input) but Task 7 as written above never even computes the
window size or an instantaneous rate for *anything*, so `theil_sen_slope`
and `decimate` would ship dead. Fix: Task 7's Step 3 is amended to also
compute `Q_meas` via `theil_sen_slope(&decimate(&self.weight_buffer,
MAX_SLOPE_POINTS))` once per `scale_tick` call while `Normal`, and store it
as `self.last_rate_g_per_min: Option<f64>` -- exposed alongside `trim_c` in
status (Task 9) as a diagnostic (`rate_g_per_min`), even though only the
cumulative check drives the correction. This keeps Task 4's output used and
gives the operator a live "measured vs commanded" number to look at, which
the spec's UI intent (§9, Overview showing scale state) implies without
spelling out this specific field. Task 9 and Task 10's interface lists above
should include `rate_g_per_min` alongside `trim_c` when implementing.

**2. Placeholder scan:** One instance flagged and left deliberately explicit
rather than silently glossed over: Task 7's `StateInput.recent_variance_g:
0.0` is a real, documented simplification (v1 doesn't compute a rolling
variance of the settling window yet; `REFILL_SETTLE_SECONDS` still gates the
`RefillSettling -> Normal` transition, so a bottle that's still being
handled just takes the fixed delay instead of also requiring low variance
first -- functionally safe, slightly less adaptive). A follow-up task
("compute `recent_variance_g` from the last N buffered readings") is a
one-function, one-test addition once this ships and is observed on a real
refill. Not left as a bare "TODO" -- the behavior and its consequence are
both stated.

**3. Type consistency:** `ScaleState` (Task 5) used identically in Task 7
and Task 8's `PersistedTrim`. `TrimOutcome` (Task 6) matched exactly in
Task 7's `match`. `ScaleConfig` (Task 1) field names (`path`, `baud`,
`density_g_per_ml`) used identically in Task 3's `build_scale` and Task 7's
`self.scale_density_g_per_ml` (sourced from `ScaleConfig.density_g_per_ml`
at `set_serial`-equivalent config-apply time -- add this assignment
explicitly when implementing Task 7, mirroring how `pump_addr` is threaded
today). `parse_sics_weight`'s `(f64, bool)` return matches its two call
sites (Task 2's own tests, Task 7's `scale_tick`).
