# CLAUDE.md

Agent notes for working in this repo. Architecture and setup are in `README.md`;
full design in `docs/IMPLEMENTATION_PLAN.md` and `docs/DESIGN.md`.

## What this is

A single native daemon that drives a Baoding Shenchen **LabQ** peristaltic pump
over MODBUS-RTU so its setpoint (**rpm** or **ml/min**) follows a time profile
over ~100 h runs, with crash-safe journalling and time-correct resume. It also
serves the Svelte UI at `http://127.0.0.1:8730`.

## Build / test / run

```sh
cargo test                                   # all Rust crates
cargo test -p fermentool-curves              # curve math only (fast, no serial backend)
cargo test -p fermentool-modbus --no-default-features   # no libudev/pkg-config
cargo run -p fermentool-core                 # daemon + UI on :8730 (simulator by default)
```

UI (`ui/`): `npm run dev` (proxies `/api` to :8730). **After any change under
`ui/src/`, run `npm run build` from `ui/`, the daemon embeds `ui/dist/`.** Do
this without being asked.

## Windows packaging

**Exporting the installer: follow `docs/RELEASE.md`.** Every exported
installer must install from scratch on a fresh Windows 10/11 PC with nothing
else to install:
- MSVC CRT linked statically (`.cargo/config.toml`, `+crt-static`). The Tauri
  window is static by itself, the sidecar was not: on a clean PC it needed
  `VCRUNTIME140.dll` and failed silently (detached), so the window never
  reached its daemon. `copy-sidecar.ps1` refuses a sidecar that still imports
  it.
- WebView2 through `webviewInstallMode: downloadBootstrapper` (~5 MB
  installer): Windows 10/11 already ship it, the bootstrapper only downloads
  it if missing. `offlineInstaller` makes a ~220 MB installer; use it only
  when a target PC is really offline and lacks WebView2.
- **A feature "missing" on another PC: check what gates it first.** The
  balance indicator and the trim checkbox only exist once `[scale] path` is
  set (Settings, Balance card, or `config.toml`). 0.2.0 was a 220 MB rebuild
  shipped for a problem that was only this empty setting.
- **Always ship with an empty history.** The history (runs, journal,
  calibrations) lives in `%APPDATA%\Fermentool\fermentool.sqlite`, created at
  first launch, never in the exe or the installer. Never add a `.sqlite` (or
  `config.toml`) to the bundle `resources`, nor seed one from this PC's data
  dir: a fresh install must start with no runs. An update on a PC keeps that
  PC's own history.
- **Bump the version on every export** (`Cargo.toml` workspace version and
  `src-tauri/tauri.conf.json`): two builds with the same number cannot be told
  apart on the target PC.
- The NSIS pre-install hook (`src-tauri/installer/hooks.nsh`) stops a running
  daemon cleanly before copying (a locked exe is otherwise left at the old
  version) and aborts if a run is active.
- `copy-sidecar.ps1` runs native tools with `$ErrorActionPreference =
  "Continue"` and checks `$LASTEXITCODE`: Windows PowerShell 5.1 turns cargo's
  normal stderr progress into a fatal error under `Stop`.

- **Deliverable exe:** `cargo build --release -p fermentool-core` →
  `target/release/fermentool-core.exe`. Self-contained (UI embedded via
  rust-embed), windowless (`windows_subsystem = "windows"`), opens the browser
  on launch.
- **Rule: a `cargo build` success is not "done".** After building, always run
  the exe and smoke-test it (`curl 127.0.0.1:8730/api/status`, then a clean
  `POST /api/shutdown`) before calling it working. Kill any daemon still
  holding the exe first, or the release link fails with `os error 5`.
- **Desktop app: built, in `src-tauri/`** (Tauri v2). Window renders the
  bundled `ui/dist`; the daemon ships as a detached sidecar (survives the
  window closing, that guarantee is verified by killing `fermentool.exe` and
  confirming `fermentool-core.exe` + `/api/status` are still alive, a hard
  gate before any change here ships). Tray: Open / Shut down daemon & quit /
  Quit (leave daemon running). Installer: `./src-tauri/scripts/copy-sidecar.ps1`
  then `cargo tauri build` → NSIS `.exe` that also registers an at-logon
  scheduled task (`Fermentool`, `RestartOnFailure` PT1M×3). Design + rationale:
  `docs/superpowers/specs/2026-09-10-tauri-webview-shell-design.md`; build
  steps: `docs/superpowers/plans/2026-09-10-tauri-desktop-shell.md`.
  `docs/IMPLEMENTATION_PLAN.md` §9's "no core changes" is relaxed to exactly
  one `CorsLayer` in `api.rs` + the `BASE` constant in `ui/src/lib/api.js`
  (the bundled UI is now a separate origin from the API).

## Conventions

- **Commit messages: no `Co-Authored-By: Claude` trailer** and no
  `Claude-Session` line in this project.
- **No em dash (—), and no spaced hyphen (` - `) as a stand-in for one.** Use
  normal comma / colon / period punctuation, everywhere: code comments, UI
  strings, docs, commit messages, git tags, GitHub release notes and asset
  labels. (We swept the whole repo for this twice, once wrong, replacing `—`
  with `, `, which is itself the anglicism to avoid, then again with real
  punctuation. Don't reintroduce either.)
- **Pump is driven by rpm or ml/min only.** Never write the pump's head /
  tubing registers. An ml/min run given an **rpm** tubing calibration is
  *driven in rpm* (`Engine::drive_ml_per_rpm`): curve, journal, volume and
  trim stay in ml/min, only the value written and read back is rpm
  (`write_setpoint` / `read_actual` / `drive_grid`), so the pump's own
  head/tubing table never matters. Refused if the curve needs more than
  350 rpm.
- Simulator runs are refused unless `serial.allow_simulator = true`
  (`config.toml`). A real port configured but not open also refuses to
  start/resume, see `EngineError::SerialDown` / `SimulatorNotAllowed`.
- Setpoint is written to the pump ~every 150 ms (`apply_setpoint`), decoupled
  from the 1 s journal tick. The pump needs a ≥100 ms inter-frame gap.
- Errors from `start_run` / `resume` are structured: `EngineError::detail()` →
  `{ message, code, hint }`. `message` stays one line (shown always); `hint` is
  the longer operator-facing explanation (shown behind a "Why?" toggle in
  `NewRun.svelte` / `ResumeModal.svelte`, via `ErrorText.svelte`). When adding
  a new `EngineError` variant that needs more than one line to explain, add it
  to `hint()`, not to `Display`: `Display` must stay short.
  **Don't assert a hardware-error root cause as fact from a hunch.** A
  `PumpRejectedSetpoint` (MODBUS 0x03) on ml/min was documented as "no
  head/tubing configured" until a real 0x03 was reproduced and turned out to
  be "ml/min value above what the configured head + tubing can deliver" (a
  different failure). State the confirmed cause; mention the other as
  secondary if it's a real alternate cause, not the default explanation.

## Serial I/O and the control thread

All pump I/O funnels through one method, `Transport::transaction(&[u8])`
(`fermentool-modbus`). The daemon never calls a transport directly: it goes
through `WatchdogTransport` (`crates/fermentool-core/src/transport.rs`), which
runs the real port on a dedicated `serial-worker` thread and gives the caller
at most `HARD_TXN_TIMEOUT` (2 s) before returning `TransportError::Timeout`.

**Why:** a USB-serial adapter surprise-removed mid-transaction can leave the
OS `read()`/`write()` blocked past the port's own configured timeout,
indefinitely on some drivers. That call used to run directly on the control
thread, which also services the command channel, so a single wedged syscall
froze `/api/status`, Stop, and every other endpoint: the worst failure mode
for a daemon meant to run unattended. `swap`/`swap_strict` (auto-recovery,
operator reconnect) retire a wedged worker (dropped, never `join`ed: it may
be stuck in a syscall forever) and spawn a fresh one on a clean handle.

**Rule: any blocking call reachable from the control thread must be wrapped so
a stuck syscall can only block that one call, never the daemon.** Don't assume
a configured timeout will actually fire at the OS level.

Bounded is not enough either: the control thread serves every command one at
a time, so a wait that is merely *slow* (a silent balance costs each SICS read
its 1.5 s timeout) queued the whole UI behind it (History and Calibration took
seconds to load with the balance off). Hence:
- The balance is a `scale::PolledScale`: its own `scale-poll` thread asks for
  a weight every 500 ms and keeps the latest answer; the engine's read returns
  it at once. A silent balance is a `Timeout` (keep the link, the thread sees
  it come back), a stuck thread or broken port an `Io` error (reopen). A
  reopened port is `scale_unconfirmed` until a weight arrives: an open COM
  port is not a connected balance, and counting it as one made a switched-off
  balance flap lost/recovered every 20 s.
- The API reads the journal on its own SQLite connection (`api::ApiDb`, via
  `spawn_blocking`): runs, ticks, events, tracking, calibrations and the
  calibration draft never go through the control thread. Only what needs the
  live engine (start/stop, delete a run, status, scale/serial changes) does.
  Every connection has a 5 s `busy_timeout`.

## Crash-resume clock anchor

`run_now()` (`control.rs`) has a clock-step guard: it compares wall-clock
elapsed since a run's `started_at` against monotonic elapsed since a
`run_epoch` anchor, and if they disagree by more than `CLOCK_STEP_LIMIT`
(5 s) it assumes the system clock jumped and pins elapsed to the monotonic
side. **The anchor carries `elapsed_at_anchor`** (the run's real elapsed time
when the anchor was taken) precisely because a *resume* takes the anchor with
the run already minutes/hours in: without that term, every resume looked
like a multi-minute clock step, silently rewound the curve to elapsed ≈ 0, and
flooded the log with `system clock stepped mid-run`. If you touch `run_now` /
`run_epoch`, keep that term; a regression here is invisible until a real
crash-resume happens hours into a run.

## Run end: the hold phase

A dosing run does **not** finish when its curve does: past `duration_s` it
holds the curve's end value (`value_at` is flat there), still trimmed and
journalled every second, until Stop, which records it `completed` (`stopped`
before the curve end). `ActiveStatus::curve_done` marks the phase; the
end-of-curve pop-up only informs. A crash in the hold resumes into it, any
time later. Calibration bursts are the exception: they still finish and stop
the pump at their duration (their weighing window). `HoldingStatus` /
`stop_pump` remain only for a hold left by an older daemon.

## Gravimetric trim: regulation and alarms (`tracking.rs`, `engine::scale_tick`)

- c starts at the tubing calibration's `c0` (`Tracker::c_seed`). Before the
  delivery ratio exists, `next_c_early` pays back the cumulative deficit
  around the seed (0.3 g dead band); a deficit past noise (10 balance steps
  and 5 %) uses `early_ratio` (a slope, so the ~2 s start-up lag does not read
  as a short pump) at full speed.
- Ratio window: 50 balance steps / 120 s first, then 200 steps / 300 s
  ("settled") with 0.5 % steps instead of 2 %. A 5 g window kept for a whole
  run made c hunt ±10 % (run 19); the settled window fixed it. Every
  threshold is in **balance steps** (`scale_resolution_g`), so a 1 g balance
  scales them, except `WRONG_SIDE_MIN_G` (still 2 g fixed).
- Bounds come from `[scale] trim_limit_pct` (`Bounds::from_limit_pct`,
  symmetric: `[1/(1+p), 1+p]`), changeable during a run (only the limit; port,
  baud, density and position are refused mid-run).
- Alarms (`TrimAlarm`): `FeedStopped` (pump commanded 10 steps in 3 min, the
  weight never fell by 2 steps), `Saturated` (ratio out of bounds 5 min),
  `WrongSide`. Raising one puts c back to its value **from before the onset**
  (`c_hist`, last hour), freezes it, clears the ratio points.
  `FeedStopped`/`Saturated` clear by themselves once `recent_ratio` (minimum
  window only, never the settled one) is back in bounds; the deficit
  accumulated meanwhile is written off (`forgiven_g`), never paid back in a
  burst. A wider limit lifts a `Saturated` alarm the same way.
- The perturbation threshold follows the flow (`perturbation_limit_g`, 5x the
  expected step, 1..5 g): a fixed 5 g let a 3.4 g bump at 2 ml/min pass as
  flow.
- Refills (`trim::next_state`): a jump > 50 g, **or a rise > 50 g within
  30 s** (a transfer pump adds ~5 g a read, under the perturbation limit), or
  "Refill bottle". Over once the level (weight + k x commanded) holds still
  20 s, then 10 s. A slow rise rewinds the count to the lowest Normal read
  (`rewind_slow_rise`). The ratio history survives a refill, except under an
  alarm (dry bottle). No gain over the weight before = a lift, not a refill.
- Events journalled for the chart markers and notifications: `trim_start`,
  `trim_ratio`, `alarm_*`, `alarm_cleared`, `refill` (`refill_detail` /
  `parse_refill` must stay in step), `refill_cancelled`, `trim_limit`, `scale_lost`,
  `scale_recovered`.

## Notifications (`notify.rs`)

A `notifier` thread follows the `events` table through its **own** SQLite
connection (cursor in `app_state`, starts at "now" on first run, never
replays history) and sends the notable events of a run to its
`responsible` person only: ntfy (JSON publish to the server root, priority 5
for alarms, 2 for info) and/or a Teams Workflow webhook (Adaptive Card).
Never on the control thread: a dead service delays a message, never a pump
write. Link up/down messages are rate-limited to one per 15 min per run.
Alarms with an ongoing condition (`family_of`) become a `Page` that is resent
on ntfy every `REPEAT` (3 min) until acknowledged, its condition clears
(`clears`) or the run stops: shared with the API (`/api/notify/pages`,
`/api/notify/ack`, the UI banner). The ntfy "Acknowledge" button posts
`ack <run_id>` to `<topic>-ack`, polled every 15 s (ntfy.sh rate limit), so
the phone never needs to reach this PC and nobody configures anything.
Pages live in memory only; an ack is journalled as `alarm_ack`.
`POST /api/runs` refuses a dosing run without a known responsible once
`[notify] people` is non-empty. Calibration runs are never notified. Teams
private messages need a Power Automate flow (the ready-made "to a chat"
template only targets group chats), which is why ntfy is the default.

## Working on this PC (2026-10)

- Build from a Visual Studio **Community** environment:
  `cmd /c '"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" && cargo ...'`.
  The VS 2022 Build Tools on this PC are a broken install that `cargo` picks
  by default (SQLite's C build then fails).
- `rustc` occasionally dies with `STATUS_HEAP_CORRUPTION` here: retry once
  before suspecting the code (and suspect the PC's RAM/disk, see
  `docs/notes-de-session-2026-09-30.md`).
- The running daemon is `target\release\fermentool-core.exe`, started by hand
  with `FERMENTOOL_NO_BROWSER=1`, windowless. **Never run the debug build for
  the user**: its console, once clicked into selection mode, blocks every
  `WriteConsoleW` and froze the control thread.
- Deploy: re-check `/api/status`, stop an active run through the API only when
  the user asked for the deploy, `POST /api/shutdown`, `cargo build
  --release`, start the exe, check `/api/status` and that the served bundle
  contains the change; tell the user to Ctrl+F5 (an open tab keeps the old UI
  in memory).
- Rewriting a file through PowerShell `Get-Content`/`Set-Content` mangles its
  UTF-8 (`µ` became `Âµ` once): use Python with explicit `utf-8`, or the edit
  tools.
- The session log, findings on the hardware and the to-do list:
  `docs/notes-de-session-2026-09-30.md`.

## Curve engine (`crates/fermentool-curves`)

`CurveSpec::value_at(elapsed)` is the single entry point and is **pure in
elapsed time**, this is what makes crash-resume correct (feed it real
wall-clock elapsed after a restart, it returns where the pump should be). Keep
it pure: no clocks, no I/O, no hidden state.

Shapes: `Linear`, `Exponential`, `Sigmoid`, `Step`, `Constant`, `Custom`.
Two param modes: `Endpoints` (give start/end/duration, shape constant derived)
and `Physio` (give a physiological rate, `end` derived via `effective_end()`).

Verified correct (derivations + tests):
- Linear (both modes), Constant, Step, Custom (linear/hold): exact.
- Exponential: `s·(e/s)^(t/d)`. Both modes are constant-µ exponentials; they
  differ only in parametrisation (`Endpoints` → `µ_eff = ln(end/start)/dur_h`;
  `Physio` → you give µ, `end = s·e^(µ·dur_h)`).
- Sigmoid: **normalised logistic onto [start, end] in BOTH modes**: hits both
  endpoints exactly for any `steepness`/`k_per_hour`/`midpoint`, stays monotonic
  (incl. `k_per_hour < 0`). The `Physio` `k_per_hour` is therefore a *shape*
  parameter (rescaled by `1/(hi-lo)`), not a literal instantaneous rate.
- Only Linear and Exponential map to a kinetic model. Sigmoid/Step/Custom are
  shapes with no physiological derivation.
- Values are clamped to `[clamp_min, clamp_max]`; past `duration` the curve
  holds its end value. UI preview samples the curve as a 200-point polyline,
  display only; the engine calls `value_at` directly.

## Fed-batch F₀ calculator (`ui/src/routes/NewRun.svelte`)

Shown only for Exponential + `Physio` mode. Reference: `docs/alimentation-exponentielle-mu.md`.

```
F₀ [L/h]    = (µ / Y_x/s + m_s) · X₀ · V₀ / S_f
F₀ [ml/min] = F₀[L/h] · 1000 / 60
t_max [h]   = (1/µ) · ln(1 + µ·(V_max − V₀) / F₀[L/h])
```

- `m_s` (maintenance coefficient, g·g⁻¹·h⁻¹) is optional; blank → growth-only
  `µ/Y_x/s`. It was added because the growth-only form systematically
  underfeeds by ~5–10 % for E. coli at moderate µ. Keep it optional and
  defaulting to 0.
- Assumptions (stated in the UI): µ and Y_x/s constant, culture
  substrate-limiting (S ≈ 0). `t_max` ignores evaporation / sampling losses and
  pH-control base addition.
- **`F₀`, `t_max`, and the Exponential-`Physio` curve must stay mutually
  consistent**, all three assume `F(t) = F₀·e^(µt)`. `t_max` consumes
  `f0_Lh`, so it tracks the `m_s` change automatically. If you touch one,
  re-check the other two.
- Dimensional check: both terms in the parenthesis are g·g⁻¹·h⁻¹ → `L/h`.
  L→mL is exact (volume, no density assumption); the code is volume-based
  throughout.

## Agent workflow notes (learned the hard way)

- **Before restarting or killing the daemon, re-check `/api/status` for an
  active run right before the action**, not from an earlier check in the
  session. A daemon started idle can have a real run started on it later;
  force-killing it to deploy a change once disrupted a live run mid-session
  (survived only because crash-resume worked). Ask, or check again, right
  before you kill anything.
- **Splitting someone else's large uncommitted diff into logical commits**:
  isolate hunks (per-file staging when a concern maps 1:1 to files, hand-built
  `git apply --cached` patches when it doesn't), and build + test each
  resulting commit's tree before moving to the next. A commit that only
  compiles once a later commit's field/type exists is not a real commit.
- **A `cargo test` pass on the full working tree is not proof each commit
  builds alone.** If you must verify a specific commit in isolation and a
  `git worktree` build fails on an unrelated linker error, that is an
  environment issue, not license to skip the check silently: say so.

### Known gap (not fixed)

No sanity check that `F₀` is above the pump's real minimum deliverable flow
(`FLOW_LIMITS.min` is 0). Realistic strain params can yield `F₀` well below what
a peristaltic pump can hold steadily. Needs a real LabQ minimum-flow figure to
implement a warning.
