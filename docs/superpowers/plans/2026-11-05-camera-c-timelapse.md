# Camera, plan C: timelapses (capture, storage, run link) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record timelapses from the daemon: linked to a run (switched on in New run, starts and stops with it) or on their own from the Camera page, each image stored as a high-quality JPEG with a thumbnail and the run's figures of that moment, so the images can later train a model.

**Architecture:** The helper gains a `Still` request (JPEG 95 plus a thumbnail). A new daemon module `timelapse/` runs a `timelapse` thread that owns the schedule, its own SQLite connection and the captures folder; it follows the run through the status broadcast (never the control thread) and asks the camera supervisor for stills. The API starts, stops and reads timelapses; the UI adds the switch in New run, two fields in Settings > Camera, and the Timelapse card, Take a picture and the latest images on the Camera page.

**Tech Stack:** Rust (`fermentool-camera` helper, `fermentool-core`, rusqlite, jiff), axum, Svelte 5.

**Spec:** `docs/superpowers/specs/2026-11-02-camera-timelapse-design.md` (behaviour), mockup https://claude.ai/artifact/PQq3jCwc41jTj9vYyPyyo7 (layout and wording: artboards "Camera page", "Settings > Camera", "New run"). Plans A (timing baseline) and B (live view, picture controls) are built on `feat/camera`.

**In this plan:** capture, schedule, storage, run link, standalone timelapses, Take a picture, camera loss and disk handling, Settings fields, New run switch, the Camera page's Timelapse card and latest images, deleting captures with their run and with Reset history.
**Plan D, not here:** the shared player ("Timelapse so far", History), the Timelapses tab, the History run detail section, .zip and .mp4 export, rename and delete of a timelapse, Browse… (native folder picker), the installer.

## Global Constraints

- **No camera or timelapse call on the control thread, ever.** The `timelapse` thread learns the run's state from `DaemonStatus` broadcasts (`api::AppState::events`, one a tick) and talks to the camera through `camera::Camera` only.
- **A camera or disk problem is never a run problem**: it never stops, pauses or alarms a run, and is never notified (`notify.rs` ignores the new event kinds).
- **Additions for the model (the user, 2026-11-05):** every stored image keeps, in its database row and in a `images.csv` beside the images, the camera name and its picture controls (JSON of `ControlInfo` list at that moment), the run elapsed time, the setpoint and its unit, the delivered volume, the balance weight and the trim factor. Stored images are **JPEG quality 95** (`STILL_QUALITY`); the live view stays 85.
- Thumbnail: 320 px wide, height to the aspect, JPEG 80 (`THUMB_WIDTH`, `THUMB_QUALITY`), made by the helper from the same frame.
- Schedule: image k is due at `anchor + k * interval` (wall clock, `jiff::Timestamp`); slots that pass without an image while recording are **missed**; slots while paused or while the run awaits resume are **not** missed; after a daemon restart the next image is the next due slot after now, never a burst.
- Thresholds: under 1 GB free the Camera page warns (`WARN_FREE_BYTES`); under 200 MB free the timelapse pauses itself (`PAUSE_FREE_BYTES`, journalled).
- `[camera] interval_min` default 5, accepted 1 to 1440. `[camera] captures_dir` default empty, meaning `%APPDATA%\Fermentool\captures` (the data dir's `captures`). A timelapse keeps the folder it started in.
- Migration **0017** (`0017_timelapses.sql`): the bench DB is at `user_version` 16 (`feat/rate-change-reset`), `main` and `feat/camera` stop at 15. Check every branch and the bench DB again before numbering.
- `timelapses.run_id` is a plain integer, no `REFERENCES runs` (foreign keys are on: a reference would block deleting a run).
- Files are served by database id only, never from a path given in a request.
- UI: `docs/DESIGN.md` (Recording is quiet green ink, never a red dot; camera lost or disk = `warn` orange; Plex Sans; `*-ink` tokens; every `.seg` has `use:segThumb`). UI strings English. No em dash, no spaced hyphen. Logic in `ui/src/lib/*.js` with tests.
- Done means: `cargo test`, `npm run check` zero errors and warnings, `npm test`, `npm run build`, deploy when no run is active, `npm run smoke` (extended, still changing nothing: it never starts a timelapse nor takes a picture). A bug fixed is a test added.
- Commit messages without `Co-Authored-By` / `Claude-Session`.

## Review Focus

- **The daemon is restarted mid-run with crash resume on "ask"**: while the run awaits Resume, no image is taken and those slots are not missed; resumed, the next image is the next due slot. Test in Task 5 (`awaiting_resume_pauses_without_missing`).
- **The captures folder disappears mid-timelapse** (USB disk unplugged): the timelapse pauses with reason `folder`, no half-written file is left, it resumes on its own once the folder is back. Test in Task 6 (`folder_gone_pauses_then_resumes`).
- **The system clock is set back an hour mid-timelapse**: no burst of images, no negative slot, the next image at the next due slot after the new now. Test in Task 3 (`clock_set_back_never_bursts`).
- **Change interval mid-timelapse**: the image already due keeps its time, the following ones use the new interval. Test in Task 3 (`interval_change_applies_after_the_next_image`).
- **Stop pressed while a still is being taken**: the image lands in the run's timelapse, then the timelapse ends; nothing is written after `ended_at`. Test in Task 5 (`run_end_during_a_still_keeps_it_and_ends`).

---

### Task 1: The helper's still (`fermentool-camera`)

**Files:**
- Modify: `crates/fermentool-camera/src/protocol.rs`, `backend.rs`, `serve.rs`, `test_pattern.rs`, `mf_backend.rs`, `pixels.rs`

**Interfaces:**
- Produces: `Request::Still { quality: u8 }`; `Reply::Still { jpeg: Vec<u8>, thumb: Vec<u8> }` sent as kind `S` with payload `u32 LE jpeg length, jpeg bytes, thumb bytes`; `Backend::still(&mut self, quality: u8) -> Result<(Vec<u8>, Vec<u8>), BackendError>`; `pixels::downscale_rgb(src: &[u8], w: u32, h: u32, tw: u32) -> (Vec<u8>, u32, u32)` (box filter, `th = round(h * tw / w)`, at least 1); constants `STILL_QUALITY: u8 = 95`, `THUMB_WIDTH: u32 = 320`, `THUMB_QUALITY: u8 = 80`, `STILL_DISCARD: u32 = 5` in `backend.rs`.

- [ ] **Step 1: Tests.** `protocol`: `still_reply_round_trips` (a 100 kB jpeg and a 5 kB thumb survive write/read; the payload length is `4 + jpeg + thumb + 1`). `pixels`: `downscale_halves_a_checkerboard` (a 4×2 RGB image of two flat colours to width 2 keeps each colour), `downscale_keeps_the_aspect` (1280×720 to 320 gives 320×180). `serve`: `still_returns_a_big_and_a_small_jpeg` on the test pattern (both start `FF D8`, thumb shorter than jpeg), `still_before_open_is_an_error`.
- [ ] **Step 2:** Run `cargo test -p fermentool-camera`: the new tests fail.
- [ ] **Step 3: Implement.** `Backend::still` defaults to an error "no camera is open". In `mf_backend`, a still reads and drops `STILL_DISCARD` frames first (a frame queued while nobody grabbed may be old), converts the last to RGB once, encodes the full JPEG at `quality` and the thumbnail at `THUMB_QUALITY` from `downscale_rgb`. Factor the RGB read of `grab_jpeg` into `read_rgb(open) -> Result<Vec<u8>, BackendError>` so grab and still share it. Test pattern and `Combined` implement it too.
- [ ] **Step 4:** `cargo test -p fermentool-camera` passes; `cargo build -p fermentool-camera --features capture` builds.
- [ ] **Step 5: Bench check** (no run active, camera disconnected from the daemon first with `POST /api/camera/disconnect`, reconnected after): drive the debug helper on the Integrated Webcam (`Open`, `Still {95}` three times 10 s apart). Note the JPEG sizes at 1280 × 720 and the still's duration, and check that the second still shows the scene of its own moment (wave a hand). Write the figures under this task.
- [ ] **Step 6: Commit** `feat(camera): helper stills, JPEG 95 with a thumbnail`.

**Bench result (2026-11-05, Integrated Webcam, debug helper):** 1280 × 720 at 95: 254 to 256 kB, thumbnail 320 × 180 ~13.5 kB, 1.2 to 1.4 s a still (5 frames dropped first, debug build). 640 × 480: 98 kB, thumbnail 320 × 240 ~14 kB, 0.4 s. Task 8 takes 255 kB and 98 kB; 1920 × 1080 is not offered by these cameras, estimated at 2.25 × 255 kB.

### Task 2: The supervisor's still and the timelapse tables

**Files:**
- Modify: `crates/fermentool-core/src/camera.rs` (fake helper too)
- Create: `crates/fermentool-core/src/store/migrations/0017_timelapses.sql`, `crates/fermentool-core/src/store/timelapse.rs`
- Modify: `crates/fermentool-core/src/store/mod.rs` (migration 17, `mod timelapse`)

**Interfaces:**
- Produces: `Camera::still(&self, quality: u8) -> Result<Still, String>` (blocking, bounded like `controls`; refused at once when not connected), `pub struct Still { pub jpeg: Vec<u8>, pub thumb: Vec<u8>, pub device: String, pub width: u32, pub height: u32, pub at: Timestamp }`. A still also becomes `latest()` (the live view shows it).
- Produces (store, `impl Store` in `store/timelapse.rs`):
  - `TimelapseRow { id, run_id: Option<i64>, kind: TimelapseKind /* Run | Standalone | Pictures */, name, folder: String, interval_s: i64, started_at: Timestamp, stop_at: Option<Timestamp>, ended_at: Option<Timestamp>, state: TimelapseState /* Recording | Paused | Ended */, pause_reason: Option<String> /* "user" | "disk" | "folder" */, anchor_at: Timestamp, anchor_slot: i64, missed: i64, images: i64, bytes: i64 }`
  - `ImageRow { id, timelapse_id, slot: Option<i64> /* None = taken by hand */, taken_at, elapsed_s: f64, file, thumb, bytes: i64, width, height, camera, controls_json, setpoint: Option<f64>, unit: Option<String>, delivered_ml: Option<f64>, weight_g: Option<f64>, trim_c: Option<f64> }`
  - `insert_timelapse(&self, t: &NewTimelapse) -> Result<i64>`, `timelapse(&self, id) -> Result<Option<TimelapseRow>>`, `open_timelapse(&self) -> Result<Option<TimelapseRow>>` (the one not ended, newest), `timelapse_of_run(&self, run_id) -> Result<Option<TimelapseRow>>`, `timelapses_of_run_ids(&self, ids: &[i64])`, `update_timelapse(&self, t: &TimelapseRow) -> Result<()>` (state, reason, anchor, interval, missed, ended_at, stop_at), `insert_image(&self, i: &NewImage) -> Result<i64>` (also adds to `images` and `bytes` of its timelapse, one transaction), `images(&self, timelapse_id, before_id: Option<i64>, limit: i64) -> Result<Vec<ImageRow>>` (newest first), `image(&self, id) -> Result<Option<ImageRow>>`, `delete_timelapse(&self, id) -> Result<()>` (rows only), `all_timelapses(&self) -> Result<Vec<TimelapseRow>>`, `pictures_of_day(&self, day: &str) -> Result<Option<TimelapseRow>>` (the `Pictures` bucket named "Pictures <day>").
- The migration's columns are exactly those fields (timestamps as RFC 3339 text like the other tables), `CREATE INDEX timelapse_images_by_timelapse ON timelapse_images(timelapse_id, id)`.

- [ ] **Step 1: Tests.** camera.rs: `still_is_refused_without_a_camera`, `still_returns_both_pictures_and_becomes_the_latest` (fake helper answers `Still` with fixed bytes). store: `migrate_creates_schema_at_the_latest_version` updated to 17; `timelapse_rows_round_trip`, `insert_image_counts_images_and_bytes`, `open_timelapse_ignores_ended_ones`, `deleting_a_run_is_not_blocked_by_its_timelapse` (insert a run, a timelapse with its `run_id`, delete the run through the existing `Store` delete: succeeds).
- [ ] **Step 2:** Run them: they fail.
- [ ] **Step 3: Implement** `Cmd::Still(u8, Sender<Result<Still, String>>)` in the supervisor (a hung or dead helper is handled as in `control_request`), the fake's `Still` answer, the migration and the store functions.
- [ ] **Step 4:** `cargo test -p fermentool-core camera store` passes.
- [ ] **Step 5: Commit** `feat(camera): stills through the supervisor, timelapse tables (0017)`.

### Task 3: The schedule (pure)

**Files:**
- Create: `crates/fermentool-core/src/timelapse/schedule.rs` (and `timelapse/mod.rs` declaring it; `lib.rs`: `pub mod timelapse`)

**Interfaces:**
- Produces: `#[derive(Clone, Copy, Debug, PartialEq)] pub struct Schedule { pub anchor: Timestamp, pub anchor_slot: i64, pub interval_s: i64 }` with
  - `fn due_at(&self, slot: i64) -> Timestamp`
  - `fn next_slot(&self, after_slot: Option<i64>, now: Timestamp) -> i64`: the first slot after `after_slot` whose time is not before `now` minus `LATE_S` (2 s: a slot just passed is still taken), never below `anchor_slot`
  - `fn missed_between(&self, last_slot: Option<i64>, next: i64) -> i64`: slots strictly between
  - `fn with_interval(&self, interval_s: i64, after_slot: Option<i64>, now: Timestamp) -> Schedule`: the next slot under the old interval keeps its time and becomes the new anchor
- The service (Task 5) keeps `last_slot` in memory and in `anchor_slot`/`missed` on the row.

- [ ] **Step 1: Tests** (`schedule.rs`):
  - `slots_from_the_anchor`: anchor 10:00, 300 s: slot 3 due 10:15.
  - `next_slot_after_a_restart_skips_and_counts`: last slot 2, now 10:31: next is 7 (10:35), `missed_between(Some(2), 7) == 4`.
  - `a_slot_just_passed_is_still_taken`: now 10:15:01, last 2: next 3.
  - `clock_set_back_never_bursts`: last slot 10 (10:50), now 09:50: next is 11, never a slot ≤ 10, `missed_between` 0.
  - `interval_change_applies_after_the_next_image`: 300 s, last 2, now 10:12; `with_interval(60, Some(2), now)` has anchor 10:15 (slot 3's time) and anchor_slot 3; its slot 4 is due 10:16.
- [ ] **Step 2:** Run: fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4:** Pass.
- [ ] **Step 5: Commit** `feat(timelapse): the schedule`.

### Task 4: Captures on disk

**Files:**
- Create: `crates/fermentool-core/src/timelapse/files.rs`
- Modify: `crates/fermentool-core/Cargo.toml` (`windows-sys` feature `Win32_Storage_FileSystem`)

**Interfaces:**
- Produces:
  - `pub fn captures_root(config: &CameraConfig, data_dir: &Path) -> PathBuf` (`captures_dir`, or `data_dir/captures` when empty)
  - `pub fn folder_name(started_local: &str /* "2026-11-05_1430" */, name: &str, id: i64) -> String`: `"<started> <name> (<id>)"`, characters `<>:"/\|?*` and control characters replaced by `_`, trimmed, the name cut to 60 characters
  - `pub fn image_name(slot: Option<i64>, manual_n: i64, elapsed_s: f64) -> String`: `"000123_+012h30m00s.jpg"`, by hand `"m0004_+012h31m10s.jpg"`
  - `pub fn ensure_writable(dir: &Path) -> Result<(), String>`: creates it, writes and deletes a probe file; the error says "This folder cannot be written to" plus the OS reason
  - `pub fn free_bytes(dir: &Path) -> Option<u64>` (`GetDiskFreeSpaceExW`, `None` elsewhere or on error)
  - `pub fn write_image(folder: &Path, name: &str, jpeg: &[u8], thumb: &[u8]) -> io::Result<()>`: `name` and `thumbs/name`, each through `<name>.tmp` then rename; a failure removes the temporary files
  - `pub fn append_csv(folder: &Path, row: &CsvRow) -> io::Result<()>`: `images.csv`, header written when the file is new: `file,taken_at,elapsed_s,manual,camera,width,height,setpoint,unit,delivered_ml,weight_g,trim_c,controls` (controls as a JSON string, CSV-quoted)
  - `pub fn remove_folder(dir: &Path) -> io::Result<()>` (missing is fine)
  - `pub const WARN_FREE_BYTES: u64 = 1_000_000_000; pub const PAUSE_FREE_BYTES: u64 = 200_000_000;`

- [ ] **Step 1: Tests** in a temp dir: `folder_names_are_safe_on_windows` (`"Run 2/3: E. coli?"` gives `"2026-11-05_1430 Run 2_3_ E. coli_ (7)"`), `image_names_sort_by_slot` (`image_name(Some(123), 0, 45000.0) == "000123_+012h30m00s.jpg"`), `write_image_leaves_no_tmp_on_failure` (target folder is a file: error, no `.tmp` left), `csv_header_once_and_quotes_json`, `ensure_writable_refuses_a_file_path`, `free_bytes_of_the_temp_dir_is_some` (Windows only).
- [ ] **Step 2:** Run: fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4:** Pass.
- [ ] **Step 5: Commit** `feat(timelapse): captures folder, files and images.csv`.

### Task 5: The timelapse service: lifecycle, run link, stills

**Files:**
- Modify: `crates/fermentool-core/src/timelapse/mod.rs`
- Modify: `crates/fermentool-core/src/engine/mod.rs` (`ActiveStatus::elapsed_s`)
- Modify: `crates/fermentool-core/src/main.rs` (spawn it), `crates/fermentool-core/src/api.rs` (`AppState::timelapse`)

**Interfaces:**
- Consumes: `Schedule` (Task 3), `files::*` (Task 4), `Store` timelapse functions and `Camera::still` (Task 2), `Store::log_event`.
- Produces:
  - `ActiveStatus.elapsed_s: f64`: the run's elapsed time at its last journal tick (the engine's `vol_elapsed_s`, from the guarded clock).
  - `pub trait StillSource: Send + 'static { fn still(&self, quality: u8) -> Result<Still, String>; fn controls(&self) -> Result<Vec<ControlInfo>, String>; fn connected(&self) -> bool; }`, implemented for `Camera`; tests use a fake.
  - `pub struct Timelapses` (cheap clone) with `spawn(store: Store, camera: impl StillSource, status: broadcast::Receiver<DaemonStatus>, root: impl Fn() -> PathBuf + Send + 'static) -> Timelapses`, and blocking calls answered by the thread: `start_with_run(run_id: i64, name: String, interval_s: i64) -> Result<i64, String>`, `start_standalone(name: String, interval_s: i64, stop_after_s: Option<i64>) -> Result<i64, String>` (refused while a run is active or one records), `stop() -> Result<(), String>` (standalone only), `pause() / resume() -> Result<(), String>`, `set_interval(interval_s: i64) -> Result<(), String>`, `take_picture() -> Result<i64 /* image id */, String>`, `delete_for_run(run_id: i64) -> Result<(), String>`, `delete_all() -> Result<(), String>` (refused while one records), `current() -> Option<CurrentTimelapse>` (read from shared state, never waits).
  - `#[derive(Serialize)] pub struct CurrentTimelapse { pub id: i64, pub kind, pub run_id: Option<i64>, pub name, pub state /* "recording" | "paused" | "waiting_camera" | "awaiting_resume" */, pub pause_reason: Option<String>, pub interval_s, pub images, pub missed, pub bytes, pub next_at: Option<Timestamp>, pub ends_at: Option<Timestamp>, pub started_at, pub free_bytes: Option<u64>, pub folder_ok: bool }`
  - Event kinds on the run (when a run owns the timelapse): `timelapse_start`, `timelapse_end`, `camera_lost`, `camera_recovered`, `timelapse_paused` (detail: reason), `timelapse_resumed`.

Behaviour (spec "Timelapse lifecycle"; one `timelapse` thread, a loop that waits on its command channel with a timeout equal to the next due action, like `camera-supervisor`):
- Run-linked: `start_with_run` creates the row (anchor = the run's `started_at`, folder from `root()` at that moment), ending any standalone one first (its row `Ended`, `timelapse_end` on nothing). From each status: the run's `active.run_id` equal to the row's → driving, take due stills; `active` none and `has_pending_recovery` → `awaiting_resume`, slots skipped, not missed, and the schedule continues after resume from the next due slot; `active` none and no recovery pending, or another run id → end the timelapse (`ended_at` now). Calibration runs never get one (the API refuses).
- At thread start: an open standalone row resumes at the next due slot, the slots passed while the daemon was down counted missed; an open run row waits for the status to say what the run is.
- Each still: `camera.still(STILL_QUALITY)` and `camera.controls()` (a refusal of the latter keeps `"[]"`), then `files::write_image`, `append_csv`, `insert_image` with the figures from the last status (`last_target`, `control_var`, `tracking.delivered_ml` or `volume_added_ml`, `scale_weight_g`, `trim_c`) and elapsed = `active.elapsed_s` plus the seconds since that status arrived (standalone: seconds since its start).
- `take_picture`: into the open timelapse with `slot` None; with none, into today's `Pictures <YYYY-MM-DD>` bucket (created, `interval_s` 0, never scheduled, kept `Recording` for the day).
- Standalone with `stop_at`: ends at it.

- [ ] **Step 1: Tests** (fake `StillSource`, fake status sender, a temp folder, an in-memory `Store`, short interval like 1 s):
  - `standalone_takes_images_on_schedule_and_resumes_after_a_restart` (3 images, drop the service, 2.5 s later a new one on the same store: missed 2, next image on the next slot).
  - `run_linked_starts_with_the_run_and_ends_with_it` (status with run 7 active → images; status with `active: None`, no recovery → `ended_at` set, `timelapse_end` event on run 7).
  - `awaiting_resume_pauses_without_missing` (status `active: None`, `has_pending_recovery: true` for 3 slots: no image, missed 0; active again: images resume).
  - `run_end_during_a_still_keeps_it_and_ends` (the fake still blocks until released; the run-ended status arrives meanwhile; after release: one image in the row, `ended_at` after its `taken_at`, no image after).
  - `a_run_ends_a_standalone_timelapse` (standalone recording, `start_with_run` → the standalone row `Ended` with its images, the run's row recording).
  - `each_image_keeps_the_runs_figures_and_the_camera_controls` (row and `images.csv` carry setpoint, unit, delivered, weight, trim c, controls JSON, elapsed ≈ status elapsed).
  - `take_picture_without_a_timelapse_goes_to_the_days_bucket`.
  - `pause_is_not_missed_and_interval_change_keeps_the_next_image` (through the service).
  - `notify.rs`: `timelapse_events_are_never_notified` (each new kind gives no message, no page).
- [ ] **Step 2:** Run: fail.
- [ ] **Step 3: Implement.** Spawn in `main.rs` after the camera and the control loop (`events.subscribe()`), `root` reading `[camera] captures_dir` from the shared config at call time. Writes to its own `Store` (opened like the notifier's), a single statement or one short transaction each.
- [ ] **Step 4:** `cargo test -p fermentool-core timelapse` passes.
- [ ] **Step 5: Commit** `feat(timelapse): the service, run-linked and standalone`.

### Task 6: Camera loss, disk and folder trouble

**Files:**
- Modify: `crates/fermentool-core/src/timelapse/mod.rs`

**Interfaces:**
- Consumes/extends Task 5's service and `CurrentTimelapse.state`.

Behaviour (spec "Camera loss", "Storage"):
- Camera not connected or a still refused: state `waiting_camera`, the slot counted missed, `camera_lost` once (run-linked); the next still that works: `camera_recovered`, back to `recording`.
- Before each still: `free_bytes < PAUSE_FREE_BYTES` → pause reason `disk`; the folder missing or `ensure_writable` failing, or `write_image` failing → pause reason `folder`. Both journalled once (`timelapse_paused`), checked again every 30 s (`TROUBLE_RECHECK`), resumed by themselves (`timelapse_resumed`), the slots in between not missed. A user pause is never resumed by itself.

- [ ] **Step 1: Tests:** `camera_lost_counts_missed_and_recovers` (fake not connected for 2 slots, then back: missed 2, one `camera_lost`, one `camera_recovered`); `folder_gone_pauses_then_resumes` (remove the timelapse's folder's parent mid-run: state paused `folder`, no file in a recreated folder until it is back, no `.tmp` left, resumes alone; `TROUBLE_RECHECK` shortened in tests through the service's timings); `low_disk_pauses` (an injected `free_bytes` below the threshold).
- [ ] **Step 2:** Run: fail.
- [ ] **Step 3: Implement** (`free_bytes` behind a function pointer in the service's timings so the test can inject it).
- [ ] **Step 4:** Pass.
- [ ] **Step 5: Commit** `feat(timelapse): camera loss, full disk and missing folder`.

### Task 7: Config and API

**Files:**
- Modify: `crates/fermentool-core/src/config.rs` (`interval_min`, `captures_dir`), `crates/fermentool-core/src/api.rs`

**Interfaces:**
- Consumes: `Timelapses` (Task 5), store reads through `ApiDb`.
- Produces, routes (JSON unless said):
  - `GET /api/timelapses/current` → `CurrentTimelapse` or `null`, plus `warn_free: bool` (free under `WARN_FREE_BYTES`).
  - `POST /api/timelapses` `{name, interval_min, stop_after_h?}` → `{id}` (standalone; 409 while a run is active or one records; name required, interval 1 to 1440, stop_after 0.1 to 1000 h).
  - `POST /api/timelapses/current/stop`, `/pause`, `/resume`, `/interval {interval_min}`.
  - `POST /api/camera/picture` → `{image_id}`.
  - `GET /api/timelapses/{id}/images?before=&limit=` (newest first, limit 1 to 200, default 8) → rows without file paths: `{id, slot, taken_at, elapsed_s, bytes}`.
  - `GET /api/timelapses/images/{image_id}.jpg` and `/thumb.jpg` → the file (`image/jpeg`, `Cache-Control: max-age=31536000, immutable`), 404 when the row or the file is gone. Path from the row only.
  - `GET /api/camera/captures` → `{folder, used_bytes, free_bytes}` (used = sum of `bytes` of every timelapse in that folder).
  - `POST /api/camera/captures {folder}` → checks `ensure_writable` then saves `[camera] captures_dir` (409 "This folder cannot be written to" with the reason); `POST /api/camera/interval {interval_min}` saves the default.
  - `POST /api/runs` accepts `timelapse: {interval_min} | null` beside `RunConfig` (`#[serde(flatten)]`): after the run starts, `start_with_run`; a refusal there never fails the run start (logged, the response carries `timelapse_error`). Refused for a calibration run (ignored, no timelapse).
  - `GET /api/runs/{id}` adds `timelapse: {id, interval_s, images} | null`.
  - `DELETE /api/runs/{id}` then `delete_for_run`; `DELETE /api/history` then `delete_all` (both after the control thread said yes; a failure to remove files is logged, the rows are gone).

- [ ] **Step 1: Tests** (router with a fake-backed `Timelapses`): `timelapse_routes_start_stop_and_list_images`, `image_files_are_served_by_id_only` (a request with `..` in the id is a 404/400, never a file outside), `run_start_with_a_timelapse_links_it` and `a_timelapse_failure_never_fails_the_run_start`, `deleting_a_run_deletes_its_images`, `captures_folder_refuses_an_unwritable_path`, config: `camera_interval_and_captures_dir_round_trip`, `interval_out_of_range_is_refused_then_fixed`.
- [ ] **Step 2:** Run: fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4:** `cargo test` passes (whole workspace).
- [ ] **Step 5: Commit** `feat(timelapse): routes, settings, run start and delete`.

### Task 8: UI logic (`ui/src/lib/timelapse.js`)

**Files:**
- Create: `ui/src/lib/timelapse.js`, `ui/src/lib/timelapse.test.js`

**Interfaces:**
- Produces:
  - `BYTES_PER_IMAGE = { '640x480': 90_000, '1280x720': 250_000, '1920x1080': 550_000 }` (replace with Task 1's measured sizes at quality 95)
  - `estimate(durationS, intervalMin, width, height, avgBytes = null) -> { images, bytes, perDay }` (images = `floor(durationS / (intervalMin*60)) + 1`; `avgBytes` from real images when known)
  - `estimateText(e) -> "about 1 200 images, 600 MB"` (rounded to 2 significant figures, thin space thousands like `fmt.js`)
  - `cardState(cur, cam) -> 'none' | 'recording_run' | 'recording' | 'waiting' | 'paused' | 'awaiting'` from `GET /api/timelapses/current` and `GET /api/camera`
  - `nextInText(nextAt, nowMs) -> "Next image in 3 m 20 s"`, `bytesText(n) -> "1.2 GB"`, `freeWarning(cur) -> string | null` ("Under 1 GB free on this disk" / "Paused: under 200 MB free" / "Paused: the folder cannot be written to")
  - `pauseReasonText(reason)`
- [ ] **Step 1: Tests** for each function with the spec's figures (100 h at 5 min at 1280 × 720 gives 1201 images; `cardState` for each state; free warnings at 999 MB and 150 MB).
- [ ] **Step 2:** `npm test`: fail. **Step 3:** implement. **Step 4:** pass.
- [ ] **Step 5: Commit** `feat(ui): timelapse logic`.

### Task 9: Settings > Camera fields

**Files:**
- Modify: `ui/src/routes/settings/CameraSettings.svelte`, `ui/tests/smoke.mjs`

Under the live view and Picture, a **Timelapse** block: **Every (min)** (number field, applied on Enter or leaving the field, `POST /api/camera/interval`, said in the status line with Undo like the other changes) and **Images kept in** (path field applied the same way through `POST /api/camera/captures`; refused: the field orange, the message "This folder cannot be written to" and the reason under it; under the field "X used here, Y free on this disk" from `GET /api/camera/captures`). No Browse… yet (plan D).

- [ ] **Step 1:** Implement; smoke: the two fields exist with their saved values, nothing typed.
- [ ] **Step 2:** `npm run check`, `npm test`, `npm run build`.
- [ ] **Step 3: Commit** `feat(ui): timelapse interval and captures folder in Settings`.

### Task 10: New run, Check and start

**Files:**
- Modify: `ui/src/routes/NewRun.svelte`, `ui/src/routes/History.svelte` (Run again seed), `ui/src/lib/runfile.js` if the settings file carries it, `ui/tests/smoke.mjs`

In Check and start, a **Timelapse with this run** row in the recap: a switch (on by default once a camera is configured and enabled, off otherwise and disabled with "No camera: Settings, Camera" link), **Every (min)** (default from `[camera] interval_min`), the estimate from `estimate(durationS, …)` ("about 1 200 images, 600 MB over 100 h"), and while a standalone timelapse records, "Ends the timelapse <name>". Under the curve preview, the camera's live picture (`LiveImg`, small) to check the framing. `start()` sends `timelapse: on ? { interval_min } : null`. Run again: the run's `timelapse` (from `GET /api/runs/{id}`) seeds the switch and the interval. A `timelapse_error` in the start answer is shown on the Overview as a one-line note (not an error band).

- [ ] **Step 1:** Implement; smoke: the switch exists in Check and start with an estimate when a curve is valid, nothing started.
- [ ] **Step 2:** `npm run check`, `npm test`, `npm run build`.
- [ ] **Step 3: Commit** `feat(ui): timelapse with the run in New run`.

### Task 11: Camera page, Timelapse card, Take a picture, latest images

**Files:**
- Modify: `ui/src/routes/Camera.svelte`, `ui/tests/smoke.mjs`
- Create: `ui/src/components/TimelapseCard.svelte`

As the mockup's Camera page: beside the picture, the **Timelapse** card (poll `GET /api/timelapses/current` every 2 s with the camera):
- `recording_run`: "Started with run N, stops with it" (link to the run in History), Every, Images, Next image in, On disk; **Change interval** (inline number field), **Pause** / **Resume**.
- `waiting`: orange band "Camera not answering", "The run is not affected", "N images missed so far".
- `paused` by disk or folder: orange band with `freeWarning`.
- `none`: the form Name, Every (min), Stop after (h, optional), the estimate per day ("288 images, 70 MB a day"), **Start timelapse** (disabled while a run is active, with "A run is going: switch the timelapse on in New run").
- `recording` (standalone): name, Every, Images, Next image in, Ends; **Stop timelapse**.
"Recording" in green ink, no red dot. Under the picture: **Take a picture** (the new image's thumbnail flashes into the strip) beside Full screen. **Latest images**: the last 8 thumbnails of the current timelapse with their run time (`runClock`), refreshed when the image count changes.

- [ ] **Step 1:** Implement; smoke: the card answers in one of its states, Take a picture and Start timelapse exist, nothing clicked.
- [ ] **Step 2:** `npm run check`, `npm test`, `npm run build`.
- [ ] **Step 3: Commit** `feat(ui): timelapse card and latest images on the Camera page`.

### Task 12: Deploy, bench checks, documentation

- [ ] **Step 1:** Re-check `/api/status` (no active run), shut down, build release helper and daemon, start, `npm run smoke`, no headless Edge left.
- [ ] **Step 2: Bench, with the user's go** (simulator pump if `allow_simulator`, else ask): a standalone timelapse at 1 min for 10 min (10 or 11 images, `images.csv` complete, thumbnails present); a run-linked one on a short simulator run, Stop: ended with the run; unplug and replug the webcam mid-timelapse: missed counted, resumes, the run untouched; kill and restart the daemon mid-run: resumes on schedule; Take a picture with nothing recording: today's Pictures bucket.
- [ ] **Step 3:** `CLAUDE.md` Camera section (the timelapse thread, the status-driven run link, the AI columns, thresholds, migration 0017), `CHANGELOG.md` (French, user-facing), session notes.
- [ ] **Step 4: Commit** `docs: timelapses` and the UI bundle commit.
- [ ] **Step 5:** The timing gate of plan B (two 6 h runs, now with a timelapse at 1 min) is run before merging `feat/camera`; ask the user for the bench time.
