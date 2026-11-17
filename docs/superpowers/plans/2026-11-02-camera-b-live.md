# Camera, plan B: helper process, live view, Settings > Camera Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pick a webcam in Settings > Camera and watch the reactor live on a new Camera page, with the capture in a supervised helper process that can hang or crash without touching the daemon.

**Architecture:** A new crate `fermentool-camera` has a library (the pipe protocol, no capture code) and a binary `fermentool-camera.exe` (feature `capture`) that lists devices, opens one and returns JPEG frames over stdin/stdout. In the daemon, `camera.rs` runs a supervisor thread that spawns the helper at below-normal priority, bounds every request with a timeout, kills and respawns a hung or dead helper, keeps the latest frame, and grabs at live rate only while someone watches. The API serves the device list, connect/disconnect, the latest frame and an MJPEG stream; the UI adds the rail entry, the Camera page (Live) and the Settings section.

**Tech Stack:** Rust (new crate `fermentool-camera`, `nokhwa` 0.10 with `input-native` for capture, `jpeg-encoder` for JPEG), axum (MJPEG `multipart/x-mixed-replace`), Svelte 5.

**Spec:** `docs/superpowers/specs/2026-11-02-camera-timelapse-design.md`. Mockup: https://claude.ai/artifact/PQq3jCwc41jTj9vYyPyyo7 (artboards "Camera page" and "Settings > Camera").

Depends on plan A (timing baseline). Plans after: C (timelapses), D (History, exports, installer).

## Global Constraints

- **No camera call on the control thread, ever.** The camera lives in `camera.rs` and its own threads; the engine and `control.rs` do not know it exists in this plan.
- **Capture runs in `fermentool-camera.exe`**, spawned with `CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS`; the daemon owns its lifetime; the helper exits when its stdin closes (daemon gone), so it never holds the camera orphaned.
- **Every request to the helper is bounded**: `REQUEST_TIMEOUT` 5 s (list, grab, close), `OPEN_TIMEOUT` 10 s (open). Past it, the helper is killed and respawned; a reply never blocks an API handler longer than that (handlers read the cache, they do not call the helper, except connect and the device list, which run in `spawn_blocking`).
- Respawn backoff: 2 s doubling to 30 s, reset after 60 s of a healthy helper.
- Live rate `LIVE_FPS` = 5 while at least one live stream is open; no grabbing otherwise in this plan.
- Picture sizes offered: 640 × 480, 1280 × 720, 1920 × 1080, filtered to what the device reports. JPEG quality 85.
- Device identity in `config.toml`: `[camera] device = "<friendly name>"`, `device_index = <n>` (0 = first device with that name), `width`, `height`, `enabled`, `test_pattern = false`. Absent section = no camera.
- Static CRT for the helper too (`.cargo/config.toml` already applies to every exe); `copy-sidecar.ps1` refuses a helper importing `VCRUNTIME140.dll`.
- UI: `docs/DESIGN.md` (state colours: camera lost / in use = `warn` orange, never red; Plex Sans; `*-ink` tokens; `.seg` gets `use:segThumb`; `LinkHeader` for the link state; Settings card rules). UI strings English. No em dash, no spaced hyphen.
- UI done means: `npm run check` zero errors and warnings, `npm test`, `npm run build`, deploy when no run is active, `npm run smoke` (`CLAUDE.md`). Smoke never clicks Connect or Disconnect.
- Commit messages without `Co-Authored-By: Claude` / `Claude-Session`.

## Review Focus

- The daemon is killed (Task Manager) while the live view is open: the helper sees stdin close and exits within 1 s, the webcam light goes off (no orphan). Pinned in Task 3 (`serve_returns_on_eof`).
- The webcam is unplugged during a live stream: the stream stops sending new frames, `/api/camera` says `lost`, the page shows the orange band; replugged, it comes back without a click. Pinned in Task 4 (`grab_error_marks_lost_then_recovers`).
- The browser tab with the live view is closed: the viewer count drops (the stream's drop guard), grabbing stops. Pinned in Task 5 (`closing_the_stream_releases_the_viewer`).
- Two identical webcams: `device_index` picks the second one, and the list shows "Logitech C920 (2)". Pinned in Task 6 (`deviceLabel` test).
- Another program holds the camera (Teams, Windows Camera): the state is `in_use`, Settings says so in orange, nothing retries in a tight loop. Mapping pinned in Task 3; the real error code is checked on the bench in Task 7.

---

### Task 1: Spike, can `nokhwa` capture on this PC with a static CRT?

Throwaway: decides the capture backend for Task 3.

**Files:**
- Create (scratch, not committed): a binary in the scratchpad that depends on `nokhwa = { version = "0.10", features = ["input-native"] }`.

- [ ] **Step 1:** List devices (`nokhwa::query(ApiBackend::MediaFoundation)`) and print name, index and the formats each reports.
- [ ] **Step 2:** Open "Integrated Webcam" at 1280 × 720, grab 10 frames, decode to RGB, encode one to JPEG with `jpeg-encoder`, write it out. Build with the workspace's `+crt-static` (VS Community environment).
- [ ] **Step 3:** Open the camera in the Windows Camera app, then try Step 2 again; note the exact error (type and HRESULT, if any).
- [ ] **Step 4: Decide.** Works: Task 3 uses `nokhwa`. Fails to build or capture: Task 3 uses Media Foundation directly through the `windows` crate (`MFEnumDeviceSources`, `IMFSourceReader`, MJPG or NV12 output). Write the decision and the "in use" error from Step 3 into this plan under this task, and tell the user.

**Spike result (2026-11-03):** `nokhwa` works. Resolved `nokhwa` 0.10.11, `jpeg-encoder` 0.6.1; builds with `+crt-static`, no `VCRUNTIME140.dll` import. "Integrated Webcam" (the only device) at 1280 × 720: ~30 ms a frame, the first 243 ms; a JPEG is ~105 kB.
- In use (Windows Camera app holding it): `open` and `open_stream` succeed; the first `frame()` fails with `ReadFrameError("Hardware MFT failed to start streaming due to lack of hardware resources. (0xC00D3704)")`, later ones `GetPropertyError` with the same HRESULT. So `InUse` is detected at the first grab (`0xC00D3704`), not at open: the helper's `open` grabs one frame to confirm.
- Not found: `OpenDeviceError("No Such Camera", "Not Found")`.
- The spike segfaulted at process exit after its work (exit 139): the helper closes the camera, flushes stdout and ends with `std::process::exit(0)`; check its exit code.
- Spike source and full notes: `.superpowers/sdd/2026-11-02-camera-b-live/task-1-report.md` (not committed).

### Task 2: The pipe protocol (`fermentool-camera` library)

**Files:**
- Create: `crates/fermentool-camera/Cargo.toml` (lib + bin `fermentool-camera`, `[features] capture = ["dep:nokhwa", "dep:jpeg-encoder"]`, bin `required-features = ["capture"]`, version from the workspace)
- Create: `crates/fermentool-camera/src/lib.rs`, `crates/fermentool-camera/src/protocol.rs`

**Interfaces:**
- Produces:
  - `#[derive(Serialize, Deserialize)] pub enum Request { List, Open { device: String, index: u32, width: u32, height: u32 }, Grab, Close, Quit }`
  - `pub struct DeviceInfo { pub name: String, pub index: u32, pub sizes: Vec<(u32, u32)> }`
  - `pub enum Reply { Devices(Vec<DeviceInfo>), Opened { width: u32, height: u32 }, Frame(Vec<u8>) /* JPEG */, Closed, Error { code: ErrorCode, message: String } }`
  - `pub enum ErrorCode { NotFound, InUse, Failed }`
  - `pub fn write_request(w: &mut impl Write, r: &Request) -> io::Result<()>`, `pub fn read_request(r: &mut impl Read) -> io::Result<Option<Request>>` (`None` on clean EOF)
  - `pub fn write_reply(w: &mut impl Write, r: &Reply) -> io::Result<()>`, `pub fn read_reply(r: &mut impl Read) -> io::Result<Reply>`
  - Framing: 4-byte little-endian length, then 1 kind byte (`b'J'` JSON, `b'F'` frame JPEG bytes), then the payload. `MAX_MESSAGE` = 16 MiB; a larger length is `io::ErrorKind::InvalidData`.

- [ ] **Step 1: Write the failing tests** (`protocol.rs`)
  - `request_round_trips` for each `Request` variant through a `Vec<u8>`.
  - `frame_reply_round_trips_without_json`: a 100 kB `Reply::Frame` round-trips and its encoded size is `100_000 + 5`.
  - `clean_eof_is_none`: `read_request` on an empty reader is `Ok(None)`.
  - `truncated_message_is_an_error`: a header announcing 100 bytes followed by 10 is `Err(UnexpectedEof)`.
  - `oversized_length_is_refused`: a header of `MAX_MESSAGE + 1` is `Err(InvalidData)` without allocating it.
- [ ] **Step 2:** Run `cargo test -p fermentool-camera`, expect FAIL (not defined).
- [ ] **Step 3:** Implement as in Interfaces (serde_json for the JSON kind).
- [ ] **Step 4:** Run `cargo test -p fermentool-camera`, expect 5 passed.
- [ ] **Step 5: Commit** `feat(camera): helper pipe protocol`.

### Task 3: The helper binary

**Files:**
- Create: `crates/fermentool-camera/src/main.rs`, `crates/fermentool-camera/src/serve.rs`, `crates/fermentool-camera/src/backend.rs`, `crates/fermentool-camera/src/test_pattern.rs`

**Interfaces:**
- Consumes: protocol (Task 2).
- Produces:
  - `pub trait Backend { fn list(&mut self) -> Result<Vec<DeviceInfo>, (ErrorCode, String)>; fn open(&mut self, device: &str, index: u32, width: u32, height: u32) -> Result<(u32, u32), (ErrorCode, String)>; fn grab(&mut self) -> Result<Vec<u8>, (ErrorCode, String)>; fn close(&mut self); }`
  - `pub fn serve(input: impl Read, output: impl Write, backend: &mut dyn Backend) -> io::Result<()>`: answers each request in order; returns on `Quit` or clean EOF, closing the device first.
  - `TestPattern` backend: one device named `"Test pattern"`, sizes as the global list, frames are a JPEG of a grey field with a bar whose position follows the frame count (so a live view visibly moves).
  - The real backend (per Task 1's decision) behind `feature = "capture"`; maps "device busy" (the error noted in Task 1, Step 3) to `ErrorCode::InUse`, "no such device" to `NotFound`, anything else to `Failed`.
  - `main`: `--test-pattern` adds the TestPattern device to the real backend's list (a `Combined` backend); stdout is the protocol only, diagnostics to stderr.

- [ ] **Step 1: Write the failing tests** (`serve.rs`, with `TestPattern`)
  - `serve_lists_opens_grabs_and_quits`: requests `List, Open{"Test pattern",0,640,480}, Grab, Quit` produce `Devices([..1 device..]), Opened{640,480}, Frame(jpeg starting with FF D8), ` and `serve` returns `Ok(())`.
  - `serve_returns_on_eof`: an input with `List` then EOF returns `Ok(())` after one reply.
  - `grab_before_open_is_an_error`: `Grab` alone replies `Error{code: Failed, ..}` and the loop continues.
  - `unknown_device_is_not_found`: `Open{"Nope",0,640,480}` replies `Error{code: NotFound, ..}`.
- [ ] **Step 2:** Run `cargo test -p fermentool-camera`, expect FAIL.
- [ ] **Step 3:** Implement `serve`, `TestPattern`, the real backend and `main`.
- [ ] **Step 4:** Run `cargo test -p fermentool-camera` (expect all pass), then `cargo build -p fermentool-camera --features capture` and, by hand, pipe a List request to the exe (a 10-line Python script in the scratchpad): the integrated webcam is listed.
- [ ] **Step 5: Commit** `feat(camera): fermentool-camera helper with test pattern`.

### Task 4: The supervisor in the daemon

**Files:**
- Create: `crates/fermentool-core/src/camera.rs`
- Modify: `crates/fermentool-core/src/lib.rs`, `crates/fermentool-core/Cargo.toml` (depend on `fermentool-camera` without `capture`)

**Interfaces:**
- Consumes: protocol (Task 2).
- Produces:
  - `pub struct HelperIo { pub to_helper: Box<dyn Write + Send>, pub from_helper: Box<dyn Read + Send>, pub kill: Box<dyn FnMut() + Send> }`
  - `pub trait Launch: Send + 'static { fn launch(&mut self) -> io::Result<HelperIo>; }` and `pub struct ProcessLaunch { pub exe: PathBuf, pub test_pattern: bool }` (the real one: `std::process::Command`, piped stdin/stdout, `creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS)` on Windows).
  - `pub fn helper_exe() -> Option<PathBuf>`: next to `current_exe()`, `fermentool-camera.exe` then `fermentool-camera-x86_64-pc-windows-msvc.exe`.
  - `#[derive(Serialize, Clone, PartialEq)] #[serde(tag = "state", rename_all = "snake_case")] pub enum CameraState { Off, Connecting { device: String }, Connected { device: String, width: u32, height: u32 }, InUse { device: String }, Lost { device: String, since: Timestamp }, NoHelper }`
  - `pub struct Frame { pub jpeg: Arc<Vec<u8>>, pub at: Timestamp, pub seq: u64 }`
  - `pub struct CameraWant { pub device: String, pub index: u32, pub width: u32, pub height: u32 }`
  - `#[derive(Clone)] pub struct Camera` (an `Arc` inside) with: `fn spawn(launch: impl Launch, want: Option<CameraWant>) -> Camera`, `fn state(&self) -> CameraState`, `fn latest(&self) -> Option<Frame>`, `fn set_want(&self, want: Option<CameraWant>)` (connect / disconnect), `fn devices(&self, refresh: bool) -> Result<Vec<DeviceInfo>, String>` (blocking, bounded by `REQUEST_TIMEOUT`, cached until `refresh`), `fn viewer(&self) -> ViewerGuard` (viewer count +1, −1 on drop), `fn wait_frame_after(&self, seq: u64, max: Duration) -> Option<Frame>` (for the stream), `fn shutdown(&self)` (sends `Quit`, kills after 1 s).
  - Threads: `camera-supervisor` (owns the helper, the state, the grab loop) and `camera-reader` per helper instance (reads replies into a channel, so a hung read never holds the supervisor; it is dropped with its helper, never joined).
  - Constants: `REQUEST_TIMEOUT` 5 s, `OPEN_TIMEOUT` 10 s, `LIVE_FPS` 5, respawn backoff 2 s × 2 up to 30 s, reset after 60 s healthy, `IN_USE_RETRY` 15 s, `LOST_RETRY` 3 s.

- [ ] **Step 1: Write the failing tests** (`camera.rs`), all with a `FakeLaunch` whose helper is an in-process thread speaking the protocol over `os_pipe` or a pair of `std::sync::mpsc`-backed `Read`/`Write` adapters (implementer's choice), scriptable to answer, hang or die:
  - `connects_and_reports_connected`: want = Test pattern 640 × 480 → within 2 s, `state()` is `Connected { width: 640, height: 480, .. }`.
  - `no_grab_without_viewers`: connected, no viewer, 1 s later the fake received no `Grab`.
  - `viewer_drives_live_grabs`: hold a `viewer()` for 1 s → the fake received 4 to 6 `Grab`s; `latest()` is `Some` with `seq` rising.
  - `hung_helper_is_killed_and_respawned`: the fake stops answering a `Grab`; within `REQUEST_TIMEOUT` + 1 s the kill closure ran and a second helper was launched; `state()` never blocked longer than 50 ms meanwhile (measure each call).
  - `grab_error_marks_lost_then_recovers`: the fake answers `Grab` with `Error{Failed}` → `Lost`; then answers again → `Connected` within `LOST_RETRY` + 1 s.
  - `in_use_is_reported_and_retried_slowly`: `Open` answers `Error{InUse}` → `InUse`; no second `Open` before `IN_USE_RETRY`.
  - `set_want_none_closes`: connected, `set_want(None)` → the fake receives `Close`, state `Off`.
  - `missing_helper_is_no_helper`: `launch` returns `Err(NotFound)` → `NoHelper`, retried on the backoff.
- [ ] **Step 2:** Run `cargo test -p fermentool-core camera::`, expect FAIL.
- [ ] **Step 3:** Implement as in Interfaces. The supervisor loop waits on its command channel with a timeout equal to the next due action (grab, retry), like `control_loop`.
- [ ] **Step 4:** Run `cargo test -p fermentool-core camera::`, expect 8 passed; then `cargo test -p fermentool-core` all pass.
- [ ] **Step 5: Commit** `feat(camera): supervised helper, bounded requests, live grabs`.

### Task 5: Config, API, daemon wiring

**Files:**
- Modify: `crates/fermentool-core/src/config.rs` (`CameraConfig`, `Config.camera: Option<CameraConfig>`, `validate`)
- Modify: `crates/fermentool-core/src/api.rs` (routes, `AppState.camera: Camera`, handlers)
- Modify: `crates/fermentool-core/src/main.rs` (spawn `Camera` from config; `shutdown()` on `POST /api/shutdown` and on exit)

**Interfaces:**
- Consumes: `Camera`, `CameraWant`, `ProcessLaunch`, `helper_exe` (Task 4).
- Produces (for Task 6 and plan C):
  - `CameraConfig { device: String, device_index: u32, width: u32 /* 1280 */, height: u32 /* 720 */, enabled: bool /* true */, test_pattern: bool }`; `validate`: `(width, height)` one of the three sizes, else `"camera picture size must be 640x480, 1280x720 or 1920x1080"`.
  - `GET /api/camera` → `CameraState` JSON plus `"configured": bool`.
  - `GET /api/camera/devices?refresh=1` → `[{ name, index, sizes: [[w,h],..] }]` (`spawn_blocking`).
  - `POST /api/camera/connect { device, index, width, height }` → saves `[camera]` through the same rev-checked path as `put_config`, then `set_want`; 400 on an unknown size.
  - `POST /api/camera/disconnect` → `set_want(None)` and `enabled = false` in `[camera]`, keeping the device in `config.toml` (like the balance's disconnect); connect sets `enabled = true`. At start the daemon connects only when `enabled`.
  - `GET /api/camera/frame.jpg` → latest JPEG, `Cache-Control: no-store`; 404 `"no picture yet"` when none.
  - `GET /api/camera/live` → `multipart/x-mixed-replace; boundary=frame`, one part per new frame (`wait_frame_after`, max 2 s, a keep-alive repeat of the last frame if none), holding a `ViewerGuard` for the stream's life.

- [ ] **Step 1: Write the failing tests**
  - `config.rs`: `camera_section_absent_is_none`, `camera_defaults_are_1280x720_enabled`, `camera_odd_size_is_refused`.
  - `api.rs` (with the existing test `AppState` builder, a `Camera` on a `FakeLaunch`): `camera_off_reports_off_and_not_configured`; `frame_404_before_any_picture`; `closing_the_stream_releases_the_viewer` (open `/api/camera/live`, read one part, drop the body, the viewer count is back to 0 within 1 s; expose `Camera::viewers()` for it).
- [ ] **Step 2:** `cargo test -p fermentool-core`, expect the new tests to FAIL.
- [ ] **Step 3:** Implement config, routes and wiring. The `test_pattern` flag passes `--test-pattern` to the helper.
- [ ] **Step 4:** `cargo test -p fermentool-core`, all pass.
- [ ] **Step 5:** Release build, start the exe (`FERMENTOOL_NO_BROWSER=1`), with `[camera] device = "Integrated Webcam"`: `curl /api/camera` says `connected`, `curl -o f.jpg /api/camera/frame.jpg` gives a picture, open `/api/camera/live` in Edge for 30 s and close it; then a clean `POST /api/shutdown`: no `fermentool-camera` process left (`Get-Process fermentool-camera`). Re-check `/api/status` for an active run before stopping the deployed daemon (`CLAUDE.md`).
- [ ] **Step 6: Commit** `feat(camera): camera config, API and live MJPEG stream`.

### Task 6: UI, rail entry, Camera page (Live), Settings > Camera

**Files:**
- Create: `ui/src/lib/camera.js`, `ui/src/lib/camera.test.js`, `ui/src/routes/Camera.svelte`, `ui/src/routes/settings/CameraSettings.svelte`
- Modify: `ui/src/components/Icon.svelte` (add `camera`: the Solar linear "camera" glyph, copied like the others), `ui/src/lib/route.js` (+ `route.test.js`), `ui/src/App.svelte` (rail entry under Feeding profile, route `camera`, page title "Camera"), `ui/src/routes/settings/sections.js` (`{ id: 'camera', label: 'Camera', group: 'Hardware', icon: 'camera', component: CameraSettings }` after Balance), `ui/tests/smoke.mjs`

**Interfaces:**
- Consumes: the API of Task 5.
- Produces (`lib/camera.js`, pure, for plan C too):
  - `cameraLine(cam)` → `{ tone: 'ok'|'warn'|'idle', word, detail }`: Connected (`ok`, "Connected", "<device>, <w> × <h>"), Connecting (`idle`, "Connecting…"), In use (`warn`, "Camera in use", "Another program holds it (Teams, Camera). Close it, then Connect."), Lost (`warn`, "Camera not answering", "Since <time>. The run is not affected."), Off (`idle`, "Not connected", "No camera chosen."), NoHelper (`warn`, "Camera helper missing", "Reinstall Fermentool.").
  - `deviceLabel(dev, all)` → the name, plus ` (n)` with n = index + 1 when two devices share the name.
  - `sizesFor(dev)` → the three standard sizes the device offers, as `"1280 × 720"` labels with `[w, h]`.

- [ ] **Step 1: Write the failing tests**
  - `camera.test.js`: one case per `cameraLine` state with the exact words above; `deviceLabel` gives `"Logitech C920 (2)"` for index 1 of two, and the bare name when alone; `sizesFor` drops 1920 × 1080 for a device that lacks it.
  - `route.test.js`: `parseHash('#camera', ids)` is `{ route: 'camera' }`; `#settings/camera` opens the section.
- [ ] **Step 2:** `npm test` from `ui/`, expect FAIL.
- [ ] **Step 3: Implement**, per the mockup:
  - Camera page: the run state bar on top (as other pages), the picture as `<img src={BASE + '/api/camera/live'}>` mounted only while the page is shown (unmounting closes the stream), "Live" tag and run time over it, camera name and size under it, Full screen (`requestFullscreen` on the picture box). Not configured: an empty state with a link to `#settings/camera`. Lost / in use: the orange band from `cameraLine`. Poll `GET /api/camera` every 2 s while the page is open. No Take a picture, no Timelapses tab yet (plan C).
  - Settings > Camera: `LinkHeader` from `cameraLine`, Connect / Disconnect as the other links; Camera `<select>` from `/api/camera/devices` with "Look again" (`?refresh=1`); Picture size `.seg` with `use:segThumb`; the "what this camera sees now" preview from `frame.jpg` refreshed every 2 s while the section is open; Save disabled until something changes (Connect applies device + size). The mockup's "Timelapse, every" and "Images kept in" fields come with plan C, where they mean something.
- [ ] **Step 4:** From `ui/`: `npm test` (pass), `npm run check` (zero errors, zero warnings), `npm run build`.
- [ ] **Step 5: Smoke**: in `tests/smoke.mjs`, open `#camera` (the page answers, a picture or the empty state is there, no "burst"), and `#settings/camera` (the camera select or the "Not connected" band, the page still answers after a `.seg` click on a size, nothing saved). Remember: a regex inside `await js(...)` doubles its backslashes. Deploy (no active run, re-checked), `npm run smoke`, expect all checks pass. Tell the user to Ctrl+F5.
- [ ] **Step 6: Commit** `feat(ui): Camera page with live view, Settings > Camera` and, separately, the built bundle `build(ui): bundle for the camera page`.

### Task 7: Packaging and the bench checks

**Files:**
- Modify: `src-tauri/scripts/copy-sidecar.ps1` (build `-p fermentool-camera --features capture --release --target $triple`, stage `fermentool-camera-$triple.exe`, same `VCRUNTIME140.dll` refusal), `src-tauri/tauri.conf.json` (`externalBin` gains `binaries/fermentool-camera`), `src-tauri/installer/hooks.nsh` (stop `fermentool-camera` after the daemon, before copying), `docs/RELEASE.md` (one line), `CHANGELOG.md`, `CLAUDE.md` (a "Camera" section: helper process, the never-on-control-thread rule, where to look), `docs/wiring.md` (webcam on another USB controller than the RS-485 and balance adapters when possible)

- [ ] **Step 1:** Run `copy-sidecar.ps1`: both exes staged, no VCRUNTIME import (`dumpbin /dependents`).
- [ ] **Step 2: In use, confirmed.** Open the webcam in the Windows Camera app, Connect in Settings: the band says "Camera in use". If the code differs from Task 1's note, fix the mapping in the backend and its test. State the confirmed cause only (`CLAUDE.md`: no hardware root cause from a hunch).
- [ ] **Step 3: Kill the daemon** in Task Manager with the live view open: `fermentool-camera` is gone within 1 s, the webcam light is off.
- [ ] **Step 4: Timing comparison**, range B: a 6 h bench run like plan A's baseline, the camera connected, the live view open for the first hour. `python scripts/timing_report.py <log> --a ... --b ...`: p99 of each metric with the camera within 10 % (or 20 ms, whichever is larger) of the baseline. Ask the user before taking the bench; write the result in the session notes. A failure stops plan C until understood.
- [ ] **Step 5: Commit** `build(camera): ship the capture helper in the installer` (no version bump here: only on an export, `docs/RELEASE.md`).
