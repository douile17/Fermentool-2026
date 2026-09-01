# Fermentool

Drives a Baoding Shenchen **LabQ** peristaltic pump over MODBUS-RTU so its
setpoint (**rpm** *or* **ml/min**) follows a chosen **time profile** — linear,
exponential fed-batch, sigmoid, step, constant or custom — over runs that average
~100 h, with **crash-safe journalling** and **time-correct resume** after any
interruption.

> Status: **scaffold (milestone 1)**. The curve math and MODBUS framing crates
> have their real cores + tests; the control engine, SQLite journal, serial link,
> HTTP API and web UI are the next milestones. Full design:
> [`docs/IMPLEMENTATION_PLAN.md`](docs/IMPLEMENTATION_PLAN.md).

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
| `crates/fermentool-curves` | Pure time-profile math, no I/O. Fully unit-tested. |
| `crates/fermentool-modbus`  | MODBUS-RTU CRC/framing + LabQ register map. Byte-exact tests vs. the vendor doc. |
| `crates/fermentool-core`    | The daemon: engine, store, serial, HTTP API, embedded UI. |
| `ui/`                       | Svelte 5 + Vite front-end; built into `crates/fermentool-core/assets/`. |
| `docs/`                     | Implementation plan, wiring, service install. |
| `LabQ Series MODBUS protocol.{md,pdf}` | Vendor protocol reference. |

## Build

Prerequisites: [Rust (stable, via rustup)](https://rustup.rs) and Node.js ≥ 18.

```sh
# Rust crates + tests
cargo test
cargo run -p fermentool-core

# Web UI (built output goes into the core crate's assets/)
cd ui
npm install
npm run dev      # dev server on :5173, proxies /api to the daemon on :8730
npm run build    # production build embedded by the daemon
```

## License

MIT.
