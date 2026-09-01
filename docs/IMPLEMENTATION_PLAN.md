# Fermentool — Implementation Plan (Phase 1)

> Status: design agreed 2026-09-01. This document is the reference for the initial build.
> Conversation language: French. Code, comments, logs, UI, and this doc: English.

## 1. Scope

**Phase 1 goal:** drive one Baoding Shenchen **LabQ** peristaltic pump over MODBUS-RTU so
that its setpoint (**rpm** *or* **ml/min**, one per run) follows a chosen **time profile**
over a run that averages ~100 h, with **zero tolerance for crash / data loss** and
**automatic time-correct resume** after any interruption.

**In scope:** curve engine, MODBUS control, persistence + journal, crash recovery, local
web UI (new run / live run / history), single self-contained binary per OS.

**Out of scope for now (hooks reserved):** leak sensors, camera, long-term data warehousing.

## 2. Hardware & wiring

No microcontroller in phase 1.

```
PC ──USB── [isolated FTDI USB↔RS485 adapter] ──A/B/GND── LabQ pump
```

- Adapter: **isolated, FT232-based, automatic direction control**
  (e.g. Waveshare "USB TO RS485 (isolated)", DSD TECH SH-U12, FTDI USB-RS485-WE cable).
  Galvanic isolation matters in a wet lab over a 100 h run.
- Serial line to pump: **8E1** (1 start, 8 data, 1 **even** parity, 1 stop), baud
  1200/2400/4800/9600 — default **9600**. Pump slave address default **1**.
- "Hold last setpoint on PC death" is inherent: once the pump has received `start` + a
  speed, it runs indefinitely with no further frames.

**Future:** an `ESP32-S3-DevKitC-1-N16R8` may be added as a *separate* sensor/vision hub
(own USB or Wi-Fi). It must **not** sit in the pump-control path.

## 3. Repository layout

Cargo workspace + a Svelte UI folder.

```
Fermentool/
├─ Cargo.toml                     # workspace
├─ crates/
│  ├─ fermentool-core/            # the daemon binary
│  │  ├─ src/
│  │  │  ├─ main.rs               # bootstrap, catch_unwind supervisor
│  │  │  ├─ config.rs             # TOML config load/save
│  │  │  ├─ api/                  # axum router: REST + WS
│  │  │  ├─ engine/               # run lifecycle, tick loop, resume logic
│  │  │  ├─ store/                # SQLite (schema, migrations, repos)
│  │  │  ├─ pump/                 # PumpTransport trait, real + simulator
│  │  │  └─ telemetry.rs          # tracing + rolling file logger
│  │  └─ assets/                  # embedded UI dist (rust-embed), built from ../../ui
│  ├─ fermentool-modbus/          # MODBUS-RTU framing/CRC + LabQ register map
│  └─ fermentool-curves/          # pure curve math, no I/O
├─ ui/                            # Svelte 5 + Vite; `npm run build` → dist/
├─ docs/
│  ├─ IMPLEMENTATION_PLAN.md      # this file
│  ├─ wiring.md
│  └─ service-install.md
├─ LabQ Series MODBUS protocol.md
└─ LabQ Series MODBUS protocol.pdf
```

Rationale: `fermentool-curves` and `fermentool-modbus` are I/O-free and fully unit-tested
in isolation; the daemon wires them to serial + HTTP + storage.

## 4. Core daemon design

### 4.1 Runtime & crates

`tokio` (multi-thread), `axum` + `tower-http` (HTTP + static + WS), `tokio-serial` +
`tokio-modbus` (RTU client, handles CRC), `rusqlite` (bundled SQLite, WAL),
`serde` / `serde_json`, `toml`, `tracing` + `tracing-subscriber` + `tracing-appender`
(rolling file), `jiff` (absolute UTC timestamps + duration math), `anyhow` (app) +
`thiserror` (libs), `rust-embed` (UI assets), `clap` (CLI flags).

### 4.2 Task layout

- **engine task** — owns the run state machine and the tick loop; the *only* task that
  talks to the pump (via an `mpsc` command queue so the API never touches serial).
- **api task** — axum server on `127.0.0.1:<port>` (default 8730); serves embedded UI,
  REST, and a WS broadcast channel fed by the engine.
- **serial-health task** — folded into the engine: reopen with backoff on error.
- **supervisor** — `main.rs` runs the engine loop inside `catch_unwind` + restart with
  backoff; a panic is logged as an `event`, never a process exit. OS-level restart
  (systemd `Restart=always` / Windows Task Scheduler / launchd `KeepAlive`) is the outer
  safety net and is documented in `docs/service-install.md`.

### 4.3 MODBUS layer (`fermentool-modbus`)

Register map (decimal address; source = `LabQ Series MODBUS protocol.md`):

| Addr | Name | Type | Notes |
|---|---|---|---|
| 1000 | Pump head type | u16 (06H) | Chart 1; KT15 = 0 |
| 1001 | Tubing size | u16 (06H) | e.g. 16# |
| 1002 | Motor speed | f32 (10H, 2 regs) | 0.1–350 rpm |
| 1004 | Flow rate | f32 (10H, 2 regs) | 0–99999 (ml/min) |
| 1006 | Start/stop | u16 (06H) | 1 = start, 0 = stop |
| 1007 | Direction | u16 (06H) | 1 = CW, 0 = CCW |
| 1008 | Full-speed run | u16 (06H) | 1 / 0 |
| 1009 | Back-suction angle | u16 (06H) | 0–360° |

Float encoding: IEEE-754, **big-endian word order** (`8.9 → 41 0E 66 66`). CRC-16
`poly 0xA001, init 0xFFFF`, low byte then high byte (tokio-modbus does this; we keep a
standalone impl for tests).

**Byte-exact test vectors** (must match the doc):
- set 58.8 rpm → `01 10 03 EA 00 02 04 42 6B 33 33 58 29`
- set 50 ml/min → `01 10 03 EC 00 02 04 42 48 00 00 7D 2C`
- start → `01 06 03 EE 00 01 28 7B`
- direction CW → `01 06 03 EF 00 01 79 BB`

**Start sequence** (engine, on run start or resume):
1. `06H` 1007 ← direction
2. if `control_var = ml_min`: `06H` 1000 ← head, `06H` 1001 ← tubing
3. `10H` 1002 or 1004 ← first setpoint (from curve at current elapsed)
4. `06H` 1006 ← 1 (start)

**Per tick:** `10H` 1002/1004 ← target. Optionally `03H` read-back for verification
(stored in `ticks.readback`).

**Stop:** `06H` 1006 ← 0.

`PumpTransport` trait:

```rust
#[async_trait]
trait PumpTransport {
    async fn set_direction(&mut self, cw: bool) -> Result<()>;
    async fn set_head_tubing(&mut self, head: u16, tubing: u16) -> Result<()>;
    async fn set_speed_rpm(&mut self, rpm: f32) -> Result<()>;
    async fn set_flow_ml_min(&mut self, ml_min: f32) -> Result<()>;
    async fn start(&mut self) -> Result<()>;
    async fn stop(&mut self) -> Result<()>;
    async fn read_speed_rpm(&mut self) -> Result<f32>;
}
```

Implementations: `RtuPump` (real) and `SimPump` (models a pump that holds its last
speed, with configurable induced comms drops and latency) — used by all engine tests.

### 4.4 Curve engine (`fermentool-curves`)

```rust
struct CurveSpec {
    mode: ParamMode,          // Endpoints | Physio
    start: f64,               // setpoint at t = 0
    end: f64,                 // setpoint at t = duration (derived in Physio for linear/exp)
    duration: Duration,
    clamp_min: f64,
    clamp_max: f64,
    params: CurveParams,      // kind-specific; params.kind() -> CurveKind (the `runs.curve_kind` column)
}

impl CurveSpec {
    fn value_at(&self, elapsed: Duration) -> f64;   // result already clamped
    fn effective_end(&self) -> f64;                 // resolves Physio linear/exp
    fn preview(&self, samples: usize) -> Vec<(f64, f64)>;
    fn validate(&self) -> Result<(), String>;
}
```

Implemented in milestone 2. `kind` is not a stored field — it is derived from
`params` so the two can never disagree.

Let `p = clamp(elapsed / duration, 0.0, 1.0)`, `S = start`, `E = end`, `D = duration` (hours).

| Kind | Endpoints mode | Physio mode |
|---|---|---|
| **Linear** | `S + (E − S)·p` | same (slope = param `rate` per hour; `E` derived) |
| **Exponential** (fed-batch) | `S·(E/S)^p` (needs `S,E > 0`); constant relative rate | `S·exp(µ·t)`, `t` in hours, `µ` = param `mu_per_h`; `E = S·exp(µ·D)` |
| **Sigmoid / logistic** | normalized logistic between `S` and `E`, steepness `a` (default 8): `S + (E−S)·(σ(a(p−½)) − σ(−a/2)) / (σ(a/2) − σ(−a/2))` | `S + (E−S)/(1 + exp(−k·(t − t_m)))`, params `k`, `t_m` |
| **Step** | list of `(at: Duration, value: f64)`; hold each value until the next | same |
| **Constant** | `S` | `S` |
| **Custom** | table of `(t_i, v_i)`; linear (or `hold`) interpolation; CSV import | same |

Invariants enforced by tests: `value_at(0) == S`; `value_at(≥D) == E` (for Endpoints,
non-step); monotonic when `S<E` for linear/exp/sigmoid; always within `[clamp_min,
clamp_max]`; `clamp_min ≥ pump floor` (0.1 rpm / 0), `clamp_max ≤ pump ceiling`
(350 rpm / 99999).

`preview(spec, n)` → `Vec<(elapsed_s, value)>` for the UI chart, reused by
`POST /api/preview`.

### 4.5 Tick loop

```
loop {
    let now_wall = Utc::now();
    let elapsed  = now_wall − run.started_at;          // wall clock is the source of truth
    let target   = curve.value_at(elapsed);
    let ok       = pump.set_setpoint(target).await.is_ok();
    store.append_tick(run.id, seq, now_wall, elapsed, target, ok, readback);
    broadcast(Status { .. });
    if elapsed >= run.duration { finish(run); break; }
    sleep_until(next_tick);                            // fixed cadence, default 10 s
}
```

- `tick_interval` configurable per run (default 10 s; min 1 s, max 300 s).
- Wall-clock elapsed (not an accumulator) so a paused/late/frozen process still computes
  the *correct* setpoint for real time — this is what makes resume trivially correct.
- A missed/failed write is logged (`written_ok = 0`) and simply retried next tick; the
  pump holds its previous speed meanwhile.

### 4.6 Persistence — SQLite (`store/`)

`PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;` — crash-safe, cost negligible at
~1 write / 10 s. DB path from config (default OS data dir: `%APPDATA%/Fermentool`,
`~/.local/share/fermentool`, `~/Library/Application Support/Fermentool`).

```sql
CREATE TABLE runs (
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL,
  created_at    TEXT NOT NULL,             -- ISO-8601 UTC
  started_at    TEXT NOT NULL,             -- t0, the resume anchor
  ended_at      TEXT,
  status        TEXT NOT NULL,             -- running | completed | stopped | aborted
  control_var   TEXT NOT NULL,             -- rpm | ml_min
  direction     TEXT NOT NULL,             -- cw | ccw
  duration_s    INTEGER NOT NULL,
  tick_interval_s INTEGER NOT NULL,
  curve_kind    TEXT NOT NULL,
  curve_mode    TEXT NOT NULL,             -- endpoints | physio
  curve_params  TEXT NOT NULL,             -- JSON (start,end,params,clamps)
  pump_addr     INTEGER NOT NULL,
  pump_head     TEXT, tubing TEXT,         -- ml_min only
  app_version   TEXT NOT NULL
);
CREATE TABLE ticks (
  id         INTEGER PRIMARY KEY,
  run_id     INTEGER NOT NULL REFERENCES runs(id),
  seq        INTEGER NOT NULL,
  wall_time  TEXT NOT NULL,                -- ISO-8601 UTC
  elapsed_s  REAL NOT NULL,
  target     REAL NOT NULL,
  written_ok INTEGER NOT NULL,             -- 1 | 0
  readback   REAL,
  note       TEXT
);
CREATE INDEX ix_ticks_run ON ticks(run_id, seq);
CREATE TABLE events (
  id        INTEGER PRIMARY KEY,
  run_id    INTEGER REFERENCES runs(id),
  wall_time TEXT NOT NULL,
  level     TEXT NOT NULL,                 -- info | warn | error
  kind      TEXT NOT NULL,                 -- start|stop|resume|serial_lost|serial_restored|
                                           -- crash_detected|curve_done|write_fail|...
  detail    TEXT
);
CREATE TABLE app_state (key TEXT PRIMARY KEY, value TEXT);  -- schema_version, ...
```

Migrations: `refinery` (embedded, versioned).

### 4.7 Crash recovery / resume logic

On startup, after migrations:

1. `SELECT * FROM runs WHERE status = 'running'` → at most one (enforced).
2. None → idle, UI shows "New run".
3. Found ⇒ this run did **not** end cleanly. Compute
   `elapsed = now_utc − started_at`.
   - `elapsed ≤ duration + grace (default 5 min)` → **resumable**. Emit
     `event(crash_detected)`. UI shows a modal:
     *"Run «name» was interrupted. Stopped around HH:MM, now HH:MM (t = Xh Ym).
     Resume at the profile value for now (= V rpm/ml·min⁻¹)?"* → **[Resume] [Finish now] [Abort]**.
   - `elapsed > duration + grace` → the curve finished while offline. Modal offers
     **[Finish (hold at end value / stop)] [Abort]**.
4. **Resume** ⇒ re-run the full **start sequence** (direction, head/tubing, first
   setpoint from `curve.value_at(elapsed)`, start) — the pump may have been
   power-cycled while the PC was down — set `status='running'`, `event(resume)`, and the
   tick loop continues from `elapsed`. `started_at` is **unchanged** so timing stays
   absolute.
5. Auto-resume option in config (`resume.prompt = true|false`); default **prompt**.

Graceful stop/abort/complete set `ended_at` + terminal `status`, so they never trigger
the recovery path.

### 4.8 HTTP API (axum, `127.0.0.1` only)

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/status` | current run, elapsed, current/next target, serial + pump state |
| POST | `/api/preview` | curve spec → sampled series (no side effects) |
| POST | `/api/runs` | create **and** start a run |
| GET | `/api/runs` / `/api/runs/:id` | list / detail |
| GET | `/api/runs/:id/ticks?from=&to=` | journal slice for charts |
| GET | `/api/runs/:id/export.csv` | full journal as CSV |
| POST | `/api/runs/:id/stop` | graceful stop (pump stop) |
| POST | `/api/runs/:id/resume` | confirm resume after crash |
| POST | `/api/runs/:id/abort` | stop + mark aborted |
| GET / PUT | `/api/config` | serial port, baud, address, data dir, port, resume prompt |
| GET | `/api/pump/probe` | one-shot read of pump registers (diagnostics) |
| GET | `/api/serial/ports` | enumerate serial ports for the settings picker |
| WS | `/api/ws` | push: `status` (every tick + on change), `tick`, `event` |

All mutations are idempotent where possible and validated against pump limits + clamps
server-side (never trust the UI).

### 4.9 Config (`config.toml`)

```toml
port = 8730                     # local web UI / API
[serial]
path = "COM4"                   # or "/dev/ttyUSB0"
baud = 9600
# parity/data/stop fixed at 8E1 for LabQ
[pump]
address = 1
default_head = "KT15"
default_tubing = "16"
[resume]
prompt = true
grace_minutes = 5
[storage]
dir = ""                        # empty = OS default
[log]
dir = ""                        # empty = <storage>/logs
level = "info"
retain_days = 30
```

### 4.10 Logging

`tracing` → daily rolling file (`tracing-appender`) + stderr. Every pump write, serial
event, run transition, and panic is logged *and* mirrored into `events` for the UI.
Integrity: nightly `PRAGMA integrity_check` logged.

### 4.11 Resilience checklist

- Serial write/read error → log, `written_ok=0`, retry next tick, reopen port with
  exponential backoff (1 s → 30 s cap), re-enumerate by saved path / VID:PID on USB
  unplug. Never exit.
- Engine panic → `catch_unwind`, `event(crash_detected internal)`, restart loop after
  backoff; run state reloaded from SQLite.
- DB write error → retry; if persistent, keep controlling the pump and surface a loud UI
  alert (control continuity beats logging).
- Clock: use `jiff` UTC; guard against wall-clock jumps (NTP step) by cross-checking a
  monotonic `Instant` and logging if they diverge > tick_interval.
- Single-instance lock file so two daemons can't fight over the port.

## 5. Web UI (`ui/`, Svelte 5 + Vite)

Built to static assets, embedded via `rust-embed`, served at `http://localhost:8730`.
Charts: **uPlot** (tiny, fast time-series). No backend framework, just `fetch` + one WS.

**Sections (phase 1):**

1. **Top bar** — serial status ●, pump status ●, wall clock, run clock, app version.
2. **New run** — form: name · control variable (rpm / ml·min⁻¹) · direction (CW/CCW) ·
   duration · tick interval · curve kind · mode toggle (Endpoints / Physiological) ·
   dynamic parameter fields · min/max clamp · pump address · (head + tubing if ml/min).
   **Live preview chart** (calls `/api/preview` on change). "Start run" button.
3. **Active run** — large current setpoint readout · elapsed / remaining / progress bar ·
   **planned curve vs. actual points** chart (uPlot, live via WS) · event log · "Stop".
4. **History** — table of past runs → open one → chart + "Export CSV".
5. **Resume modal** — shown on load when the daemon reports `crash_detected`.
6. **Settings** — serial port picker (`/api/serial/ports`), baud, address, data dir,
   UI port, resume-prompt toggle; "Probe pump" diagnostic button.

**Disabled/placeholder tabs** (visible, not clickable): *Sensors*, *Camera*, *Data export*.

**Reconnect:** the WS auto-reconnects with backoff; the page fully rebuilds state from
`/api/status` + `/api/runs/:id/ticks` on reconnect, so a closed laptop lid / browser
restart loses nothing.

## 6. Testing strategy

- **Unit (`fermentool-curves`)** — invariants in §4.4; property tests (`proptest`) for
  bounds/monotonicity; golden values for exp & sigmoid.
- **Unit (`fermentool-modbus`)** — CRC vectors + byte-exact frame builders vs. the four
  doc examples; float encode/decode round-trips incl. `8.9 → 41 0E 66 66`.
- **Engine integration** — against `SimPump`: run a compressed profile (tick 1 s,
  duration 300 s); mid-run `SIGKILL`/panic; restart; assert the resumed setpoint equals
  `curve.value_at(real_elapsed)` within tolerance and that `started_at` is unchanged.
- **Comms-loss** — `SimPump` drops ACKs for N ticks: assert no crash, `written_ok=0`
  rows, recovery `event`s, pump still at last speed.
- **Soak** — 100 h+ `SimPump` run (nightly / long local): assert flat RSS, no panics,
  `integrity_check = ok`, tick count == expected.
- **Hardware bring-up** — `cargo run -- probe` against the real pump; a short real ramp
  with a beaker before the first real ferment.

## 7. Build & packaging

- `cargo build --release` per target: `x86_64-pc-windows-msvc`,
  `x86_64-unknown-linux-gnu` (+ `aarch64`), `aarch64-apple-darwin` /
  `x86_64-apple-darwin`.
- `build.rs` (or CI step) runs `npm ci && npm run build` in `ui/` and fails the build if
  `ui/dist` is missing/stale; `rust-embed` bakes it in.
- Output: one self-contained binary, no runtime install on the target.
- `docs/service-install.md`: systemd unit (`Restart=always`), launchd plist
  (`KeepAlive`), Windows Task Scheduler / NSSM notes. v1 may just ship an autostart
  shortcut + a tray-less console.
- CI (GitHub Actions or local `just` recipes): fmt, clippy `-D warnings`, test, build
  matrix, soak job.

## 8. Milestones (build order)

1. **Scaffold** — workspace, three crates, `ui/` Vite app, CI, this doc wired to README. ✅
2. **`fermentool-curves`** — all kinds + both modes + tests + `preview()` + `validate()`. ✅
   (Design language captured in `docs/DESIGN.md` alongside this milestone.)
3. **`fermentool-modbus`** — CRC + `f32` encoding + register map ✅; still to add: full
   frame builders/parsers, async RTU client, `PumpTransport` + `SimPump`.
4. **`store/`** — SQLite schema, migrations, run/tick/event repos, `integrity_check`.
5. **`engine/`** — run lifecycle, tick loop, start/stop sequences, server-side clamps.
6. **Recovery** — startup scan, resume decision, re-init on resume, kill/restart tests.
7. **API + config** — axum REST + WS, `config.toml`, single-instance lock, logging.
8. **UI** — new-run + preview, active-run + live chart, history + CSV, resume modal,
   settings.
9. **Package** — embed UI, per-OS binaries, service docs.
10. **Soak + field test** — 100 h sim, then real pump bring-up.
11. **Docs** — README, `wiring.md`, `service-install.md`.

## 9. Reserved for later (design now, don't build)

- `SensorSource` trait + internal event bus (leak, temp, pH…); `sensors` + `sensor_readings`
  tables; *Sensors* UI tab.
- Camera: USB webcam captured by the daemon or shown by the browser; `snapshots` table;
  *Camera* tab. (ESP32-S3 hub optional, out of the control path.)
- Data export: Parquet/CSV bundles, run comparison overlay.
- Multi-pump: RS485 multi-drop, per-run `pump_addr` already in the schema.
- Optional native WebView shell (Tauri/Wails) — no core changes.
```
