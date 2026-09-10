# Live Serial Reconnect + Port Rescan Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the Settings page switch the running daemon's pump transport (simulator ⇄ a real serial port) and refresh the detected-port list without restarting the daemon.

**Architecture:** Extract the "open the transport, fall back to the simulator" logic out of `main.rs::build_engine` into a reusable `fermentool_core::transport` module that also defines a `SwapTransport` trait. `Engine` gains `swap_transport()` (refused while a run is active) and records the live `TransportKind` for the status frame. A new `Command::Reconnect` reaches the control thread; `POST /api/serial/reconnect` persists the choice to `config.toml` and fires that command. The Svelte Settings page gets **Rescan** and **Connect now** buttons plus a "Connected to: …" readout.

**Tech Stack:** Rust (axum 0.8, tokio, serde, `serialport` 4 behind the modbus crate's default `serial` feature), Svelte 5 (runes), Vite.

**Spec:** [docs/superpowers/specs/2026-09-02-live-serial-reconnect-design.md](../specs/2026-09-02-live-serial-reconnect-design.md)

## Global Constraints

- Reconnect is refused while a run is active (v1). Stop the run, reconnect, start a new run.
- No automatic hot-plug detection / background port watcher in v1.
- `config.toml` stays the single source of truth: every reconnect writes the chosen `serial.path` / `serial.baud` to it (path `%APPDATA%\Fermentool\config.toml` on Windows) so a later restart matches the live state.
- Changing `[pump] address` still needs a daemon restart; reconnect only re-opens the transport.
- LabQ serial frame is fixed 8E1; valid baud values are 1200 / 2400 / 4800 / 9600.
- No `Co-Authored-By: Claude` trailer on commits in this repo.
- After any change under `ui/src/`, run `npm run build` in `ui/` (regenerates `ui/dist/`, which is committed).
- Rust is at `~/.cargo/bin`, not on the Bash `PATH` by default. Prefix commands with `export PATH="$HOME/.cargo/bin:$PATH"` (or use a shell where cargo is already available).

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `crates/fermentool-core/src/transport.rs` | Build a `Box<dyn Transport + Send>` from a `SerialConfig`; `TransportKind`; `SwapTransport` trait + impls | **create** |
| `crates/fermentool-core/src/lib.rs` | Register the new module | modify |
| `crates/fermentool-core/src/config.rs` | `SerialConfig::use_simulator()` helper (DRY with `Config::use_simulator`) | modify |
| `crates/fermentool-core/src/engine/mod.rs` | Store live `TransportKind`; `swap_transport()`; `transport` in `EngineStatus` | modify |
| `crates/fermentool-core/src/control.rs` | `Command::Reconnect`; `handle` arm; `SwapTransport` bound on `control_loop`/`handle`/`spawn`; `transport` in `DaemonStatus` | modify |
| `crates/fermentool-core/src/api.rs` | `ReconnectReq`, `serial_reconnect` handler, route | modify |
| `crates/fermentool-core/src/main.rs` | `build_engine` uses `transport::open`; drop now-unused imports | modify |
| `ui/src/routes/Settings.svelte` | Rescan / Connect now buttons, error surfacing, "Connected to" readout | modify |

---

## Task 1: `transport` module, build & swap the pump transport

**Files:**
- Create: `crates/fermentool-core/src/transport.rs`
- Modify: `crates/fermentool-core/src/lib.rs` (add `pub mod transport;`)
- Modify: `crates/fermentool-core/src/config.rs` (add `SerialConfig::use_simulator`, delegate `Config::use_simulator` to it)
- Test: inline `#[cfg(test)] mod tests` in `crates/fermentool-core/src/transport.rs`

**Interfaces:**
- Consumes: `crate::config::SerialConfig`; `fermentool_modbus::{SimPump, Transport}`; `fermentool_modbus::serial::SerialTransport`.
- Produces:
  - `pub enum TransportKind { Sim, Serial(String) }`, `#[derive(Debug, Clone, PartialEq, Eq)]`; method `pub fn label(&self) -> String` (`"sim"` or the port name).
  - `pub fn open(serial: &SerialConfig, pump_addr: u8) -> (Box<dyn Transport + Send>, TransportKind)`, simulator for sim/empty path, else `SerialTransport::open(path, baud, 1500ms)`, falling back to the simulator (logged `error!`) on open failure.
  - `pub trait SwapTransport { fn swap(&mut self, serial: &SerialConfig, pump_addr: u8) -> TransportKind; }` with impls for `Box<dyn Transport + Send>` (real swap via `open`) and `SimPump` (no-op returning `TransportKind::Sim`).
  - `crate::config::SerialConfig::use_simulator(&self) -> bool`.

- [ ] **Step 1: Add the `use_simulator` helper on `SerialConfig`**

In `crates/fermentool-core/src/config.rs`, add to a new `impl SerialConfig` block (place it right after the `impl Default for SerialConfig` block, ~line 117):

```rust
impl SerialConfig {
    /// `true` when this points at the pump simulator rather than a real port.
    pub fn use_simulator(&self) -> bool {
        self.path.eq_ignore_ascii_case("sim") || self.path.is_empty()
    }
}
```

Then change `Config::use_simulator` (~line 185) to delegate:

```rust
    /// `true` when the pump simulator should be used instead of a real port.
    pub fn use_simulator(&self) -> bool {
        self.serial.use_simulator()
    }
```

- [ ] **Step 2: Create `crates/fermentool-core/src/transport.rs` with the implementation**

```rust
//! Building the pump transport from configuration, and swapping it on a live
//! [`Engine`](crate::engine::Engine) without restarting the daemon.
//!
//! `build_engine` (in `main.rs`) and `POST /api/serial/reconnect` both go through
//! [`open`]: `serial.path = "sim"` (or empty) selects the in-process simulator;
//! anything else is a real serial port, with the same "log the error and fall
//! back to the simulator" behaviour the daemon has always had at boot.

use std::time::Duration;

use fermentool_modbus::serial::SerialTransport;
use fermentool_modbus::{SimPump, Transport};

use crate::config::SerialConfig;

/// A whole serial transaction is bounded by this; matches `build_engine`'s
/// historical value.
const OPEN_TIMEOUT: Duration = Duration::from_millis(1500);

/// Which transport an engine is currently driving. Surfaced in the status frame
/// so the UI can show "Connected to: COM3" / "simulator".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportKind {
    /// The in-process register simulator.
    Sim,
    /// A real serial port, by OS name (e.g. `"COM3"`, `"/dev/ttyUSB0"`).
    Serial(String),
}

impl TransportKind {
    /// Short label for the status frame: `"sim"` or the port name.
    pub fn label(&self) -> String {
        match self {
            TransportKind::Sim => "sim".to_string(),
            TransportKind::Serial(name) => name.clone(),
        }
    }
}

/// Open the transport named by `serial`. Never fails: an unopenable port is
/// logged and the simulator is returned instead, so the daemon always comes up.
pub fn open(serial: &SerialConfig, pump_addr: u8) -> (Box<dyn Transport + Send>, TransportKind) {
    if serial.use_simulator() {
        tracing::warn!(
            "serial.path = \"{}\": using the pump SIMULATOR",
            serial.path
        );
        return (Box::new(SimPump::new(pump_addr)), TransportKind::Sim);
    }
    match SerialTransport::open(&serial.path, serial.baud, OPEN_TIMEOUT) {
        Ok(t) => {
            tracing::info!(port = %serial.path, baud = serial.baud, "serial port open");
            (Box::new(t), TransportKind::Serial(serial.path.clone()))
        }
        Err(e) => {
            tracing::error!(
                "cannot open {} ({e}); falling back to the SIMULATOR",
                serial.path
            );
            (Box::new(SimPump::new(pump_addr)), TransportKind::Sim)
        }
    }
}

/// A live engine transport that can be rebuilt in place from a [`SerialConfig`].
///
/// Implemented for the daemon's boxed transport (a real swap) and for [`SimPump`]
/// (a no-op, `control.rs`'s simulator-backed tests build an `Engine<SimPump>`).
pub trait SwapTransport {
    fn swap(&mut self, serial: &SerialConfig, pump_addr: u8) -> TransportKind;
}

impl SwapTransport for Box<dyn Transport + Send> {
    fn swap(&mut self, serial: &SerialConfig, pump_addr: u8) -> TransportKind {
        let (new, kind) = open(serial, pump_addr);
        *self = new; // old transport dropped here, SerialTransport's Drop closes the port
        kind
    }
}

impl SwapTransport for SimPump {
    fn swap(&mut self, _serial: &SerialConfig, _pump_addr: u8) -> TransportKind {
        TransportKind::Sim
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(path: &str) -> SerialConfig {
        SerialConfig {
            path: path.to_string(),
            baud: 9600,
        }
    }

    #[test]
    fn open_returns_the_simulator_for_sim_or_empty() {
        assert_eq!(open(&sc("sim"), 1).1, TransportKind::Sim);
        assert_eq!(open(&sc("SIM"), 1).1, TransportKind::Sim);
        assert_eq!(open(&sc(""), 1).1, TransportKind::Sim);
    }

    #[test]
    fn open_falls_back_to_the_simulator_when_the_port_cannot_open() {
        // A name that cannot resolve to a real port on any OS.
        let (_t, kind) = open(&sc("NOPE_NOT_A_REAL_PORT_99999"), 1);
        assert_eq!(kind, TransportKind::Sim);
    }

    #[test]
    fn box_swap_rebuilds_in_place_and_reports_the_kind() {
        let mut t: Box<dyn Transport + Send> = Box::new(SimPump::new(1));
        assert_eq!(t.swap(&sc("sim"), 1), TransportKind::Sim);
        assert_eq!(t.swap(&sc("NOPE_NOT_A_REAL_PORT_99999"), 1), TransportKind::Sim);
    }

    #[test]
    fn simpump_swap_is_a_noop() {
        let mut p = SimPump::new(1);
        assert_eq!(p.swap(&sc("COM3"), 1), TransportKind::Sim);
    }

    #[test]
    fn label_is_sim_or_the_port_name() {
        assert_eq!(TransportKind::Sim.label(), "sim");
        assert_eq!(
            TransportKind::Serial("COM3".to_string()).label(),
            "COM3"
        );
    }

    #[test]
    fn serial_config_use_simulator() {
        assert!(sc("sim").use_simulator());
        assert!(sc("").use_simulator());
        assert!(!sc("COM3").use_simulator());
    }
}
```

- [ ] **Step 3: Register the module**

In `crates/fermentool-core/src/lib.rs`, add `pub mod transport;` in alphabetical position (after `pub mod store;`):

```rust
pub mod api;
pub mod config;
pub mod control;
pub mod engine;
pub mod store;
pub mod transport;
```

- [ ] **Step 4: Run the new tests, expect PASS**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p fermentool-core transport::`
Expected: the 6 `transport::tests::*` tests pass.

- [ ] **Step 5: Run the config tests, expect PASS (no regression)**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p fermentool-core config::`
Expected: all existing `config::tests::*` still pass.

- [ ] **Step 6: Commit**

```bash
git add crates/fermentool-core/src/transport.rs crates/fermentool-core/src/lib.rs crates/fermentool-core/src/config.rs
git commit -m "core: transport module, build + swap the pump transport from config"
```

---

## Task 2: `Engine` records and swaps its transport

**Files:**
- Modify: `crates/fermentool-core/src/engine/mod.rs`
  - `Engine<T>` struct (~line 153), add `transport: TransportKind` field
  - `Engine::new` (~line 162), initialise it to `TransportKind::Sim`
  - new methods `set_transport_kind`, `transport_kind`, `swap_transport`
  - `EngineStatus` struct (~line 122), add `transport: String`
  - `Engine::status` (~line 176), populate it
- Test: inline `#[cfg(test)] mod tests` in the same file (helpers `engine()`, `linear_cfg()`, `t0()`, `at()` already exist)

**Interfaces:**
- Consumes: `crate::transport::{SwapTransport, TransportKind}`, `crate::config::SerialConfig` (Task 1).
- Produces:
  - `Engine::set_transport_kind(&mut self, kind: TransportKind)`, used by `build_engine` (Task 5).
  - `Engine::transport_kind(&self) -> &TransportKind`.
  - `Engine::swap_transport(&mut self, serial: &SerialConfig, pump_addr: u8) -> Result<TransportKind>`, `Err(EngineError::Busy)` while a run is active; otherwise rebuilds the pump transport, updates the recorded kind, returns it. In `impl<T: Transport + SwapTransport> Engine<T>`.
  - `EngineStatus { active: Option<ActiveStatus>, transport: String }`, new `transport` field (`"sim"` or port name).

- [ ] **Step 1: Write failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/fermentool-core/src/engine/mod.rs` (near the other lifecycle tests):

```rust
    #[test]
    fn status_reports_the_transport_and_defaults_to_sim() {
        let e = engine();
        assert_eq!(e.status().transport, "sim");
        assert_eq!(e.transport_kind(), &crate::transport::TransportKind::Sim);
    }

    #[test]
    fn swap_transport_is_refused_while_a_run_is_active() {
        let mut e = engine();
        e.start_run(linear_cfg(), t0()).unwrap();
        let sc = crate::config::SerialConfig { path: "sim".into(), baud: 9600 };
        assert!(matches!(e.swap_transport(&sc, 1), Err(EngineError::Busy)));
    }

    #[test]
    fn swap_transport_when_idle_updates_the_recorded_kind() {
        let mut e = engine();
        let sc = crate::config::SerialConfig { path: "sim".into(), baud: 9600 };
        assert_eq!(
            e.swap_transport(&sc, 1).unwrap(),
            crate::transport::TransportKind::Sim
        );
        assert_eq!(e.status().transport, "sim");
    }
```

- [ ] **Step 2: Run them, expect FAIL**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p fermentool-core engine::tests::swap_transport 2>&1 | tail -20`
Expected: compile error, `no method named swap_transport` / `no field transport` / `transport_kind` not found.

- [ ] **Step 3: Add the imports**

At the top of `crates/fermentool-core/src/engine/mod.rs`, with the other `use crate::` lines (near `use crate::store::{...}` ~line 18):

```rust
use crate::config::SerialConfig;
use crate::transport::{SwapTransport, TransportKind};
```

- [ ] **Step 4: Add the struct field**

`Engine<T>` (~line 153) becomes:

```rust
/// Owns the pump and the journal for the lifetime of the process.
pub struct Engine<T: Transport> {
    pump: Pump<T>,
    store: Store,
    app_version: String,
    active: Option<ActiveRun>,
    /// Which transport the pump is currently driving, for the status frame.
    transport: TransportKind,
}
```

`Engine::new` (~line 162) becomes:

```rust
    pub fn new(pump: Pump<T>, store: Store, app_version: impl Into<String>) -> Self {
        Self {
            pump,
            store,
            app_version: app_version.into(),
            active: None,
            transport: TransportKind::Sim,
        }
    }
```

- [ ] **Step 5: Add the three methods**

Add to the `impl<T: Transport> Engine<T>` block (e.g. right after `new`):

```rust
    /// Record which transport this engine is driving (called at boot once the
    /// real port has been opened).
    pub fn set_transport_kind(&mut self, kind: TransportKind) {
        self.transport = kind;
    }

    /// The transport the pump is currently driving.
    pub fn transport_kind(&self) -> &TransportKind {
        &self.transport
    }
```

Add a new `impl` block immediately after the `impl<T: Transport> Engine<T>` block closes:

```rust
impl<T: Transport + SwapTransport> Engine<T> {
    /// Rebuild the pump transport in place from `serial` (simulator ⇄ real
    /// port). Refused while a run is active, stop the run first. Returns the
    /// transport now in use (which may be the simulator if the port failed to
    /// open).
    pub fn swap_transport(
        &mut self,
        serial: &SerialConfig,
        pump_addr: u8,
    ) -> Result<TransportKind> {
        if self.active.is_some() {
            return Err(EngineError::Busy);
        }
        let kind = self.pump.transport_mut().swap(serial, pump_addr);
        self.transport = kind.clone();
        Ok(kind)
    }
}
```

- [ ] **Step 6: Populate `EngineStatus`**

`EngineStatus` (~line 122):

```rust
#[derive(Debug, Clone, Serialize)]
pub struct EngineStatus {
    pub active: Option<ActiveStatus>,
    /// `"sim"` or the open serial port name.
    pub transport: String,
}
```

`Engine::status` (~line 176), add the field to the returned literal:

```rust
    pub fn status(&self) -> EngineStatus {
        EngineStatus {
            active: self.active.as_ref().map(|a| ActiveStatus {
                run_id: a.id,
                started_at: a.started_at,
                duration_s: a.duration_s,
                control_var: a.control_var,
                tick_interval_s: TICK_INTERVAL.as_secs() as u32,
                last_seq: (a.next_seq > 0).then_some(a.next_seq - 1),
                last_target: a.last_target,
            }),
            transport: self.transport.label(),
        }
    }
```

- [ ] **Step 7: Run the engine tests, expect PASS**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p fermentool-core engine::`
Expected: the 3 new tests pass; all existing `engine::tests::*` still pass.

- [ ] **Step 8: Commit**

```bash
git add crates/fermentool-core/src/engine/mod.rs
git commit -m "core/engine: record the live transport, add swap_transport (idle only)"
```

---

## Task 3: `Command::Reconnect` on the control thread

**Files:**
- Modify: `crates/fermentool-core/src/control.rs`
  - `use` block, add `use crate::config::SerialConfig;` and `use crate::transport::{SwapTransport, TransportKind};`
  - `Command` enum (~line 33), add `Reconnect { .. }`
  - `DaemonStatus` struct (~line 58), add `transport: String`
  - `current_status` (~line 108), populate it
  - `spawn` bound (~line 122), `control_loop` bound (~line 142), `handle` bound (~line 259), add `+ SwapTransport`
  - `handle` match (~line 261), add the `Command::Reconnect` arm
- Test: inline `#[cfg(test)] mod tests` (helpers `spawn`, `Command`, `cadence_run()` already there)

**Interfaces:**
- Consumes: `Engine::swap_transport` (Task 2); `crate::config::SerialConfig`; `crate::transport::TransportKind`.
- Produces:
  - `Command::Reconnect { serial: SerialConfig, pump_addr: u8, reply: oneshot::Sender<Result<String, String>> }`, reply is `Ok(human message)` or `Err("a run is already active")`.
  - `DaemonStatus { app_version, active, has_pending_recovery, transport: String }`.

- [ ] **Step 1: Write failing tests**

Add to `#[cfg(test)] mod tests` in `crates/fermentool-core/src/control.rs`:

```rust
    #[tokio::test]
    async fn reconnect_when_idle_reports_the_transport() {
        let engine = Engine::new(
            Pump::new(SimPump::new(1), 1),
            Store::open_in_memory().unwrap(),
            "test",
        );
        let (events, _keep_rx) = broadcast::channel(16);
        let handle = spawn(engine, Duration::from_secs(300), events);

        let serial = crate::config::SerialConfig { path: "sim".into(), baud: 9600 };
        let msg = handle
            .call(|reply| Command::Reconnect { serial, pump_addr: 1, reply })
            .await
            .unwrap()
            .unwrap();
        assert!(msg.to_lowercase().contains("simulator"), "got: {msg}");

        handle.shutdown();
    }

    #[tokio::test]
    async fn reconnect_is_refused_during_a_run() {
        let engine = Engine::new(
            Pump::new(SimPump::new(1), 1),
            Store::open_in_memory().unwrap(),
            "test",
        );
        let (events, _keep_rx) = broadcast::channel(16);
        let handle = spawn(engine, Duration::from_secs(300), events);

        handle
            .call(|reply| Command::StartRun(cadence_run(), reply))
            .await
            .unwrap()
            .unwrap();

        let serial = crate::config::SerialConfig { path: "sim".into(), baud: 9600 };
        let err = handle
            .call(|reply| Command::Reconnect { serial, pump_addr: 1, reply })
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err, "a run is already active");

        handle.shutdown();
    }
```

- [ ] **Step 2: Run them, expect FAIL**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p fermentool-core control::tests::reconnect 2>&1 | tail -20`
Expected: compile error, no variant `Reconnect` on `Command`.

- [ ] **Step 3: Add the imports**

In `crates/fermentool-core/src/control.rs`, with the other `use crate::` lines (~line 22):

```rust
use crate::config::SerialConfig;
use crate::engine::{ActiveStatus, Engine, RecoveryInfo, RunConfig, TickOutcome, TICK_INTERVAL};
use crate::store::{EventRow, RunRow, RunStatus, TickRow};
use crate::transport::{SwapTransport, TransportKind};
```

- [ ] **Step 4: Add the `Command` variant**

In the `Command` enum (~line 33), after `Preview(...)` and before `Shutdown`:

```rust
    /// Rebuild the pump transport from a serial config, without restarting.
    Reconnect {
        serial: SerialConfig,
        pump_addr: u8,
        reply: oneshot::Sender<Result<String, String>>,
    },
    Shutdown,
```

- [ ] **Step 5: Add `transport` to `DaemonStatus` and `current_status`**

`DaemonStatus` (~line 58):

```rust
#[derive(Debug, Clone, Serialize)]
pub struct DaemonStatus {
    pub app_version: String,
    pub active: Option<ActiveStatus>,
    pub has_pending_recovery: bool,
    /// `"sim"` or the open serial port name.
    pub transport: String,
}
```

`current_status` (~line 108), bind the status once and forward the field:

```rust
pub fn current_status<T: Transport>(engine: &Engine<T>, grace: Duration) -> DaemonStatus {
    let st = engine.status();
    DaemonStatus {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        active: st.active,
        has_pending_recovery: engine
            .pending_recovery(Timestamp::now(), grace)
            .map(|r| r.is_some())
            .unwrap_or(false),
        transport: st.transport,
    }
}
```

(`current_status` stays `T: Transport`, it only reads.)

- [ ] **Step 6: Widen the trait bounds**

Add `+ SwapTransport` to these three signatures:

```rust
pub fn spawn<T>(
    mut engine: Engine<T>,
    grace: Duration,
    events: broadcast::Sender<DaemonStatus>,
) -> ControlHandle
where
    T: Transport + Send + SwapTransport + 'static,
{
```

```rust
fn control_loop<T: Transport + SwapTransport>(
    engine: &mut Engine<T>,
    rx: &mpsc::Receiver<Command>,
    grace: Duration,
    events: &broadcast::Sender<DaemonStatus>,
) {
```

```rust
fn handle<T: Transport + SwapTransport>(engine: &mut Engine<T>, cmd: Command, grace: Duration) -> bool {
```

- [ ] **Step 7: Add the `handle` arm**

In `handle`'s `match cmd` (~line 261), after the `Command::Preview` arm and before the closing brace:

```rust
        Command::Reconnect {
            serial,
            pump_addr,
            reply,
        } => {
            let wanted_serial = !serial.use_simulator();
            let msg = engine
                .swap_transport(&serial, pump_addr)
                .map(|kind| match kind {
                    TransportKind::Serial(name) => format!("serial port open: {name}"),
                    TransportKind::Sim if wanted_serial => format!(
                        "could not open {}, running on the pump simulator",
                        serial.path
                    ),
                    TransportKind::Sim => "running on the pump simulator".to_string(),
                })
                .map_err(|e| e.to_string());
            let _ = reply.send(msg);
            true
        }
```

- [ ] **Step 8: Run the control tests, expect PASS**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p fermentool-core control::`
Expected: both new tests pass; existing `control::tests::*` (cadence, setpoint) still pass.

- [ ] **Step 9: Commit**

```bash
git add crates/fermentool-core/src/control.rs
git commit -m "core/control: Command::Reconnect + transport field on DaemonStatus"
```

---

## Task 4: `POST /api/serial/reconnect`

**Files:**
- Modify: `crates/fermentool-core/src/api.rs`
  - new `ReconnectReq` struct + `serial_reconnect` handler (place next to `serial_ports`, ~line 388)
  - route registration in `router` (~line 60)
  - inline tests in `#[cfg(test)] mod tests`
- Test: `crates/fermentool-core/src/api.rs` `#[cfg(test)] mod tests` (helpers `test_state()`, `post_json()`, `get()`, `body_json()`, `linear_curve()` already there)

**Interfaces:**
- Consumes: `Command::Reconnect` (Task 3); `AppState` (`config: Arc<RwLock<Config>>`, `config_path: Arc<PathBuf>`, `control`); `Config::save`.
- Produces: `POST /api/serial/reconnect`, JSON body `{ "path"?: string, "baud"?: number }`, `200 { "connected": "<message>" }`; `409` if a run is active; `400` on config-save failure or a body that isn't valid JSON; `503` if the control thread is gone.

- [ ] **Step 1: Write failing tests**

Add to `#[cfg(test)] mod tests` in `crates/fermentool-core/src/api.rs`:

```rust
    #[tokio::test]
    async fn reconnect_persists_config_and_reports_the_transport() {
        let dir = std::env::temp_dir().join(format!("ft-reconnect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfgp = dir.join("config.toml");

        let mut st = test_state();
        st.config_path = Arc::new(cfgp.clone());
        let app = router(st);

        let res = app
            .oneshot(post_json("/api/serial/reconnect", json!({ "path": "sim" })))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert!(
            v["connected"].as_str().unwrap().to_lowercase().contains("simulator"),
            "got: {v}"
        );

        let written = std::fs::read_to_string(&cfgp).unwrap();
        assert!(written.contains("path = \"sim\""), "config not written: {written}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reconnect_is_conflict_during_a_run() {
        let app = router(test_state());

        let start = post_json(
            "/api/runs",
            json!({
                "name": "t", "control_var": "rpm", "direction": "cw",
                "pump_addr": 1, "curve": linear_curve()
            }),
        );
        assert_eq!(app.clone().oneshot(start).await.unwrap().status(), StatusCode::CREATED);

        let res = app
            .oneshot(post_json("/api/serial/reconnect", json!({ "path": "sim" })))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn reconnect_with_empty_body_uses_current_config() {
        let app = router(test_state());
        let res = app
            .oneshot(post_json("/api/serial/reconnect", json!({})))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn reconnect_rejects_a_non_json_body() {
        let app = router(test_state());
        let req = Request::builder()
            .method("POST")
            .uri("/api/serial/reconnect")
            .header("content-type", "application/json")
            .body(Body::from("not json"))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert!(res.status().is_client_error(), "got: {}", res.status());
    }
```

- [ ] **Step 2: Run them, expect FAIL**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p fermentool-core api::tests::reconnect 2>&1 | tail -20`
Expected: `404 Not Found` from the router (route missing) → assertions fail.

- [ ] **Step 3: Add the route**

In `router` (~line 60), next to the other `/api/serial` route:

```rust
        .route("/api/serial/ports", get(serial_ports))
        .route("/api/serial/reconnect", post(serial_reconnect))
```

- [ ] **Step 4: Add the handler**

In `crates/fermentool-core/src/api.rs`, right after `serial_ports` (~line 403):

```rust
#[derive(Deserialize)]
struct ReconnectReq {
    /// New serial path; omitted = keep the current one.
    #[serde(default)]
    path: Option<String>,
    /// New baud; omitted = keep the current one.
    #[serde(default)]
    baud: Option<u32>,
}

/// Persist `serial.*` to `config.toml` and rebuild the live pump transport
/// without a daemon restart. Refused (409) while a run is active.
async fn serial_reconnect(
    State(s): State<AppState>,
    Json(req): Json<ReconnectReq>,
) -> ApiResult<Response> {
    let mut cfg = s.config.read().await.clone();
    if let Some(p) = req.path {
        cfg.serial.path = p;
    }
    if let Some(b) = req.baud {
        cfg.serial.baud = b;
    }
    cfg.save(&s.config_path)
        .map_err(|e| ApiError::Bad(e.to_string()))?;
    *s.config.write().await = cfg.clone();

    let serial = cfg.serial.clone();
    let pump_addr = cfg.pump.address;
    let msg = s
        .control
        .call(move |reply| Command::Reconnect {
            serial,
            pump_addr,
            reply,
        })
        .await
        .map_err(|_| ApiError::Down)?
        .map_err(ApiError::Conflict)?;

    Ok(Json(json!({ "connected": msg })).into_response())
}
```

- [ ] **Step 5: Run the api tests, expect PASS**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p fermentool-core api::`
Expected: the 4 new tests pass; existing `api::tests::*` still pass.

- [ ] **Step 6: Commit**

```bash
git add crates/fermentool-core/src/api.rs
git commit -m "api: POST /api/serial/reconnect, persist + hot-swap the transport"
```

---

## Task 5: Wire `build_engine` through `transport::open`

**Files:**
- Modify: `crates/fermentool-core/src/main.rs`
  - `build_engine` (~line 86), use `fermentool_core::transport::open` + `set_transport_kind`
  - `use` block (~line 25-26), drop `SerialTransport` and `SimPump` (now unused); keep `Pump`, `Transport`
  - `use std::time::Duration;` (~line 14), drop if `cargo build` reports it unused after the change

**Interfaces:**
- Consumes: `fermentool_core::transport::open` (Task 1), `Engine::set_transport_kind` (Task 2).
- Produces: no new public surface; `build_engine` still returns `anyhow::Result<Engine<Box<dyn Transport + Send>>>`.

- [ ] **Step 1: Rewrite `build_engine`**

Replace the whole function (~lines 86-120) with:

```rust
fn build_engine(cfg: &Config, db: &Path) -> anyhow::Result<Engine<Box<dyn Transport + Send>>> {
    let store = Store::open(db).context("open journal")?;
    let (transport, kind) = fermentool_core::transport::open(&cfg.serial, cfg.pump.address);
    let mut engine = Engine::new(Pump::new(transport, cfg.pump.address), store, VERSION);
    engine.set_transport_kind(kind);
    Ok(engine)
}
```

- [ ] **Step 2: Fix imports**

Change the modbus import line (~line 26) from:

```rust
use fermentool_modbus::serial::SerialTransport;
use fermentool_modbus::{Pump, SimPump, Transport};
```

to:

```rust
use fermentool_modbus::{Pump, Transport};
```

- [ ] **Step 3: Build the binary, expect success, no warnings**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build -p fermentool-core --bin fermentool-core 2>&1 | tail -20`
Expected: `Finished`. If it warns `unused import: std::time::Duration` or similar, delete that `use` line and rebuild.

- [ ] **Step 4: Full workspace test, expect PASS**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test 2>&1 | tail -30`
Expected: all crates' tests pass.

- [ ] **Step 5: Clippy, expect clean**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo clippy --all-targets 2>&1 | tail -20`
Expected: no warnings in the changed files.

- [ ] **Step 6: Commit**

```bash
git add crates/fermentool-core/src/main.rs
git commit -m "core/main: build_engine goes through transport::open"
```

---

## Task 6: Settings page, Rescan, Connect now, "Connected to"

**Files:**
- Modify: `ui/src/routes/Settings.svelte`
- Build artefact: `ui/dist/**` (regenerated by `npm run build`)

**Interfaces:**
- Consumes: `GET /api/serial/ports`, `POST /api/serial/reconnect` (Task 4), `app.status.transport` (Task 3, arrives via `/api/status` + `/api/ws`).
- Produces: no new interface.

- [ ] **Step 1: Replace the `<script>` block**

In `ui/src/routes/Settings.svelte`, replace lines 1-37 (`<script> … </script>`) with:

```svelte
<script>
  import { get, put, post } from '../lib/api.js';
  import { app } from '../lib/state.svelte.js';

  let cfg = $state(null);
  let ports = $state([]);
  let portErr = $state(null);
  let err = $state(null);
  let msg = $state(null);
  let saving = $state(false);
  let reconnecting = $state(false);
  let stopped = $state(false);

  $effect(() => {
    get('/api/config').then((c) => (cfg = c)).catch((e) => (err = e.message));
    rescan();
  });

  function rescan() {
    portErr = null;
    get('/api/serial/ports')
      .then((r) => (ports = r.ports ?? []))
      .catch((e) => (portErr = e.message));
  }

  async function reconnect() {
    reconnecting = true;
    err = null;
    msg = null;
    try {
      const r = await post('/api/serial/reconnect', {
        path: cfg.serial.path,
        baud: cfg.serial.baud,
      });
      msg = r.connected ? `Connected, ${r.connected}` : 'Reconnected.';
    } catch (e) {
      err = e.message;
    }
    reconnecting = false;
  }

  async function save() {
    saving = true;
    err = null;
    msg = null;
    try {
      const r = await put('/api/config', cfg);
      msg = r.note ? `Saved, ${r.note}` : 'Saved.';
    } catch (e) {
      err = e.message;
    }
    saving = false;
  }

  async function shutdown() {
    try {
      await post('/api/shutdown');
      stopped = true;
    } catch (e) {
      err = e.message;
    }
  }
</script>
```

- [ ] **Step 2: Replace the serial-port field markup**

Replace the single `<label class="field">` block for the serial port (lines 53-59 in the original, the one containing `list="ports"`) with:

```svelte
      <label class="field"><span>Serial port ("sim" for the simulator)</span>
        <input type="text" list="ports" bind:value={cfg.serial.path} />
        <datalist id="ports">
          {#each ports as p}<option value={p.name}>{p.name}, {p.product ?? p.kind}</option>{/each}
          <option value="sim">sim</option>
        </datalist>
        <div class="port-row">
          <button type="button" class="btn-ghost" onclick={rescan}>Rescan</button>
          <button type="button" class="btn-ghost" disabled={reconnecting} onclick={reconnect}>
            {reconnecting ? 'Connecting…' : 'Connect now'}
          </button>
          <span class="port-now">Connected to: <b>{app.status?.transport ?? ', '}</b></span>
        </div>
        {#if portErr}<div class="err" style="margin-top:8px">{portErr}</div>{/if}
      </label>
```

- [ ] **Step 3: Add CSS**

In the `<style>` block at the bottom of `ui/src/routes/Settings.svelte`, add:

```css
  .port-row {
    display: flex;
    align-items: center;
    gap: var(--s-2);
    margin-top: var(--s-2);
    flex-wrap: wrap;
  }
  .port-now { color: var(--muted); font-size: 13px; }
```

- [ ] **Step 4: Build the UI**

Run: `cd ui && npm run build`
Expected: `vite build` completes; `ui/dist/` updated. (Run from a shell where `npm` is available; on this machine that is PowerShell or the same Bash session.)

- [ ] **Step 5: Manual smoke test**

Run the daemon: `export PATH="$HOME/.cargo/bin:$PATH" && cargo run -p fermentool-core --bin fermentool-core` (leave it running in another shell), open `http://127.0.0.1:8730/`, go to **Settings**:
- Click **Rescan**, the port list refreshes; if `/api/serial/ports` errors it shows under the field instead of silently doing nothing.
- With no run active, type/pick `COM3` (or the port from Task-0 discovery), click **Connect now**, a green "Connected, serial port open: COM3" (or "could not open … simulator") message appears, and "Connected to:" updates.
- Start a run, then **Connect now**, a red "a run is already active" message; the transport does not change.

- [ ] **Step 6: Commit**

```bash
git add ui/src/routes/Settings.svelte ui/dist
git commit -m "ui/Settings: rescan ports + connect to a serial port without restart"
```

---

## Self-Review

**1. Spec coverage**

| Spec section | Task |
|---|---|
| §1 extract transport construction → `transport::open` returning kind | Task 1 (fn `open`), Task 5 (`build_engine` uses it) |
| §2 `SwapTransport` trait (box + SimPump impls), `Engine::swap_transport` refused mid-run | Task 1 (trait+impls), Task 2 (`swap_transport`) |
| §3 `Command::Reconnect` + `POST /api/serial/reconnect` (persist + swap, 409 on active run) | Task 3 (command+handler), Task 4 (endpoint) |
| §4 `transport: String` on the top-level `DaemonStatus` | Task 3 (`DaemonStatus`, `current_status`); Task 2 supplies it via `EngineStatus` |
| §5 UI: Rescan button, Connect button, `.catch` surfaces errors, "Connected to" readout | Task 6 |
| §5 keep free-text `<input list="ports">` | Task 6 (kept) |
| Testing bullets (modbus/core, engine, control, api) | Tasks 1/2/3/4 test steps |
| Risk: port held elsewhere → fallback made explicit in the response string | Task 3 `handle` arm (`could not open … simulator`) |
| Out of scope: mid-run reconnect, hot-plug watcher | not implemented (guard returns `EngineError::Busy`) |

No gaps.

**2. Placeholder scan**, no `TBD`/`TODO`/"handle edge cases"/"similar to Task N"; every code step has full code; the one conditional step (Task 5 Step 3, drop `Duration` import only if it warns) has an explicit trigger and action.

**3. Type consistency**

- `TransportKind`, defined `crate::transport` (Task 1); imported in `engine` (Task 2) and `control` (Task 3); `.label()` used in `Engine::status` (Task 2) and `current_status` (Task 3). ✓
- `SwapTransport`, defined Task 1; bound added to `Engine` impl block (Task 2) and `spawn`/`control_loop`/`handle` (Task 3). `SimPump: SwapTransport` (Task 1) covers `engine()` test helper (`Engine<SimPump>`) and api/control test states. ✓
- `open(serial: &SerialConfig, pump_addr: u8) -> (Box<dyn Transport + Send>, TransportKind)`, same signature in Task 1 def, Task 1 `SwapTransport for Box` impl, Task 5 `build_engine`. ✓
- `Engine::swap_transport(&mut self, serial: &SerialConfig, pump_addr: u8) -> Result<TransportKind>`, Task 2 def; called in Task 3 `handle` with `(&serial, pump_addr)`. `Result` is engine's alias `Result<T, EngineError>`. ✓
- `Command::Reconnect { serial: SerialConfig, pump_addr: u8, reply: oneshot::Sender<Result<String, String>> }`, Task 3 def; constructed in Task 3 tests and Task 4 handler with the same field names. ✓
- `EngineStatus { active, transport: String }`, Task 2; consumed in Task 3 `current_status` as `st.transport`. ✓
- `DaemonStatus { app_version, active, has_pending_recovery, transport: String }`, Task 3; serialised to JSON, read by UI as `app.status.transport` (Task 6). ✓
- `SerialConfig::use_simulator()`, Task 1; used in Task 1 `open`, Task 3 `handle` arm. ✓
- API: `ReconnectReq { path: Option<String>, baud: Option<u32> }`, response `{ "connected": <string> }`, Task 4 def; UI posts `{path, baud}` and reads `r.connected` (Task 6). ✓

Consistent.
