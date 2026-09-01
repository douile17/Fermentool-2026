# Fermentool

Drives a Baoding Shenchen **LabQ** peristaltic pump over MODBUS-RTU so its
setpoint (**rpm** *or* **ml/min**) follows a chosen **time profile** — linear,
exponential fed-batch, sigmoid, step, constant or custom — over runs that average
~100 h, with **crash-safe journalling** and **time-correct resume** after any
interruption.

> Status: **milestone 7 — the daemon runs.** `fermentool-core` now loads `config.toml`,
> opens the journal, builds the engine (real serial port or a built-in simulator), runs
> it on a dedicated control thread, and serves a REST API on `127.0.0.1:8730`:
> status · config · preview · runs (create/list/get/ticks/events/stop/abort) · recovery
> (get/resume/discard) · serial ports · shutdown. Rolling-file logging; graceful
> shutdown on ctrl-c or `POST /api/shutdown`; port-in-use is the single-instance guard.
> 66 tests green + a full manual API smoke.
> Next: the web UI (Svelte) served by the daemon, with a WebSocket for live updates.
> Full design: [`docs/IMPLEMENTATION_PLAN.md`](docs/IMPLEMENTATION_PLAN.md) ·
> visual language: [`docs/DESIGN.md`](docs/DESIGN.md).

## Architecture

One self-contained native binary per OS (Windows / Linux / macOS). It owns the
serial port, runs the curve engine, writes every tick to SQLite, **and** serves
the web UI at `http://localhost:8730`. The UI is a disposable browser tab — it
rebuilds its state from the daemon on reconnect, so closing it (or the whole PC)
never loses a run. The pump keeps its last commanded speed with no further
frames, so "hold last setpoint on crash" is inherent.

Hardware: PC → isolated FTDI **USB↔RS485** adapter → pump `A/B/GND`. No
microcontroller. (An ESP32-S3 sensor/vision hub may be added later, off the
control path.)

## Layout

| Path | What |
|---|---|
| `crates/fermentool-curves` | Pure time-profile math (`CurveSpec`), no I/O. Fully unit-tested. |
| `crates/fermentool-modbus`  | MODBUS-RTU CRC/framing + LabQ register map & limits. Byte-exact tests vs. the vendor doc. |
| `crates/fermentool-core`    | The daemon — `config` · `store` (SQLite journal) · `engine` (curve engine + crash recovery) · `control` (engine on its own thread) · `api` (axum REST) · embedded UI (m8). |
| `ui/`                       | Svelte 5 + Vite front-end; built to `ui/dist/`, embedded by the daemon at milestone 8. |
| `docs/`                     | Implementation plan, design language, wiring, service install. |
| `LabQ Series MODBUS protocol.{md,pdf}` | Vendor protocol reference. |

## Build

Prerequisites: [Rust (stable, via rustup)](https://rustup.rs) and Node.js ≥ 18.

```sh
# Rust crates + tests
cargo test

# Run the daemon. With no config it writes <data-dir>/config.toml and uses the
# pump SIMULATOR (serial.path = "sim"). API on http://127.0.0.1:8730.
cargo run -p fermentool-core
#   curl http://127.0.0.1:8730/api/status
#   curl -XPOST http://127.0.0.1:8730/api/shutdown

# pure logic only, no serial backend (hosts without libudev/pkg-config):
cargo test -p fermentool-modbus --no-default-features

# Web UI (built output goes to ui/dist/)
cd ui
npm install
npm run dev      # dev server on :5173, proxies /api to the daemon on :8730
npm run build    # production build embedded by the daemon
```

## License

MIT.
