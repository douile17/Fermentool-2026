# Tauri WebView shell + daemon-as-service

**Date:** 2026-09-10
**Status:** approved, ready for implementation plan

## Problem

`fermentool-core.exe` is already a windowless background daemon (axum on
`127.0.0.1:8730`, `ui/dist` baked in via `rust-embed`, `windows_subsystem =
"windows"`). In release it opens the operator's **default browser** at the
daemon URL ([crates/fermentool-core/src/main.rs](../../../crates/fermentool-core/src/main.rs),
commit `603d362`).

That works but reads as "a local web server", not an application:

- No window of its own — the UI lives in whatever browser tab, easy to lose,
  mixed in with the operator's other tabs.
- No taskbar / tray presence; nothing tells you the daemon is up except an
  open tab.
- After a machine reboot (power cut, Windows update) **nothing runs** until
  someone re-launches the exe, so a run interrupted by a power cut does not
  resume until a human notices.

`docs/IMPLEMENTATION_PLAN.md` §9 already reserves this: *"Optional native
WebView shell (Tauri/Wails) — no core changes."* `docs/service-install.md`
already sketches the "run it as a service" side (systemd / launchd / Task
Scheduler / NSSM).

## Goal

Ship Fermentool as a **desktop app** on Windows 11 (the primary target) without
touching the daemon:

1. A native window (WebView2) showing the existing UI at `:8730` — its own
   window, taskbar entry, app icon.
2. **The daemon is a separate detached process.** Closing the app window does
   **not** stop it: a ~100 h run keeps going. The app re-attaches to the
   running daemon on next launch.
3. The daemon **auto-starts at login** (scheduled task) so it comes back on its
   own after a reboot and offers the resume prompt.
4. One installer `.exe`, no runtime install on the target.

**Non-goals (v1):**

- No changes to `fermentool-core`, `fermentool-curves`, `fermentool-modbus`,
  `ui/`. If the shell needs something from the daemon, that is a separate spec.
- Not a real Windows service (no NSSM, no session-0 service account). A "At log
  on" scheduled task is the outer safety net, matching `service-install.md`.
- Linux / macOS shells: the plan lists them; out of scope here. The daemon
  stays cross-platform; only `src-tauri/` is Windows-first.
- No auto-update, no code signing (tracked as follow-ups).

## Design

### 1. New crate `src-tauri/` (Tauri v2)

Add to the workspace: `Cargo.toml` `members += "src-tauri"`. Crate name
`fermentool-tauri`, binary `fermentool` (so the installed app exe is
`fermentool.exe`, distinct from `fermentool-core.exe`).

Tauri v2 (v1 is legacy; v2 has the stable `single-instance`, `shell` and
`process` plugins and the sidecar mechanism this design needs).

`src-tauri/tauri.conf.json`:

- One window: 1200×800, min 900×600, `title: "Fermentool"`, `resizable`,
  `center`. Content is **not** a bundled frontend — it is a remote URL,
  `http://127.0.0.1:8730` (set at runtime after the daemon is confirmed up; see
  §3), so `build.frontendDist` points at a tiny local `splash/` page shown
  while the daemon comes up.
- `bundle.externalBin = ["binaries/fermentool-core"]` — the daemon ships as a
  **sidecar** (Tauri appends the target triple; the real file is
  `src-tauri/binaries/fermentool-core-x86_64-pc-windows-msvc.exe`, produced by
  a build step, see §5).
- `bundle.targets = ["nsis"]`, `bundle.icon` = the existing logo
  (`ui/public` / `logos/export`), `identifier = "com.fermentool.app"`.
- CSP: allow the webview to talk to `http://127.0.0.1:8730` (connect-src,
  ws:).

### 2. Shell responsibilities (`src-tauri/src/main.rs`, ~150 lines)

Plugins: `tauri-plugin-single-instance`, `tauri-plugin-shell`,
`tauri-plugin-process`.

On startup, in order:

1. **Single instance.** `single-instance` plugin: a second launch focuses the
   existing window and exits.
2. **Is the daemon already up?** `GET http://127.0.0.1:8730/api/status` with a
   ~500 ms timeout. `reqwest` (blocking) or `ureq` — no daemon dependency.
   - **Up** → skip to step 4 (re-attach; the scheduled task or a manual launch
     already started it).
   - **Down** → step 3.
3. **Spawn the daemon, detached.** `tauri-plugin-shell` sidecar
   (`app.shell().sidecar("fermentool-core")`), but spawned so it **outlives the
   shell**:
   - Do **not** keep the `CommandChild`; drop it after spawn.
   - Windows creation flags: `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`
     (`0x00000008 | 0x00000200`) via `std::os::windows::process::CommandExt`
     if the plugin's sidecar API doesn't expose flags — fall back to a raw
     `std::process::Command` on the sidecar path resolved from
     `app.path().resource_dir()`.
   - **No Job object.** Tauri does not attach spawned sidecars to a job by
     default; verify on the target that killing the shell leaves the child
     running (test in §6).
   - Then poll `/api/status` every 200 ms up to ~15 s until it binds.
4. **Load the UI.** Navigate the window to `http://127.0.0.1:8730`
   (`window.eval("location.replace(...)")` or set the URL and show the window).
   Show a "Starting Fermentool…" splash until then; on the 15 s timeout show a
   retry / "open log folder" panel instead.

### 3. Window close → tray

`tauri.conf.json` window `"visible": false` at boot (shown after step 4). Tray
icon (`tauri::tray::TrayIconBuilder`) always present. Window close event
(`WindowEvent::CloseRequested`) → `api.prevent_close()` + `window.hide()`.

Tray menu:

- **Open Fermentool** → `window.show()` + `set_focus()`.
- **Shut down daemon & quit** → `POST http://127.0.0.1:8730/api/shutdown`
  (best-effort, 2 s timeout), then `app.exit(0)`.
- (separator) **Quit (leave daemon running)** → `app.exit(0)` only.

Left-click tray → Open. Tooltip reflects reachability of `:8730` (a 5 s poll):
"Fermentool — running" / "Fermentool — daemon not responding".

The daemon's own `POST /api/shutdown` and the Settings "Shut down daemon"
button keep working unchanged; the tray "Shut down" is just a second door to
the same endpoint.

### 4. Installer (NSIS, via `tauri build`)

`tauri build` with `bundle.targets = ["nsis"]` produces
`fermentool_<version>_x64-setup.exe`.

NSIS install hooks (`src-tauri/installer/hooks.nsh`, referenced from
`tauri.conf.json` `bundle.windows.nsis.installerHooks`):

- **postInstall:** register the login task. `schtasks /change` cannot set
  failure-restart, so ship a task XML (`src-tauri/installer/fermentool-task.xml`:
  `LogonTrigger`, `RunLevel=HighestAvailable`, `RestartOnFailure` interval
  `PT1M` count `3`, `<Exec><Command>` left as a placeholder) and in the hook do
  `powershell -Command "(Get-Content task.xml) -replace '{{EXE}}', '$INSTDIR\fermentool-core.exe' | Set-Content task.xml"`
  then `schtasks /create /tn "Fermentool" /xml "task.xml" /f`. Task runs the
  **installed** `$INSTDIR\fermentool-core.exe` so an uninstall removes the
  target with it.
- **preUninstall:** `schtasks /delete /tn "Fermentool" /f`, then a best-effort
  daemon stop — `powershell -Command "try { Invoke-WebRequest -UseBasicParsing -Method POST http://127.0.0.1:8730/api/shutdown -TimeoutSec 2 } catch {}"` —
  so we don't leave an orphan daemon holding `:8730`.

The scheduled task means: after a reboot, at the operator's login the daemon is
back within seconds, reads the journal, and (if a run was active) the UI shows
the resume prompt when the app is opened. The app, when launched, finds `:8730`
already up (step 2) and just attaches.

First launch right after install: the task has not fired yet (no new login), so
the app spawns the daemon itself (step 3). No double-start — step 2's health
check gates it, and `fermentool-core`'s own `AddrInUse` guard is the backstop.

### 5. Getting the daemon binary into the bundle

The sidecar file must exist before `tauri build`. Add a `just` recipe / npm
script / `build.rs` in `src-tauri` that, before bundling:

```
cargo build --release -p fermentool-core --target x86_64-pc-windows-msvc
copy target/x86_64-pc-windows-msvc/release/fermentool-core.exe \
     src-tauri/binaries/fermentool-core-x86_64-pc-windows-msvc.exe
```

`fermentool-core` already fails its own build if `ui/dist` is missing, so the
UI is baked into that sidecar exactly as today. `src-tauri/binaries/` is
`.gitignore`d (build artifact).

Document the full release sequence in `README.md`:
`npm --prefix ui ci && npm --prefix ui run build` → the copy step above →
`cargo tauri build`.

### 6. Repo changes

| Added / changed | Untouched |
|---|---|
| `+ src-tauri/` (new crate: `Cargo.toml`, `tauri.conf.json`, `src/main.rs`, `splash/`, `installer/hooks.nsh`, `binaries/.gitignore`) | `crates/fermentool-core`, `crates/fermentool-curves`, `crates/fermentool-modbus` — **0 diff** |
| root `Cargo.toml`: `members += "src-tauri"`; workspace deps for tauri | `ui/` — **0 diff** |
| `docs/service-install.md`: new "Windows — Fermentool app + login task" section | `fermentool-core.exe` still runs standalone / from Task Scheduler / from a browser |
| `README.md`: installer build steps | the `:8730` HTTP API, browser fallback |
| `.gitignore`: `src-tauri/binaries/`, `src-tauri/target/` | |

Workspace note: `src-tauri` pulls a large dependency tree (tauri, wry,
webview2-com). Keep it a workspace member so `cargo build` at the root builds
it too — or exclude it from the default set and build explicitly. Decision:
**member, not excluded**, so CI compile-checks it; the soak/test jobs that only
need the daemon can `-p fermentool-core`.

## Testing

- **CI:** `cargo build -p fermentool-tauri` (compile check; no bundling in CI
  for v1).
- **Manual, dev machine (no installer):**
  1. `cargo tauri dev` with `:8730` free → shell spawns the sidecar → splash →
     UI loads.
  2. Close window → tray; `curl http://127.0.0.1:8730/api/status` still 200
     (daemon alive). **This is the core guarantee.**
  3. Re-open from tray → same daemon, no second spawn (check only one
     `fermentool-core.exe` in `tasklist`).
  4. Launch a 2nd copy of the shell → first window focuses, no 2nd process.
  5. Start `fermentool-core.exe` by hand first, then launch the shell → it
     attaches, spawns nothing.
  6. Tray "Shut down daemon & quit" → `:8730` stops answering, both processes
     gone.
- **Installer, clean Windows session (VM):**
  1. Run the NSIS setup → app installed, `schtasks /query /tn Fermentool` shows
     the task.
  2. Launch app → daemon spawns (task hasn't fired), UI loads.
  3. Reboot, log in, **don't** open the app → `curl :8730/api/status` answers
     (task started it).
  4. Open the app → attaches; if a run was mid-flight before the reboot, the
     resume prompt shows.
  5. Uninstall → task gone, `:8730` no longer answers, no orphan process.

## Risks / notes

- **Detached-child behaviour is the load-bearing assumption.** If Tauri's
  sidecar spawn attaches the child to a job object that kills it on shell exit,
  the "close window, run survives" guarantee breaks. Mitigation: spawn via raw
  `std::process::Command` with `DETACHED_PROCESS` on the resolved sidecar path,
  and test #2 above is a hard gate before this ships.
- **WebView2 runtime.** Present by default on Windows 10 21H2+ and Windows 11.
  Tauri's NSIS bundler can embed the Evergreen bootstrapper
  (`bundle.windows.webviewInstallMode`); use `downloadBootstrapper` (small
  installer, fetches at install time) — acceptable since install is
  interactive.
- **Scheduled task vs. per-user.** `/sc onlogon` is per the installing user. A
  machine used by one operator (the expected case) is fine. Multi-user or
  "runs headless with nobody logged in" would need the NSSM path — explicitly
  deferred.
- **Two shutdown doors.** Tray "Shut down" and Settings button both hit
  `/api/shutdown`; harmless, both just Notify the daemon.
- **Port still hard-coded to 8730** on the shell side (health check + navigate).
  If a user changes `[port]` in `config.toml`, the shell won't find the daemon.
  v1: document "don't change the port when using the app". A follow-up can have
  the shell read `config.toml` first.
- **Toolchain not yet installed here:** `cargo-tauri` / `tauri-cli`, the NSIS
  bundler, `x86_64-pc-windows-msvc` target. Implementation plan step 0 is
  installing and verifying these.
- **Code signing / SmartScreen:** an unsigned NSIS installer triggers a
  SmartScreen warning. Out of scope; note it in the README.
