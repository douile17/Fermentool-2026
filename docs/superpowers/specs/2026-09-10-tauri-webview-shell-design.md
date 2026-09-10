# Tauri desktop shell + daemon-as-login-task

**Date:** 2026-09-10
**Status:** approved (Option A), ready for implementation plan

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

`docs/IMPLEMENTATION_PLAN.md` §9 reserves this: *"Optional native WebView shell
(Tauri/Wails) — no core changes."* `docs/service-install.md` sketches the "run
it as a service" side (systemd / launchd / Task Scheduler / NSSM).

## Goal

Ship Fermentool as a **desktop app** on Windows 11 (the primary target):

1. A native window (WebView2) rendering the Svelte UI **bundled into the app**.
   The daemon is used only as its HTTP + WebSocket backend.
2. **The daemon is a separate detached process.** Closing the app window does
   **not** stop it: a ~100 h run keeps going. The app re-attaches to the
   running daemon on next launch.
3. The daemon **auto-starts at login** (scheduled task) so it comes back on its
   own after a reboot and offers the resume prompt.
4. One installer `.exe`, no runtime install on the target.

### `§9` "no core changes" — relaxed, on purpose

Bundling the UI means it no longer runs on the daemon's origin, so its API
calls become cross-origin (CORS). Option A accepts two tiny changes for this:

- `ui/src/lib/api.js`: ~15 lines — target `http://127.0.0.1:8730` when running
  inside Tauri, relative paths otherwise (dev / browser). No behaviour or
  feature change.
- `fermentool-core`: a `tower-http` `CorsLayer` (~8 lines) allowing the Tauri
  webview origin. The API is already `127.0.0.1`-only, so this does not widen
  the threat model.

The rejected alternative (Option B) was a reverse proxy *inside the shell* that
forwards `/api/*` and the WebSocket to `:8730` under one origin — ~80 lines
incl. WS relay, a component to maintain forever, for the aesthetic gain of an
untouched core. Not worth it.

**Non-goals (v1):**

- No changes to `fermentool-curves` / `fermentool-modbus`. The `fermentool-core`
  and `ui/` changes are limited to the two above.
- Not a real Windows service (no NSSM, no session-0 service account). An "at log
  on" scheduled task is the outer safety net, matching `service-install.md`.
- Linux / macOS shells: the plan lists them; out of scope here. The daemon
  stays cross-platform; only `src-tauri/` is Windows-first.
- No auto-update, no code signing (tracked as follow-ups).

## Implementation order

**Step 0 is a spike.** Before writing the real crate, throwaway-prove the one
uncertain path (~30 min): a bare Tauri v2 project with `ui/dist` bundled,
`api.js` pointed at `http://127.0.0.1:8730`, the `CorsLayer` added to a locally
run daemon. Confirm all three work from the webview:

- a plain `GET /api/status`,
- a `PUT /api/config` (triggers a CORS preflight),
- the `/api/ws` WebSocket (no CORS preflight, but the server may check the
  `Origin` header on upgrade — verify axum's handler does not reject it).

If the WebSocket is rejected on `Origin`, that is a third small `fermentool-core`
change (accept the Tauri origin on the upgrade) — decide then, cheaply.

## Design

### 1. New crate `src-tauri/` (Tauri v2)

Add to the workspace: root `Cargo.toml` `members += "src-tauri"`. Crate
`fermentool-tauri`, binary `fermentool` (installed app exe = `fermentool.exe`,
distinct from `fermentool-core.exe`).

Tauri v2 — v1 is legacy; v2 has the stable `single-instance`, `shell` and
`process` plugins and the sidecar mechanism.

`src-tauri/tauri.conf.json`:

- One window: 1200×800, min 900×600, `title: "Fermentool"`, `resizable`,
  `center`, `"visible": false` at boot (shown once the shell has decided the
  daemon is coming up — see §3).
- `build.frontendDist = "../ui/dist"` — the Svelte SPA is **bundled**. No
  splash page: the UI already renders a "reconnecting…" state (`app.connected`
  in `ui/src/lib/state.svelte.js`) that covers "daemon not up yet".
- `bundle.externalBin = ["binaries/fermentool-core"]` — the daemon ships as a
  **sidecar** (Tauri appends the target triple; real file
  `src-tauri/binaries/fermentool-core-x86_64-pc-windows-msvc.exe`, see §6).
- `bundle.targets = ["nsis"]`, `bundle.icon` = existing logo
  (`ui/public` / `logos/export`), `identifier = "com.fermentool.app"`.
- CSP: `connect-src` allows `http://127.0.0.1:8730` and `ws://127.0.0.1:8730`.

### 2. Frontend ↔ daemon wiring (Option A)

**`ui/src/lib/api.js`** gains an API base:

```js
// Bundled in a Tauri window → the daemon is a separate origin. In the browser
// (dev proxy, or the daemon serving ui/dist itself) → same origin, stay relative.
const BASE = '__TAURI_INTERNALS__' in window ? 'http://127.0.0.1:8730' : '';
```

- `req()` calls `fetch(BASE + path, …)`.
- `connectWs()` builds the URL from `BASE` (→ `ws://127.0.0.1:8730/api/ws`)
  instead of `location.host`, and its `onopen` snapshot fetch uses `BASE` too.
- Nothing else in `ui/` changes. Dev (`npm run dev`, Vite proxy) and browser
  mode (daemon serving `ui/dist`) are unaffected — `BASE` is `''` there.

**`fermentool-core`** adds a CORS layer on the router
([crates/fermentool-core/src/api.rs](../../../crates/fermentool-core/src/api.rs)):

```rust
use tower_http::cors::{Any, CorsLayer};
// The webview origin on Windows is http://tauri.localhost. Allow the tauri
// origins; the listener is already 127.0.0.1-only.
let cors = CorsLayer::new()
    .allow_origin([
        "http://tauri.localhost".parse().unwrap(),
        "https://tauri.localhost".parse().unwrap(),
        "tauri://localhost".parse().unwrap(),
    ])
    .allow_methods(Any)
    .allow_headers(Any);
router.layer(cors)
```

- `Cargo.toml`: `tower-http` gains the `cors` feature (already a dep for the
  static-file serving).
- Browser / curl / same-origin callers are unaffected (CORS headers only matter
  to a browser enforcing them).
- If step 0 shows the `/api/ws` upgrade rejects the Tauri `Origin`, add the same
  allow-list check there.

### 3. Shell responsibilities (`src-tauri/src/main.rs`, ~150 lines)

Plugins: `tauri-plugin-single-instance`, `tauri-plugin-shell`,
`tauri-plugin-process`.

On startup, in order:

1. **Single instance.** `single-instance` plugin: a second launch focuses the
   existing window and exits.
2. **Is the daemon already up?** `GET http://127.0.0.1:8730/api/status`,
   ~500 ms timeout, blocking `ureq` (no `fermentool-core` dependency).
   - **Up** → the scheduled task or a manual launch already started it. Go to 4.
   - **Down** → 3.
3. **Spawn the daemon, detached** so it outlives the shell:
   - Resolve the sidecar path from `app.path().resource_dir()`.
   - Spawn with a **raw `std::process::Command`** + `.creation_flags(DETACHED_PROCESS
     | CREATE_NEW_PROCESS_GROUP)` (`0x00000008 | 0x00000200`,
     `std::os::windows::process::CommandExt`). Do not use the plugin's
     `CommandChild` (it can tie the child's lifetime to the app). Drop the
     `Child` handle immediately.
   - **No Job object.** Verify on the target that killing the shell leaves the
     child running (§ Testing #2 — hard gate).
   - Poll `/api/status` every 200 ms up to ~15 s until it binds.
4. **Show the window.** The bundled UI is already loaded; just
   `window.show() + set_focus()`. It renders in its own "reconnecting…" state
   and goes live as soon as `/api/status` + the WS connect. On the 15 s spawn
   timeout, still show the window (the UI's reconnect banner is the error
   surface) and log; the tray tooltip reads "daemon not responding".

### 4. Window close → tray

Tray icon (`tauri::tray::TrayIconBuilder`) always present. Window
`CloseRequested` → `api.prevent_close()` + `window.hide()`.

Tray menu:

- **Open Fermentool** → `window.show()` + `set_focus()`.
- **Shut down daemon & quit** → `POST http://127.0.0.1:8730/api/shutdown`
  (best-effort, 2 s), then `app.exit(0)`.
- (separator) **Quit (leave daemon running)** → `app.exit(0)` only.

Left-click tray → Open. Tooltip reflects a 5 s poll of `:8730`:
"Fermentool — running" / "Fermentool — daemon not responding".

The daemon's `POST /api/shutdown` and the Settings "Shut down daemon" button
are unchanged; the tray item is a second door to the same endpoint.

### 5. Installer (NSIS, via `tauri build`)

`tauri build` with `bundle.targets = ["nsis"]` → `fermentool_<version>_x64-setup.exe`.

NSIS install hooks (`src-tauri/installer/hooks.nsh`, referenced from
`tauri.conf.json` `bundle.windows.nsis.installerHooks`):

- **postInstall:** register the login task. `schtasks /change` cannot set
  failure-restart, so ship a task XML
  (`src-tauri/installer/fermentool-task.xml`: `LogonTrigger`,
  `RunLevel=HighestAvailable`, `RestartOnFailure` interval `PT1M` count `3`,
  `<Exec><Command>` a `{{EXE}}` placeholder). In the hook:
  `powershell -Command "(Get-Content task.xml) -replace '{{EXE}}', '$INSTDIR\fermentool-core.exe' | Set-Content task.xml"`
  then `schtasks /create /tn "Fermentool" /xml "task.xml" /f`. The task runs the
  **installed** `$INSTDIR\fermentool-core.exe`, removed by uninstall.
- **preUninstall:** `schtasks /delete /tn "Fermentool" /f`, then best-effort
  `powershell -Command "try { Invoke-WebRequest -UseBasicParsing -Method POST http://127.0.0.1:8730/api/shutdown -TimeoutSec 2 } catch {}"`
  so no orphan daemon keeps `:8730`.

After a reboot: at the operator's login the daemon is back within seconds,
reads the journal, and (if a run was active) the UI shows the resume prompt
when the app is opened. The app, when launched, finds `:8730` already up (step
2) and attaches.

First launch right after install: the task has not fired (no new login), so the
app spawns the daemon itself (step 3). No double-start — step 2's health check
gates it, and `fermentool-core`'s own `AddrInUse` guard is the backstop.

### 6. Getting the daemon binary into the bundle

The sidecar file must exist before `tauri build`. A `just` recipe / npm script
in `src-tauri`, run before bundling:

```
cargo build --release -p fermentool-core --target x86_64-pc-windows-msvc
copy target/x86_64-pc-windows-msvc/release/fermentool-core.exe \
     src-tauri/binaries/fermentool-core-x86_64-pc-windows-msvc.exe
```

`fermentool-core` still fails its own build if `ui/dist` is missing, so the
sidecar carries `ui/dist` embedded exactly as today — meaning **browser mode
still works** (run `fermentool-core.exe` alone, it serves the UI at `:8730`).
The app bundles the *same* `ui/dist` from the *same* build, so there is no
UI/daemon version skew within one installer. `src-tauri/binaries/` is
`.gitignore`d.

Release sequence in `README.md`:
`npm --prefix ui ci && npm --prefix ui run build` → the copy above →
`cargo tauri build`.

### 7. Repo changes

| Added / changed | Untouched |
|---|---|
| `+ src-tauri/` (new crate: `Cargo.toml`, `tauri.conf.json`, `src/main.rs`, `installer/hooks.nsh`, `installer/fermentool-task.xml`, `binaries/.gitignore`) | `crates/fermentool-curves`, `crates/fermentool-modbus` — **0 diff** |
| root `Cargo.toml`: `members += "src-tauri"` | `ui/` — only `src/lib/api.js` |
| `ui/src/lib/api.js`: API base (~15 lines) | `fermentool-core` — only `api.rs` + `Cargo.toml` |
| `crates/fermentool-core/src/api.rs`: `CorsLayer` on the router; `Cargo.toml`: `tower-http` `cors` feature | the `:8730` HTTP API surface, browser fallback, `fermentool-core.exe` standalone use |
| `docs/service-install.md`: "Windows — Fermentool app + login task" section | |
| `README.md`: installer build steps | |
| `.gitignore`: `src-tauri/binaries/`, `src-tauri/target/` | |

Workspace note: `src-tauri` pulls a large tree (tauri, wry, webview2-com). Keep
it a **member** so CI compile-checks it; jobs that only need the daemon use
`-p fermentool-core`.

## Testing

- **Step 0 spike** — see *Implementation order*. Gates the whole design.
- **CI:** `cargo build -p fermentool-tauri` (compile check; no bundling in CI).
- **Manual, dev machine (`cargo tauri dev`, no installer):**
  1. `:8730` free → shell spawns the sidecar → window shows in "reconnecting…"
     state → goes live within ~2 s.
  2. Close the window → tray; `curl http://127.0.0.1:8730/api/status` still 200.
     **This is the core guarantee.**
  3. Re-open from tray → same daemon, no second spawn (one `fermentool-core.exe`
     in `tasklist`).
  4. Launch a 2nd copy of the shell → first window focuses, no 2nd process.
  5. Start `fermentool-core.exe` by hand first, then the shell → it attaches,
     spawns nothing.
  6. Exercise the UI end to end from the window: status live via WS, `PUT`
     config (Settings save), start/stop a sim run, History chart + CSV.
  7. Tray "Shut down daemon & quit" → `:8730` stops answering, both gone.
- **Installer, clean Windows session (VM):**
  1. NSIS setup → app installed, `schtasks /query /tn Fermentool` shows the task.
  2. Launch app → daemon spawns (task hasn't fired), UI live.
  3. Reboot, log in, **don't** open the app → `curl :8730/api/status` answers.
  4. Open the app → attaches; a run mid-flight before the reboot shows the
     resume prompt.
  5. Uninstall → task gone, `:8730` silent, no orphan process.

## Risks / notes

- **Detached-child behaviour is load-bearing.** If the spawned daemon dies with
  the shell, the "close window, run survives" guarantee breaks. Mitigation: raw
  `std::process::Command` + `DETACHED_PROCESS`; Testing #2 is a hard gate.
- **WebSocket `Origin`.** `/api/ws` is a same-origin assumption today. Step 0
  verifies the webview can connect; if not, a small allow-`Origin` check on the
  upgrade handler is the fix.
- **CORS scope.** Allow-list the three `tauri` origins, not `Any` — tidy, and
  the listener is `127.0.0.1`-only anyway.
- **WebView2 runtime.** Default on Windows 10 21H2+ / Windows 11. NSIS bundler:
  `bundle.windows.webviewInstallMode = downloadBootstrapper` (small installer,
  fetches at install time; install is interactive).
- **Scheduled task is per-user** (`/sc onlogon`). Fine for one operator. Headless
  / multi-user needs the NSSM path — deferred.
- **Port hard-coded to 8730** on the shell side (health check) and in `api.js`
  (`BASE`). Changing `[port]` in `config.toml` breaks the app. v1: document it;
  a follow-up can have the shell read `config.toml` and inject the base.
- **Toolchain not installed here:** `tauri-cli`, NSIS bundler,
  `x86_64-pc-windows-msvc` target. Plan step 0 installs and verifies them.
- **Code signing / SmartScreen:** an unsigned installer warns. Out of scope;
  note in the README.
