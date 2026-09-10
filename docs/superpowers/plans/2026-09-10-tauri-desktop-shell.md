# Tauri Desktop Shell Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Fermentool as an installable Windows desktop app — a Tauri v2 window rendering the bundled Svelte UI, backed by the existing daemon running as a detached process that auto-starts at login.

**Architecture:** A new `src-tauri/` crate builds `fermentool.exe`, a WebView2 window whose frontend is the bundled `ui/dist`. The unchanged `fermentool-core.exe` ships as a Tauri sidecar; the shell health-checks `127.0.0.1:8730`, and if nothing answers, spawns the daemon **detached** so it outlives the window. The window closes to a tray icon. An NSIS installer registers an "at log on" scheduled task so the daemon returns after a reboot. The only changes outside `src-tauri/` are an API-base constant in `ui/src/lib/api.js` and a `CorsLayer` in `fermentool-core` (the bundled UI is now a separate origin from the API).

**Tech Stack:** Rust, Tauri v2 (`tauri`, `tauri-plugin-single-instance`, `tauri-plugin-shell`, `tauri-plugin-process`), WebView2, NSIS bundler, `tower-http` (cors), axum 0.8, Svelte 5 / Vite (existing UI).

**Spec:** `docs/superpowers/specs/2026-09-10-tauri-webview-shell-design.md`

## Global Constraints

Copied verbatim from the spec. Every task's requirements implicitly include these.

- **Tauri v2**, not v1.
- **Windows 11 is the target.** `src-tauri/` is Windows-first; `fermentool-core`, `fermentool-curves`, `fermentool-modbus` stay cross-platform.
- **The daemon is spawned detached.** Raw `std::process::Command` with `.creation_flags(0x00000008 | 0x00000200)` (`DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`). Do not keep the `Child`. No Job object. **Closing the app window must never stop the daemon** — this is the load-bearing guarantee.
- **Port is hard-coded to `8730`** — the shell health-check and the `ui/src/lib/api.js` base both assume it.
- **Core changes are limited to exactly two things:** a `CorsLayer` in `crates/fermentool-core/src/api.rs` (+ its `Cargo.toml` dep) and an API-base constant in `ui/src/lib/api.js`. Zero changes to `fermentool-curves` / `fermentool-modbus`. Nothing else in `fermentool-core` / `ui/` (unless Task 0's spike proves the `/api/ws` upgrade needs an `Origin` allowance — then that one addition too).
- **CORS allow-list is explicit**, not `Any`: `http://tauri.localhost`, `https://tauri.localhost`, `tauri://localhost`.
- **Installer:** NSIS. Per-user scheduled task named `Fermentool`, trigger `onlogon`, `RunLevel=HighestAvailable`, `RestartOnFailure` interval `PT1M` count `3`. Uninstall deletes the task and does a best-effort `POST /api/shutdown`.
- **WebView2 runtime:** `bundle.windows.webviewInstallMode = downloadBootstrapper`.
- **v1 excludes:** auto-update, code signing.
- **Commits:** no `Co-Authored-By: Claude` trailer, no `Claude-Session` line (project rule in `CLAUDE.md`).
- **Toolchain is not yet installed on the dev machine.** Task 0 installs and verifies `tauri-cli`, the NSIS bundler, and the `x86_64-pc-windows-msvc` target.

---

## File Structure

**New — `src-tauri/` crate:**
- `src-tauri/Cargo.toml` — crate `fermentool-tauri`, binary `fermentool`; tauri + plugin deps.
- `src-tauri/tauri.conf.json` — window, `frontendDist = "../ui/dist"`, sidecar, NSIS bundle config, CSP.
- `src-tauri/build.rs` — `tauri_build::build()` (standard).
- `src-tauri/src/main.rs` — shell entry: single-instance, daemon health-check + detached spawn, window show, tray, close-to-tray.
- `src-tauri/src/daemon.rs` — `is_up()`, `spawn_detached()`, `wait_until_up()`, `shutdown()`.
- `src-tauri/src/tray.rs` — tray icon + menu + handlers.
- `src-tauri/installer/hooks.nsh` — NSIS `postInstall` / `preUninstall` hooks.
- `src-tauri/installer/fermentool-task.xml` — scheduled-task definition with a `{{EXE}}` placeholder.
- `src-tauri/binaries/.gitignore` — ignores the copied sidecar exe.
- `src-tauri/icons/` — app icons generated from the existing logo.

**Modified — existing:**
- `Cargo.toml` (root) — `members += "src-tauri"`.
- `crates/fermentool-core/Cargo.toml` — add `tower-http = { version = "0.6", features = ["cors"] }`.
- `crates/fermentool-core/src/api.rs` — build and `.layer()` a `CorsLayer` in `router()`.
- `ui/src/lib/api.js` — `BASE` constant; prefix `fetch` paths and the WS URL.
- `.gitignore` (root) — `src-tauri/target/`, `src-tauri/binaries/*.exe`.
- `docs/service-install.md` — "Windows — Fermentool app + login task" section.
- `README.md` — installer build sequence.
- `docs/IMPLEMENTATION_PLAN.md` — mark §9 shell "in progress", note the CORS relaxation.

---

## Task 0: Spike — prove the webview ↔ daemon path

**Goal:** Throwaway-verify the one uncertain thing before building the real crate: a bundled UI on the `tauri.localhost` origin can reach the daemon's API and WebSocket with just a `CorsLayer`. Produces a go/no-go note and the confirmed facts later tasks depend on.

**Files:**
- Create: `/tmp/fermentool-spike/` (throwaway, outside the repo — **not committed**)
- Reference only: `crates/fermentool-core/src/api.rs` (`router()`, `/api/ws`), `ui/src/lib/api.js`

**Interfaces:**
- Consumes: nothing.
- Produces (written into `docs/superpowers/plans/2026-09-10-tauri-desktop-shell.md` as a short "Spike result" note appended at the bottom, and repeated verbatim in the Task 1 / Task 4 notes if it changes anything):
  - `SPIKE_WS_NEEDS_ORIGIN_FIX: yes|no` — whether `/api/ws` rejects the `tauri.localhost` Origin.
  - `SPIKE_WEBVIEW_ORIGIN: "<string>"` — the exact `Origin` header value the WebView2 webview sends (expected `http://tauri.localhost`; confirm).

- [ ] **Step 1: Install the toolchain**

Run (PowerShell):
```powershell
rustup target add x86_64-pc-windows-msvc
cargo install tauri-cli --version "^2" --locked
cargo tauri --version   # expect: tauri-cli 2.x
```
NSIS is downloaded by the bundler on first `tauri build`; nothing to install now. If `cargo install tauri-cli` fails on a missing linker, install the **Visual Studio Build Tools** ("Desktop development with C++") and retry.

Expected: `cargo tauri --version` prints a 2.x version.

- [ ] **Step 2: Scaffold a throwaway Tauri app with the real UI bundled**

Run:
```powershell
cd /tmp
npm create tauri-app@latest fermentool-spike -- --template vanilla --manager npm --yes
cd fermentool-spike
```
Then edit `src-tauri/tauri.conf.json`: set `"build": { "frontendDist": "../dist" }` and copy the built UI in:
```powershell
npm --prefix C:\Users\Administrateur\Documents\SOFT_Homemade\Fermentool\ui run build
Remove-Item -Recurse -Force .\dist -ErrorAction SilentlyContinue
Copy-Item -Recurse C:\Users\Administrateur\Documents\SOFT_Homemade\Fermentool\ui\dist .\dist
```
In `src/main.js` of the spike (or a `<script>` in `dist/index.html` you add by hand), the point is only to exercise the API — you can instead just open the app and use the real UI, since it will try to talk to `/api/...`.

- [ ] **Step 3: Add the CorsLayer to a locally-run daemon and start it**

Temporarily, in a scratch branch of the real repo (do **not** commit yet — Task 1 does it properly), add to `crates/fermentool-core/src/api.rs` `router()` just before the final return:
```rust
use tower_http::cors::CorsLayer;
let cors = CorsLayer::new()
    .allow_origin([
        "http://tauri.localhost".parse().unwrap(),
        "https://tauri.localhost".parse().unwrap(),
        "tauri://localhost".parse().unwrap(),
    ])
    .allow_methods(tower_http::cors::Any)
    .allow_headers(tower_http::cors::Any);
```
and `.layer(cors)` on the router, plus `tower-http = { version = "0.6", features = ["cors"] }` in `crates/fermentool-core/Cargo.toml`. Run:
```powershell
cargo run -p fermentool-core
```
(simulator is fine — no pump needed).

- [ ] **Step 4: Point the spike UI at the daemon and run it**

In the spike's copied `dist/`, the UI's `api.js` still uses relative paths. For the spike only, edit `dist/assets/index-*.js` is impractical — instead, run the daemon with the CorsLayer AND set the spike's frontend to load the UI from a `<base href>` is also messy. Simplest: in the spike `src-tauri/tauri.conf.json`, set the window to load a remote URL `"http://127.0.0.1:8730"` OR add a 3-line `dist/probe.html` that does the three checks explicitly:
```html
<script type="module">
const B = 'http://127.0.0.1:8730';
console.log('GET', (await fetch(B + '/api/status')).status);
console.log('PUT', (await fetch(B + '/api/config', {method:'PUT',headers:{'content-type':'application/json'},body: JSON.stringify(await (await fetch(B+'/api/config')).json())})).status);
const ws = new WebSocket('ws://127.0.0.1:8730/api/ws');
ws.onopen = () => console.log('WS open');
ws.onerror = (e) => console.log('WS error', e);
</script>
```
Set `frontendDist` to the folder holding `probe.html` and the window URL to `probe.html`. Run `cargo tauri dev`. Open the webview devtools (right-click → Inspect, or `"devtools": true` in the config).

Expected in the console:
- `GET 200`
- `PUT 200` (this is the CORS-preflighted request — a `403`/network error here means the allow-list is wrong)
- `WS open` (a `WS error` means `/api/ws` rejects the Origin)

Also, in the **daemon's** log or with Wireshark/devtools Network tab, note the exact `Origin:` request header value.

- [ ] **Step 5: Record the result and clean up**

Append to the bottom of this plan file:
```markdown
## Spike result (Task 0)

- Toolchain: tauri-cli <version>, msvc target OK.
- GET /api/status from the webview: PASS
- PUT /api/config (CORS preflight) from the webview: PASS / FAIL — <notes>
- /api/ws WebSocket from the webview: PASS / FAIL
- SPIKE_WEBVIEW_ORIGIN: "<exact string>"
- SPIKE_WS_NEEDS_ORIGIN_FIX: yes / no
```
Then `git checkout -- crates/fermentool-core` in the real repo (drop the scratch CorsLayer — Task 1 adds it for real) and delete `/tmp/fermentool-spike`.

- [ ] **Step 6: Commit**

```bash
git add docs/superpowers/plans/2026-09-10-tauri-desktop-shell.md
git commit -m "docs: record Tauri spike result"
```

**Gate:** if GET+PUT+WS all PASS, proceed. If PUT fails, the CORS config is wrong — fix the allow-list understanding before Task 1. If WS fails, Task 1 also adds an `Origin` check to the `/api/ws` handler (an allowed third core change, per Global Constraints).

---

## Task 1: CorsLayer on the daemon router

**Goal:** The daemon accepts cross-origin API calls from the Tauri webview. TDD via axum's `oneshot` test harness.

**Files:**
- Modify: `crates/fermentool-core/Cargo.toml` (add `tower-http`)
- Modify: `crates/fermentool-core/src/api.rs` — `router()` (around line 46-66), and its `#[cfg(test)] mod tests`
- Test: `crates/fermentool-core/src/api.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: Task 0's `SPIKE_WS_NEEDS_ORIGIN_FIX`.
- Produces: `router()` unchanged signature (`pub fn router(state: AppState) -> Router`) but now carrying a `CorsLayer`. No new public items.

- [ ] **Step 1: Add the dependency**

In `crates/fermentool-core/Cargo.toml`, under `[dependencies]`, add:
```toml
tower-http = { version = "0.6", features = ["cors"] }
```

- [ ] **Step 2: Write the failing tests**

In `crates/fermentool-core/src/api.rs` `mod tests`, add (the existing tests use `router(test_state())` + `tower::ServiceExt::oneshot` + `http::Request`):
```rust
#[tokio::test]
async fn cors_allows_the_tauri_origin() {
    let app = router(test_state());
    let res = app
        .oneshot(
            http::Request::builder()
                .method("GET")
                .uri("/api/status")
                .header("origin", "http://tauri.localhost")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), http::StatusCode::OK);
    assert_eq!(
        res.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("http://tauri.localhost"),
    );
}

#[tokio::test]
async fn cors_preflight_is_answered() {
    let app = router(test_state());
    let res = app
        .oneshot(
            http::Request::builder()
                .method("OPTIONS")
                .uri("/api/config")
                .header("origin", "http://tauri.localhost")
                .header("access-control-request-method", "PUT")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // tower-http answers a valid preflight with 200 and the allow headers.
    assert!(res.status().is_success());
    assert!(res.headers().contains_key("access-control-allow-methods"));
}

#[tokio::test]
async fn cors_ignores_an_unknown_origin() {
    let app = router(test_state());
    let res = app
        .oneshot(
            http::Request::builder()
                .method("GET")
                .uri("/api/status")
                .header("origin", "http://evil.example")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Request still served (CORS is browser-enforced), but no allow-origin echo.
    assert_eq!(res.status(), http::StatusCode::OK);
    assert!(res.headers().get("access-control-allow-origin").is_none());
}
```
If `http` / `axum::body::Body` aren't already imported in the test module, match whatever the sibling tests use (they build requests already — copy their imports).

- [ ] **Step 3: Run the tests, verify they fail**

Run: `cargo test -p fermentool-core cors_`
Expected: FAIL — no `access-control-allow-origin` header (layer not added yet).

- [ ] **Step 4: Add the layer**

In `crates/fermentool-core/src/api.rs`, at the top add `use tower_http::cors::{Any, CorsLayer};`. In `router()`, build the layer and attach it to the returned `Router` (after `.fallback(static_handler)`):
```rust
let cors = CorsLayer::new()
    .allow_origin([
        "http://tauri.localhost".parse().unwrap(),
        "https://tauri.localhost".parse().unwrap(),
        "tauri://localhost".parse().unwrap(),
    ])
    .allow_methods(Any)
    .allow_headers(Any);
```
and `.layer(cors)` in the builder chain.

- [ ] **Step 5: Run the tests, verify they pass**

Run: `cargo test -p fermentool-core cors_`
Expected: PASS (3 tests).

- [ ] **Step 6: (only if `SPIKE_WS_NEEDS_ORIGIN_FIX == yes`) allow the Origin on the WS upgrade**

Find the `/api/ws` handler in `api.rs` (`ws_upgrade`). Before `ws.on_upgrade(...)`, read the `Origin` header from the request and reject with `StatusCode::FORBIDDEN` unless it is one of the three allowed values or absent. Add a test `ws_upgrade_accepts_the_tauri_origin` mirroring the style above (assert the upgrade returns `101` or the handler's normal path). Skip this whole step if the spike said `no`.

- [ ] **Step 7: Full suite + commit**

Run: `cargo test`
Expected: all pass (existing 90 + 15 + 18, plus the new CORS tests).
```bash
git add crates/fermentool-core/Cargo.toml crates/fermentool-core/src/api.rs Cargo.lock
git commit -m "feat(core): allow the Tauri webview origin (CORS)"
```

---

## Task 2: API base in the UI

**Goal:** The Svelte UI targets `http://127.0.0.1:8730` when it runs inside a Tauri window, and stays on relative paths in the browser and `npm run dev`.

**Files:**
- Modify: `ui/src/lib/api.js` (whole file is ~75 lines; touch the top constant, `req()`, and `connectWs()`)
- Verify: `ui/dist/` after `npm run build`

**Interfaces:**
- Consumes: nothing.
- Produces: `get`, `post`, `put`, `del`, `connectWs` — same names and signatures as today. Behaviour identical when `BASE === ''`.

- [ ] **Step 1: Add the base constant**

At the top of `ui/src/lib/api.js`, replace the `// Thin wrappers …` comment block's first line and add:
```js
// Thin wrappers over the daemon's REST + WebSocket API.
//
// In a Tauri window the UI is bundled into the app, so it runs on a different
// origin (http://tauri.localhost) from the daemon and must use an absolute URL.
// In the browser (Vite dev proxy, or the daemon serving ui/dist itself) it is
// same-origin, so BASE is '' and every path stays relative — unchanged.
const BASE =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
    ? 'http://127.0.0.1:8730'
    : '';
```

- [ ] **Step 2: Use it in `req()`**

In `req()`, change `fetch(path, {` to `fetch(BASE + path, {`.

- [ ] **Step 3: Use it in `connectWs()`**

In `connectWs()`'s `open()`:
- Replace the WS URL line. Currently:
  ```js
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  ws = new WebSocket(`${proto}://${location.host}/api/ws`);
  ```
  with:
  ```js
  const wsBase = BASE
    ? BASE.replace(/^http/, 'ws')
    : `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}`;
  ws = new WebSocket(`${wsBase}/api/ws`);
  ```
- In the same function, the `onopen` snapshot fetch `fetch('/api/status')` → `fetch(BASE + '/api/status')`.

- [ ] **Step 4: Verify the browser path is unchanged**

Run:
```bash
node -e "global.window=undefined; const s=require('fs').readFileSync('ui/src/lib/api.js','utf8'); if(!s.includes('BASE + path')||!s.includes('__TAURI_INTERNALS__')) process.exit(1); console.log('api.js wired');"
cd ui && npm run build
```
Expected: build succeeds; `dist/assets/index-*.js` regenerated.

- [ ] **Step 5: Manual browser smoke**

Start the daemon (`cargo run -p fermentool-core`), open `http://127.0.0.1:8730`. Expected: UI loads, status is live (WS connected), Settings save works — i.e. `BASE === ''` behaves exactly as before.

- [ ] **Step 6: Commit**

```bash
git add ui/src/lib/api.js ui/dist
git commit -m "feat(ui): use an absolute API base when bundled in Tauri"
```

---

## Task 3: `src-tauri` crate — window on the bundled UI

**Goal:** A `fermentool-tauri` crate that builds and, via `cargo tauri dev`, shows a window rendering the bundled `ui/dist`. No daemon-spawn logic yet — you run `cargo run -p fermentool-core` by hand alongside.

**Files:**
- Create: `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/tauri.conf.json`, `src-tauri/src/main.rs`, `src-tauri/icons/` (generated)
- Modify: `Cargo.toml` (root — `members`), `.gitignore` (root)

**Interfaces:**
- Consumes: `ui/dist/` (built in Task 2).
- Produces: crate `fermentool-tauri`, binary target `fermentool`. `cargo build -p fermentool-tauri` compiles.

- [ ] **Step 1: Generate app icons**

Run (from `src-tauri/` after creating it, or repo root):
```powershell
cargo tauri icon C:\Users\Administrateur\Documents\SOFT_Homemade\Fermentool\logos\export\<the-square-logo>.png
```
(pick the largest square PNG in `logos/export/` or `ui/public/`). This writes `src-tauri/icons/`.

- [ ] **Step 2: Write `src-tauri/Cargo.toml`**

```toml
[package]
name = "fermentool-tauri"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true

[[bin]]
name = "fermentool"
path = "src/main.rs"

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
tauri-plugin-single-instance = "2"
tauri-plugin-shell = "2"
tauri-plugin-process = "2"
ureq = "2"
serde_json = "1"

[features]
# default Tauri custom-protocol flag for production builds
custom-protocol = ["tauri/custom-protocol"]
```

- [ ] **Step 3: Write `src-tauri/build.rs`**

```rust
fn main() {
    tauri_build::build();
}
```

- [ ] **Step 4: Write `src-tauri/tauri.conf.json`**

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "Fermentool",
  "version": "0.1.3",
  "identifier": "com.fermentool.app",
  "build": {
    "frontendDist": "../ui/dist"
  },
  "app": {
    "windows": [
      {
        "title": "Fermentool",
        "width": 1200,
        "height": 800,
        "minWidth": 900,
        "minHeight": 600,
        "resizable": true,
        "center": true,
        "visible": false
      }
    ],
    "security": {
      "csp": "default-src 'self'; connect-src 'self' http://127.0.0.1:8730 ws://127.0.0.1:8730; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'"
    }
  },
  "bundle": {
    "active": true,
    "targets": ["nsis"],
    "icon": ["icons/32x32.png", "icons/128x128.png", "icons/icon.ico"],
    "externalBin": ["binaries/fermentool-core"],
    "windows": {
      "webviewInstallMode": { "type": "downloadBootstrapper" }
    }
  }
}
```
Set `"version"` to match `Cargo.toml` `workspace.package.version`.

- [ ] **Step 5: Write a minimal `src-tauri/src/main.rs`**

```rust
// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = tauri::Manager::get_webview_window(app, "main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let w = tauri::Manager::get_webview_window(app, "main").unwrap();
            w.show()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Fermentool");
}
```

- [ ] **Step 6: Register the crate in the workspace**

Root `Cargo.toml`: change `members = ["crates/*"]` to `members = ["crates/*", "src-tauri"]`.
Root `.gitignore`: add
```
/src-tauri/target
/src-tauri/binaries/*.exe
/src-tauri/gen
```

- [ ] **Step 7: Build and run**

Run:
```powershell
cargo build -p fermentool-tauri
```
Expected: compiles (first build is slow — pulls the tauri tree).

Then, in one terminal `cargo run -p fermentool-core` (simulator), in another:
```powershell
cargo tauri dev --config src-tauri/tauri.conf.json
```
Expected: a window titled "Fermentool" opens showing the real UI. Because the daemon is running, status goes live within ~2 s. Confirm Settings save works (this exercises the CORS PUT from Task 1).

- [ ] **Step 8: Commit**

```bash
git add src-tauri Cargo.toml Cargo.lock .gitignore
git commit -m "feat(tauri): shell crate rendering the bundled UI"
```

---

## Task 4: Detached daemon lifecycle in the shell

**Goal:** On launch the shell health-checks `:8730`; if nothing answers it spawns the sidecar **detached** and waits for it to bind. Killing the shell leaves the daemon running — the hard gate.

**Files:**
- Create: `src-tauri/src/daemon.rs`
- Modify: `src-tauri/src/main.rs` (call into `daemon`), `src-tauri/tauri.conf.json` (already has `externalBin`)
- Create: `src-tauri/scripts/copy-sidecar.ps1` (or a `just` recipe) + note in the plan's build steps

**Interfaces:**
- Consumes: `tauri_plugin_shell` (path resolution), the sidecar at `src-tauri/binaries/fermentool-core-x86_64-pc-windows-msvc.exe`.
- Produces `src-tauri/src/daemon.rs`:
  - `pub fn is_up() -> bool` — `GET http://127.0.0.1:8730/api/status`, 500 ms timeout, true iff HTTP 200.
  - `pub fn spawn_detached(exe: &std::path::Path) -> std::io::Result<()>` — raw `Command`, creation flags `0x00000008 | 0x00000200`, `Child` dropped.
  - `pub fn wait_until_up(timeout: std::time::Duration) -> bool` — poll `is_up()` every 200 ms until true or timeout.
  - `pub fn shutdown() -> bool` — `POST http://127.0.0.1:8730/api/shutdown`, 2 s timeout.
  - `pub fn sidecar_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf>` — resolves the bundled sidecar (`app.path().resource_dir()` + platform-suffixed name) in a bundle, or the `target/<triple>/…` copy in `cargo tauri dev`.

- [ ] **Step 1: Write `src-tauri/src/daemon.rs`**

```rust
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const BASE: &str = "http://127.0.0.1:8730";

pub fn is_up() -> bool {
    ureq::get(&format!("{BASE}/api/status"))
        .timeout(Duration::from_millis(500))
        .call()
        .map(|r| r.status() == 200)
        .unwrap_or(false)
}

pub fn wait_until_up(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if is_up() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    is_up()
}

pub fn shutdown() -> bool {
    ureq::post(&format!("{BASE}/api/shutdown"))
        .timeout(Duration::from_secs(2))
        .call()
        .is_ok()
}

#[cfg(windows)]
pub fn spawn_detached(exe: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    const FLAGS: u32 = 0x0000_0008 | 0x0000_0200;
    let child = Command::new(exe)
        .creation_flags(FLAGS)
        .current_dir(exe.parent().unwrap_or_else(|| Path::new(".")))
        .spawn()?;
    drop(child); // never wait — the daemon must outlive us
    Ok(())
}

#[cfg(not(windows))]
pub fn spawn_detached(exe: &Path) -> io::Result<()> {
    let child = Command::new(exe).spawn()?;
    drop(child);
    Ok(())
}

pub fn sidecar_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    let name = if cfg!(windows) {
        "fermentool-core-x86_64-pc-windows-msvc.exe"
    } else {
        "fermentool-core"
    };
    // bundled: alongside the app exe / in resources
    if let Ok(dir) = app.path().resource_dir() {
        let p = dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join(name);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}
```

- [ ] **Step 2: Wire it into `main.rs` `setup`**

```rust
mod daemon;
mod tray; // added in Task 5; create an empty `pub fn build() {}` stub now if needed

// in .setup(|app|):
let handle = app.handle().clone();
std::thread::spawn(move || {
    if !daemon::is_up() {
        if let Some(exe) = daemon::sidecar_path(&handle) {
            let _ = daemon::spawn_detached(&exe);
        }
        daemon::wait_until_up(std::time::Duration::from_secs(15));
    }
    if let Some(w) = tauri::Manager::get_webview_window(&handle, "main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
});
```
Remove the unconditional `w.show()?` from Task 3's setup (the thread shows it now).

- [ ] **Step 3: Add the sidecar copy script**

`src-tauri/scripts/copy-sidecar.ps1`:
```powershell
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
& npm --prefix "$root\ui" run build
& cargo build --release -p fermentool-core --target x86_64-pc-windows-msvc
New-Item -ItemType Directory -Force "$PSScriptRoot\..\binaries" | Out-Null
Copy-Item -Force `
  "$root\target\x86_64-pc-windows-msvc\release\fermentool-core.exe" `
  "$PSScriptRoot\..\binaries\fermentool-core-x86_64-pc-windows-msvc.exe"
Write-Host "sidecar copied"
```

- [ ] **Step 4: Manual verification — spawn**

Run `.\src-tauri\scripts\copy-sidecar.ps1`, then (no daemon running — `taskkill /F /IM fermentool-core.exe` first):
```powershell
cargo tauri dev --config src-tauri/tauri.conf.json
```
Expected: window opens; within ~3 s status goes live (the shell spawned the daemon). `tasklist | findstr fermentool-core` shows one process.

- [ ] **Step 5: Manual verification — THE GATE: daemon survives the shell**

With the above running, in another terminal:
```powershell
# kill only the shell, not the daemon
taskkill /F /IM fermentool.exe
timeout /t 2
curl http://127.0.0.1:8730/api/status
```
Expected: `curl` still returns a 200 JSON body. **If the daemon died with the shell, stop — the creation flags or the `Child` handling is wrong. Do not proceed.**

- [ ] **Step 6: Manual verification — attach, no double-spawn**

Start `fermentool-core.exe` by hand, then `cargo tauri dev`. Expected: window shows immediately, `tasklist` shows exactly one `fermentool-core.exe` (the shell did not spawn a second).

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/daemon.rs src-tauri/src/main.rs src-tauri/scripts/copy-sidecar.ps1
git commit -m "feat(tauri): spawn the daemon detached; attach if already up"
```

---

## Task 5: Tray icon + close-to-tray

**Goal:** Closing the window hides it to a tray icon; the daemon keeps running. Tray menu opens the window or shuts the daemon down and quits.

**Files:**
- Create: `src-tauri/src/tray.rs`
- Modify: `src-tauri/src/main.rs` (build the tray in `setup`, handle `CloseRequested`)

**Interfaces:**
- Consumes: `daemon::shutdown()` (Task 4).
- Produces `src-tauri/src/tray.rs`:
  - `pub fn build(app: &tauri::AppHandle) -> tauri::Result<()>` — creates the tray icon, menu, and click/menu handlers.

- [ ] **Step 1: Write `src-tauri/src/tray.rs`**

```rust
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

use crate::daemon;

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Fermentool", true, None::<&str>)?;
    let quit_daemon =
        MenuItem::with_id(app, "quit_daemon", "Shut down daemon & quit", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit (leave daemon running)", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &sep, &quit_daemon, &quit])?;

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("Fermentool")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            "quit_daemon" => {
                let _ = daemon::shutdown();
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, .. } = event {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}
```

- [ ] **Step 2: Build the tray + intercept close in `main.rs`**

In `.setup(|app|)` add `tray::build(&app.handle())?;`. Add a window-event handler:
```rust
.on_window_event(|window, event| {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        if window.label() == "main" {
            api.prevent_close();
            let _ = window.hide();
        }
    }
})
```
(chain it on `tauri::Builder::default()` before `.run(...)`).

- [ ] **Step 3: Manual verification**

`cargo tauri dev`. Expected:
1. Close the window (X) → window disappears, tray icon remains, `curl :8730/api/status` still 200.
2. Left-click the tray icon → window reappears.
3. Tray menu → "Open Fermentool" → focuses the window.
4. Launch a second `cargo tauri dev` (or the built exe) → the first window comes to front, no second process.
5. Tray menu → "Shut down daemon & quit" → `curl :8730/api/status` fails (connection refused), the app process is gone.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/tray.rs src-tauri/src/main.rs
git commit -m "feat(tauri): tray icon, close-to-tray, shutdown menu"
```

---

## Task 6: NSIS installer + login task + docs

**Goal:** `cargo tauri build` produces a signed-less NSIS `setup.exe` that installs the app, bundles the daemon sidecar, and registers a per-user "at log on" scheduled task. Uninstall removes both.

**Files:**
- Create: `src-tauri/installer/hooks.nsh`, `src-tauri/installer/fermentool-task.xml`
- Modify: `src-tauri/tauri.conf.json` (NSIS `installerHooks`, `resources` for the task XML)
- Modify: `docs/service-install.md`, `README.md`, `docs/IMPLEMENTATION_PLAN.md`

**Interfaces:**
- Consumes: the sidecar copied by `scripts/copy-sidecar.ps1` (Task 4).
- Produces: `target/release/bundle/nsis/Fermentool_<version>_x64-setup.exe`.

- [ ] **Step 1: Write `src-tauri/installer/fermentool-task.xml`**

```xml
<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Starts the Fermentool pump-control daemon at log on.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger><Enabled>true</Enabled></LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
  </Settings>
  <Actions Context="Author">
    <Exec><Command>{{EXE}}</Command></Exec>
  </Actions>
</Task>
```

- [ ] **Step 2: Write `src-tauri/installer/hooks.nsh`**

```nsi
!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToLog 'powershell -NoProfile -Command "(Get-Content -Raw \"$INSTDIR\fermentool-task.xml\") -replace \"{{EXE}}\", \"$INSTDIR\fermentool-core.exe\" | Set-Content -Encoding Unicode \"$INSTDIR\fermentool-task.xml\""'
  nsExec::ExecToLog 'schtasks /create /tn "Fermentool" /xml "$INSTDIR\fermentool-task.xml" /f'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog 'schtasks /delete /tn "Fermentool" /f'
  nsExec::ExecToLog 'powershell -NoProfile -Command "try { Invoke-WebRequest -UseBasicParsing -Method POST http://127.0.0.1:8730/api/shutdown -TimeoutSec 2 | Out-Null } catch {}"'
!macroend
```

- [ ] **Step 3: Reference the hooks and ship the XML + sidecar**

In `src-tauri/tauri.conf.json` `bundle`:
```json
"resources": ["installer/fermentool-task.xml"],
"windows": {
  "webviewInstallMode": { "type": "downloadBootstrapper" },
  "nsis": { "installerHooks": "installer/hooks.nsh" }
}
```
Confirm `externalBin: ["binaries/fermentool-core"]` is present so `fermentool-core.exe` lands in `$INSTDIR`.

- [ ] **Step 4: Build the installer**

```powershell
.\src-tauri\scripts\copy-sidecar.ps1
cargo tauri build --config src-tauri/tauri.conf.json
```
Expected: `target/release/bundle/nsis/Fermentool_0.1.3_x64-setup.exe` exists. First run downloads NSIS — allow it.

- [ ] **Step 5: Manual verification on a clean Windows session (VM or a fresh user profile)**

1. Run the setup exe → installs without error.
2. `schtasks /query /tn Fermentool` → task present, "Ready".
3. Launch **Fermentool** from the Start menu → window opens, daemon spawns (task hasn't fired this session), UI live.
4. Close to tray; `curl http://127.0.0.1:8730/api/status` → 200.
5. Sign out and back in (or reboot) **without opening the app** → `curl http://127.0.0.1:8730/api/status` → 200 (the task started it).
6. Open the app → attaches (one `fermentool-core.exe`). If a sim run was left active before the reboot, the resume prompt shows.
7. Uninstall → `schtasks /query /tn Fermentool` → "ERROR: cannot find", `curl :8730` → refused, no `fermentool-core.exe` in `tasklist`.

- [ ] **Step 6: Write the docs**

`docs/service-install.md` — add a section:
```markdown
## Windows — the Fermentool app (recommended)

The installer (`Fermentool_<version>_x64-setup.exe`) does this for you:

- installs `fermentool.exe` (the window) and `fermentool-core.exe` (the daemon);
- registers a scheduled task **Fermentool** that starts the daemon at log on
  and restarts it (every 1 min, 3 times) if it exits.

So after a reboot the daemon is back within seconds and — if a run was
interrupted — the app shows the resume prompt when you open it. Closing the
app window leaves the daemon running; use the tray menu's *Shut down daemon &
quit* to stop it. Uninstalling removes the task and stops the daemon.

The manual Task Scheduler / NSSM notes below are only for a headless machine
with no interactive login.
```
`README.md` — add under build/packaging:
```markdown
### Windows installer

    npm --prefix ui ci && npm --prefix ui run build
    ./src-tauri/scripts/copy-sidecar.ps1     # builds ui + the daemon sidecar
    cargo tauri build --config src-tauri/tauri.conf.json
    # -> target/release/bundle/nsis/Fermentool_<version>_x64-setup.exe

Unsigned: Windows SmartScreen will warn on first run. Code signing is a follow-up.
```
`docs/IMPLEMENTATION_PLAN.md` §9 — change the "Optional native WebView shell" line to note it is implemented for Windows (Tauri v2, `src-tauri/`), and that "no core changes" was relaxed to a single `CorsLayer` — see `docs/superpowers/specs/2026-09-10-tauri-webview-shell-design.md`.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/installer src-tauri/tauri.conf.json docs/service-install.md README.md docs/IMPLEMENTATION_PLAN.md
git commit -m "feat(tauri): NSIS installer + at-logon daemon task; docs"
```

---

## Self-Review

**1. Spec coverage:**

| Spec section | Task |
|---|---|
| §Goal 1 — native window, bundled UI | Task 3 |
| §Goal 2 — detached daemon, survives window close | Task 4 (Step 5 gate) |
| §Goal 3 — auto-start at login | Task 6 (task XML + hook) |
| §Goal 4 — one installer .exe | Task 6 |
| §"§9 relaxed" — CorsLayer + api.js base | Task 1, Task 2 |
| §Implementation order — Step 0 spike | Task 0 |
| §Design 1 — `src-tauri/` crate, Tauri v2, conf.json | Task 3 |
| §Design 2 — api.js BASE + CorsLayer + optional WS Origin | Task 2, Task 1 (Step 6) |
| §Design 3 — health-check, detached spawn, poll, show window | Task 4 |
| §Design 4 — tray, close-to-tray, menu | Task 5 |
| §Design 5 — NSIS, hooks, task XML, uninstall | Task 6 |
| §Design 6 — sidecar copy, no version skew | Task 4 (Step 3 script) |
| §Design 7 — repo changes table | Tasks 1-6 (files match) |
| §Testing — spike, dev-machine manual, installer VM | Task 0 Step 4, Task 4-5 manual steps, Task 6 Step 5 |
| §Risks — detached child | Task 4 Step 5 (hard gate) |
| §Risks — WS Origin | Task 0 Step 4 + Task 1 Step 6 |
| §Risks — WebView2 bootstrapper | Task 3 conf.json (`downloadBootstrapper`) |
| §Risks — per-user task | documented, Task 6 Step 6 |
| §Risks — port hard-coded | Global Constraints + Task 2 comment |
| §Risks — toolchain | Task 0 Step 1 |
| §Risks — code signing | Task 6 Step 6 README note |

No gaps.

**2. Placeholder scan:** No "TBD"/"handle edge cases"/"similar to Task N". Task 0's spike steps describe manual actions with exact expected console output. The one conditional (Task 1 Step 6 / WS Origin) is gated on a spike output value defined in Task 0's Interfaces.

**3. Type consistency:** `daemon::is_up()`, `spawn_detached(&Path)`, `wait_until_up(Duration)`, `shutdown()`, `sidecar_path(&AppHandle)` — defined in Task 4 Interfaces, used with the same names/signatures in Task 4 Step 2 and Task 5 (`daemon::shutdown()`). `tray::build(&AppHandle)` — defined Task 5 Interfaces, called Task 5 Step 2. `router(state) -> Router` unchanged. `BASE` constant name consistent across Task 2 steps. Window label `"main"` consistent (Task 3 conf.json has one unnamed window → Tauri labels it `"main"` by default; Task 4/5 use `get_webview_window("main")` — correct).

---

## Spike result (Task 0)

_(filled in by Task 0 Step 5)_
