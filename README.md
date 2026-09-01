# Fermentool

Drives a Baoding Shenchen **LabQ** peristaltic pump over MODBUS-RTU so its
setpoint (**rpm** *or* **ml/min**) follows a chosen **time profile** — linear,
exponential fed-batch, sigmoid, step, constant or custom — over runs that average
~100 h, with **crash-safe journalling** and **time-correct resume** after any
interruption.

> Status: **milestone 5**. `fermentool-curves`, `fermentool-modbus`, and
> `fermentool-core::store` complete. `fermentool-core::engine` now has `Engine<T>`:
> `start_run` (clamps ∩ pump limits, validate, pump start sequence), `tick(now)`
> (wall-clock elapsed → setpoint → pump write → journal; write failures are logged and
> the run continues; auto-finish at the curve's end), `stop_run` / `abort_run`. Time is
> injected, so a 100 h run is tested in milliseconds. 49 tests green.
> Next: crash recovery / resume, then the HTTP API and web UI.
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
| `crates/fermentool-core`    | The daemon: engine, store, serial, HTTP API, embedded UI. |
| `ui/`                       | Svelte 5 + Vite front-end; built to `ui/dist/`, embedded by the daemon at milestone 8. |
| `docs/`                     | Implementation plan, design language, wiring, service install. |
| `LabQ Series MODBUS protocol.{md,pdf}` | Vendor protocol reference. |

## Build

Prerequisites: [Rust (stable, via rustup)](https://rustup.rs) and Node.js ≥ 18.

```sh
# Rust crates + tests
cargo test
cargo run -p fermentool-core
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
