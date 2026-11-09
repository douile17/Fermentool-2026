# Fermentool

Drives a peristaltic pump over MODBUS-RTU (Baoding Shenchen **LabQ**,
**LabV**, Innofluid **PDS II**) so its setpoint (**rpm** *or* **ml/min**)
follows a chosen **time profile**, linear, exponential fed-batch, sigmoid,
steps, constant or custom, over runs that average ~100 h, with **crash-safe
journalling** and **time-correct resume** after any interruption.

> Status: **in use on the bench, version 0.1.6** (Windows installer, see
> [`docs/RELEASE.md`](docs/RELEASE.md)). The daemon serves the Svelte UI at
> **http://127.0.0.1:8730**: Overview, New run, History, Tubing calibration,
> Settings. User-facing changes: [`CHANGELOG.md`](CHANGELOG.md). Full design:
> [`docs/IMPLEMENTATION_PLAN.md`](docs/IMPLEMENTATION_PLAN.md) · visual
> language: [`docs/DESIGN.md`](docs/DESIGN.md) · agent notes:
> [`CLAUDE.md`](CLAUDE.md).

## Running a feed

A run is set up in **New run**, in four steps: Run (name, responsible),
Pump and tube, Feed profile, Check and start.

**Regulation.** Two ways to drive the pump:

- **Balance regulation**: the feed bottle sits on a balance and the daemon
  corrects the pump so the weighed flow follows the curve. It needs a
  balance set up (Settings, Balance) and asks for the tube on the pump:
  a calibration from **Tubing calibration** (three measurements), measured
  on the feed pump's model. In rpm a tube is required; in ml/min it is
  optional (an rpm calibration then drives the pump in rpm, without one
  the correction starts at ×1.00). Optional automatic refill of the bottle
  by a second pump.
- **No regulation**: no tube is asked. The pump gets the rpm or ml/min
  as typed. In ml/min it turns the flow into rpm with its own head and
  tubing setting, so calibrate the pump itself for the tube installed.

**Feed profile.** Linear, Exponential (endpoints, or µ with the fed-batch
F₀ calculator), Sigmoid, Constant, and **Steps**: a table of rates, each
from a given time, up to 1000 rows, imported from Excel (`.xlsx`, `.csv`)
with a downloadable template. A step that starts before the previous one is
refused. Past the curve's end, every run holds its last value until Stop.

A Constant run's rate can be changed during the run. History shows each
run's verdict, the flow against the curve, the running total, the feed
bottle and the journal, and starts it again with **Run again**.

## Architecture

One self-contained native binary per OS (Windows / Linux / macOS). It owns the
serial port, runs the curve engine, writes every tick to SQLite, **and** serves
the web UI at `http://localhost:8730`. The UI is a disposable browser tab, it
rebuilds its state from the daemon on reconnect, so closing it (or the whole PC)
never loses a run. The pump keeps its last commanded speed with no further
frames, so "hold last setpoint on crash" is inherent.

Hardware: PC → FTDI **USB↔RS485** adapter → pump `A/B` (feed pump, then
an optional refill pump on the same cable), plus an optional balance on its
own serial port. No microcontroller ([`docs/wiring.md`](docs/wiring.md)). (An ESP32-S3 sensor/vision hub may be added later, off the
control path.)

## Layout

| Path | What |
|---|---|
| `crates/fermentool-curves` | Pure time-profile math (`CurveSpec`), no I/O. Fully unit-tested. |
| `crates/fermentool-modbus`  | MODBUS-RTU CRC/framing + LabQ / LabV / PDS II register maps & limits. Byte-exact tests vs. the vendor doc. |
| `crates/fermentool-core`    | The daemon, `config` · `store` (SQLite journal) · `engine` (curve engine, crash recovery, refill, priming) · `tracking` (balance regulation) · `control` (engine on its own thread) · `api` (axum REST) · `notify` (ntfy / Teams) · embedded UI. |
| `ui/`                       | Svelte 5 + Vite front-end; built to `ui/dist/`, embedded by the daemon. Testable logic in `ui/src/lib/`. |
| `src-tauri/`                | Windows desktop shell (window + tray) and NSIS installer. |
| `docs/`                     | Implementation plan, design language, release steps, wiring, service install, session notes. |
| `LabQ Series MODBUS protocol.{md,pdf}`, `LabV Series MODBUS protocol.md`, `PDS Series MODBUS protocol.md` | Vendor protocol references. |

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
npm run check    # ESLint + svelte-check, zero warnings
npm test         # vitest on src/lib
npm run smoke    # walks the deployed app in headless Edge, changes nothing
```

### Windows desktop app (Tauri)

A native window + tray shell over the same UI, with an installer that also
registers the daemon to start at log on. Needs `cargo install tauri-cli`
(v2) and the MSVC build tools.

Exporting an installer for another PC: follow [`docs/RELEASE.md`](docs/RELEASE.md).

```powershell
npm --prefix ui ci ; npm --prefix ui run build
./src-tauri/scripts/copy-sidecar.ps1     # builds ui/dist + stages the daemon sidecar
cargo tauri build                        # -> target/release/bundle/nsis/Fermentool_<version>_x64-setup.exe
```

`cargo tauri dev` runs it against a live daemon (run `copy-sidecar.ps1` first).
The installer is unsigned, Windows SmartScreen warns on first run; code signing
is a follow-up. Design: `docs/superpowers/specs/2026-09-10-tauri-webview-shell-design.md`.

## License

MIT.
