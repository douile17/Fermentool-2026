# Live serial reconnect + port rescan

**Date:** 2026-09-02
**Status:** approved, ready for implementation plan

## Problem

Plugging in a USB-RS485 adapter after the daemon has started is useless without
a full daemon restart:

- `build_engine()` ([crates/fermentool-core/src/main.rs](../../../crates/fermentool-core/src/main.rs))
  constructs the transport once, at boot. Changing `[serial] path` via
  `PUT /api/config` only rewrites the file; the running `Engine` keeps its
  original transport. `put_config` even says so: `"port and serial changes take
  effect on restart"`.
- `Settings.svelte` fetches `/api/serial/ports` once on mount and swallows any
  error (`.catch(() => {})`), so a device attached later never shows up and a
  failed fetch is invisible.

The port-enumeration backend itself is fine: `/api/serial/ports` calls
`serialport::available_ports()` live on every request and already returns the
adapter (verified: `COM3`, FTDI `0403:6001`).

## Goal

From the Settings page, with the daemon running and **no active run**:

1. **Rescan**, refetch the port list on demand; surface fetch errors.
2. **Connect**, switch the live `Engine` transport (sim → `COM3`, or port →
   port) without restarting the daemon, persisting the choice to `config.toml`.
3. See which transport is actually connected.

Out of scope for v1: reconnecting while a run is active (must stop → reconnect →
start a new run); automatic port-change detection / hot-plug watcher.

## Design

### 1. Transport construction, extracted

Pull the sim-vs-serial `match` out of `build_engine` into a reusable function in
`fermentool-core` (e.g. `transport::open(serial: &SerialConfig, pump_addr: u8)
-> Box<dyn Transport + Send>`):

- `serial.path` empty or `"sim"` (case-insensitive) → `SimPump::new(pump_addr)`.
- otherwise `SerialTransport::open(path, baud, 1500 ms)`; on error, log and fall
  back to `SimPump::new(pump_addr)` (same behaviour as today).

Return value also needs to report **what actually opened** so the status frame
can show it. Either return `(Box<dyn Transport + Send>, TransportKind)` where
`TransportKind` is `Sim` or `Serial(String)`, or expose it via a getter the
`Engine` stores. Prefer returning the pair; `build_engine` and the reconnect
handler both use it.

### 2. `SwapTransport` trait + `Engine::swap_transport`

```rust
/// A transport that can be rebuilt in place from a serial config.
pub trait SwapTransport {
    fn swap(&mut self, serial: &SerialConfig, pump_addr: u8) -> TransportKind;
}
```

- `impl SwapTransport for Box<dyn Transport + Send>`, `*self = transport::open(..)`.
  Assigning drops the old value; `SerialTransport`'s `Drop` closes the port.
- `impl SwapTransport for SimPump`, no-op returning `TransportKind::Sim`.
  `Engine<T: Transport>` holds `pump: Pump<T>`, and `control.rs`'s tests build
  `Engine::new(Pump::new(SimPump::new(1), 1), ...)`, i.e. `T = SimPump`, so this
  impl is what keeps `spawn` / `control_loop` compiling there. Only these two
  concrete impls; no blanket impl.

`Engine::swap_transport(&mut self, serial: &SerialConfig, pump_addr: u8)
-> Result<TransportKind>`:

- if `self.active.is_some()` → `Err(EngineError::…("stop the run before
  reconnecting"))`.
- else delegate to `self.pump.transport_mut().swap(serial, pump_addr)`, store the
  resulting `TransportKind` on the `Engine` for status, return it.

The `control_loop` / `spawn` generic bound gains `+ SwapTransport`.

### 3. `Command::Reconnect` + endpoint

```rust
Command::Reconnect {
    serial: SerialConfig,          // resolved path + baud
    pump_addr: u8,
    reply: oneshot::Sender<Result<String, String>>,
}
```

Handler (control thread): call `engine.swap_transport`, map the result to a
human string, `"serial port open: COM3"` / `"COM3 unavailable, running on the
simulator"` / the run-active error.

`POST /api/serial/reconnect`, body `{ "path"?: string, "baud"?: number }`:

1. Build the new `Config` from the current one with `serial.path` / `serial.baud`
   overridden by the body (falling back to current values).
2. `new.save(&s.config_path)` then `*s.config.write().await = new.clone()`
   (same as `put_config`).
3. Send `Command::Reconnect { serial: new.serial, pump_addr: new.pump.address }`.
4. `200` with `{ "connected": "<string from handler>" }`; `409` if the handler
   reports an active run; `4xx`/`5xx` on save / channel failure.

### 4. Status frame

Add `transport: String` to the top-level `DaemonStatus` (not `ActiveStatus`,
it's meaningful when idle too). Value: `"sim"` or the open port name, from the
`TransportKind` the `Engine` stores. Populated in `current_status`.

### 5. UI, `Settings.svelte`

- Extract the port fetch into a `rescan()` function; call it from `$effect` and
  from a new **Rescan** button next to the port field. Replace `.catch(() => {})`
  with `.catch((e) => (portErr = e.message))` and render `portErr`.
- **Connect** button → `post('/api/serial/reconnect', { path: cfg.serial.path,
  baud: cfg.serial.baud })`; show the returned `connected` string in the
  existing `msg` slot, or the error in `err`. On `409`, message = "stop the run
  first".
- Small line under the field: `Connected to: {status.transport}` read from the
  shared `app.status`.
- Keep the free-text `<input list="ports">`. Drop the "restart to apply" framing
  for serial (API port still needs restart).

## Testing

- **modbus / core**: `SwapTransport for Box<dyn Transport + Send>` swaps
  sim→sim and preserves the pump address.
- **engine**: `swap_transport` → `Err` while a run is active; `Ok(Sim)` when idle;
  `TransportKind` is stored and surfaced by `status()`.
- **control**: `Command::Reconnect` idle → `Ok`; during a run → `Err` string.
- **api**: `POST /api/serial/reconnect` → `200`, config file rewritten, in-memory
  config updated; active run → `409`; malformed body → `400`.

## Risks / notes

- Opening a port another process holds → falls back to sim; the response string
  makes that explicit rather than looking like success.
- USB unplug can leave a blocked port handle; v1 only swaps on explicit user
  action while idle, so a stuck `SerialTransport::open` is bounded by its
  1500 ms timeout.
- `config.toml` at `%APPDATA%\Fermentool\config.toml` is the single source of
  truth; reconnect keeps it in sync so a later restart matches the live state.
