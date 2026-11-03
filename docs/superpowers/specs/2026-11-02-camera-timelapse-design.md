# Camera: live view and timelapse

Date: 2026-11-02. Status: design approved by the user (2026-11-02), next
step the implementation plan.

Mockup (approved by the user, "j'aime trop"):
https://claude.ai/artifact/PQq3jCwc41jTj9vYyPyyo7. It has six artboards:
the Camera page, Settings > Camera, New run (Check and start), History run
detail, the shared timelapse player, and a placeholder camera image. The
mockup is the reference for layout and wording; this document is the
reference for behaviour.

## Why

The user wants to see the reactor from the app: a live view of the vessel
during a run, and a **timelapse** kept with the run so foam, colour and
level can be watched again over a 100 h fed-batch, played back like a
video and exported. The original plan already reserved a *Camera* page
(`docs/IMPLEMENTATION_PLAN.md` §5, §10).

## Decisions (the user, 2026-11-02)

- **USB webcam on the bench PC** for now (no IP camera).
- **Camera is its own entry in the left rail**, under Feeding profile, not
  a tab of the pump views.
- **Settings > Camera** lists the cameras this PC has and lets the user
  pick one.
- **Images are kept**: a timelapse at a set interval.
- **A timelapse is linked to a run** (switched on, interval set, in New
  run's Check and start step; starts and stops with the run), **and can
  also be recorded on its own**, without a run, from the Camera page.
- **The timestamp is never burned into the stored image.** It is drawn
  over the picture when viewing, with a choice: none, run time, date and
  time, both. **A video export can carry the timestamp or not**, same
  choice.
- **Playback like a video**: a player (play/pause, scrub bar, speeds, full
  screen) on the Camera page ("Live / Timelapse so far") and in History.
- The app window stays open during an experiment, but capture still runs
  in the daemon (below).

## Constraints (hard)

1. **The camera must never delay a pump write, a balance read or a journal
   tick** (requirement set 2026-10-07). No camera call on the control
   thread, ever. Capture runs on its own thread(s) at below-normal
   priority. A camera that wedges in a driver call blocks only its own
   worker, which is retired and replaced, never joined (same rule as
   `WatchdogTransport` and `PolledScale`, `CLAUDE.md` "Serial I/O").
2. **Capture lives in the daemon, not in the page.** Reasons, given to the
   user: a minimised WebView2 window gets its timers throttled; a WebView
   reload or crash would stop the timelapse silently; the daemon already
   survives the window and resumes runs after a crash. Also, a webcam
   opens in one application at a time on Windows, so the page shows the
   daemon's frames rather than opening the camera itself.
   **Capture runs in a helper process** (`fermentool-camera.exe`), spawned
   and supervised by the daemon at below-normal priority, not in threads
   of `fermentool-core.exe`. *(My call: a wedged driver call is handled
   by retiring a thread, but a faulting webcam driver, an access
   violation in Media Foundation or a vendor DLL, would kill the daemon
   and the run with it. In its own process it kills only the camera.)*
   The daemon restarts the helper when it dies; the helper is statically
   linked like the sidecar (`+crt-static`), goes through
   `copy-sidecar.ps1` and the bundle, and is stopped by the NSIS hook and
   by `POST /api/shutdown`. The daemon owns the schedule and the database;
   the helper only lists devices, opens one, and hands back frames.
3. **Images live in a folder the user chooses** (default
   `%APPDATA%\Fermentool\captures\`), never in the exe or the installer.
   A fresh install starts with none, like the history.
4. **No new install on the target PC.** Whatever encodes video or reads
   the camera ships inside the exe/bundle or is part of Windows 10/11.
5. A camera problem is **never a run problem**: it never stops, pauses or
   alarms a run.
6. House rules apply: `docs/DESIGN.md` (state colours, Plex Sans, no
   em dash, no spaced hyphen, "Say each figure once"), the UI check list in
   `CLAUDE.md`, and **a bug fixed is a test added**.

## What the user sees

### Camera page (rail entry "Camera", route `camera`)

- The run state bar on top, as on every page other than Overview.
- A big 16:9 picture. A **Live / Timelapse so far** choice above it (only
  once the current timelapse has images).
  - *Live*: the daemon's frames, a "Live" tag and the run time over the
    picture; under it the camera name and picture size, **Take a picture**
    and **Full screen**.
  - *Timelapse so far*: the shared player on the current timelapse's
    images.
- **Latest images**: the last 8 thumbnails with their run time, and a link
  to the run in History.
- A **Timelapse** card on the side, in one of four states (mockup's
  "Mockup state" buttons):
  - *Recording with a run*: "Started with run N, stops with it", Every,
    Images, Next image in, On disk; **Change interval**, **Pause**.
  - *Waiting for the camera* (camera lost mid-timelapse): orange band at
    the top, "Camera not answering", "The run is not affected"; the card
    says images missed so far.
  - *No run, nothing recording*: a form (Name, Every (min), Stop after (h),
    optional) with the images and MB per day it will make, and **Start
    timelapse**.
  - *Recording on its own*: name, Every, Images, Next image in, Ends;
    **Stop timelapse**.
- "Recording" is quiet green ink, never a red REC dot (red means something
  went wrong).

### Settings > Camera (group Hardware, after Balance)

- `LinkHeader` band: Connected / Camera in use (another program holds it,
  orange) / Not connected; Connect, or Disconnect as on the other links.
- **Camera**: a list of the PC's video capture devices, with "Look again"
  after plugging one in. **Picture size**: 640 × 480, 1280 × 720,
  1920 × 1080 (only the sizes the chosen camera offers). A small "what this
  camera sees now" preview.
- **Timelapse, every (min)**: the default a new run starts with.
- **Images kept in**: **the user chooses the folder** (decided
  2026-11-02): a path field and **Browse…** (a native folder picker in the
  desktop window; in a plain browser tab the field alone), default
  `%APPDATA%\Fermentool\captures`. Under it: space used there and free on
  that disk. The daemon checks the folder can be written to (creating it
  if missing) before Save; one it cannot write is refused with an orange
  field and "This folder cannot be written to".
- Save disabled until something changes (Settings rule).

### New run, Check and start

- A **Timelapse with this run** switch (on by default once a camera is
  configured), **Every (min)** (from Settings), the estimate ("about 1 200
  images, 600 MB over 100 h"), and the camera's current picture to check
  the framing before Start.
- Run again keeps the run's timelapse choice and interval.

### History run detail

- A **Timelapse** section: "N images, every X min", **Download images
  (.zip)**, **Export video (.mp4)**.
- The shared player, a **Time on the image** choice (None, Run time, Date
  and time, Both), and beside it the figures at the image shown: run time,
  feed rate asked, fed so far.
- 13 thumbnails across the run (every 1/12 of it); a click jumps there.
- The **Feed rate** chart carries a cursor at the image shown, which moves
  during playback; a click on the chart jumps to the image of that moment.
- **Export video**: a panel with a frame preview, **Time on the video**
  (same four choices, starting from the viewer's), **Length** (30 s,
  1 min, 3 min), the image count, fps and size, **Export**.

### Past timelapses (decided 2026-11-02, "on part là-dessus")

Every timelapse, run-linked or not, needs a place to be replayed,
exported and deleted once over; History lists runs only. As on the
mockup (artboard "Camera > Timelapses"): the Camera page has two
tabs, **Live** (everything under "Camera page" above) and **Timelapses**. Timelapses lists them all, newest
first, filter All / With a run / On their own, a thumbnail, name, when,
images and interval per row; the one picked opens in the player, with
.zip and .mp4 export, a link to its run in History when it has one, and a
"…" menu (Rename, Open the folder, Delete timelapse and its images). A
day's hand-taken pictures appear as their own entry. A run's timelapse
shows in both places (here and in its History run detail); deleting it
here deletes its images only, never the run.

### The player (shared)

Play/pause (a big ▶ over the paused picture), a scrub bar, "position /
total" in run time, speeds Slow / Normal / Fast with "whole run in ...",
full screen, the timestamp overlay. It can be driven by its parent (History
syncs the chart with it).

## Behaviour

### Timelapse lifecycle

- **One camera, one timelapse at a time.**
- **Run-linked**: created when a run starts with the switch on; first
  image at the run's start; continues through the hold phase; ends when
  the run ends (stopped, completed, aborted). Calibration runs never have
  one.
- **Standalone**: started from the Camera page when no run is going;
  ends on Stop, or after "Stop after (h)" if given.
- **A run started with its timelapse on while a standalone timelapse
  records ends the standalone one** (its images stay, under its own name)
  and starts the run's. New run says so next to the switch. *(My call: it
  covers "film the culture before the feed starts" without two writers on
  one camera.)*
- **A run-linked timelapse takes images only while its run is driven**
  (the user, 2026-11-02: "ça sert à rien de prendre des photos si l'exp
  est morte"). While the run awaits resume (crash resume off, the Resume
  dialog, the grace window), no image is taken and those slots are a
  pause, not missed. Resumed, the timelapse goes on at the next due slot;
  discarded, it ends with what it has. The run's engine state drives it.
- **A standalone timelapse survives a daemon restart**: it resumes by
  itself at the next due slot, from its row in the database. *(My call.)*
- **Schedule from elapsed time**: image k is due at
  `start + k * interval`. After a daemon restart or a crash-resume, the
  next image is the next due slot after now; slots missed while the daemon
  was down are counted as missed, not taken late in a burst.
- **Change interval** applies from the next image; **Pause** stops taking
  images until Resume (a paused slot is not "missed").
- **Take a picture** saves one image at once into the current timelapse,
  marked manual (outside the schedule); with no timelapse, into a
  standalone "Pictures" bucket for that day. *(My call.)*

### Camera loss

- A device gone or failing: the timelapse is *waiting for the camera*, the
  Camera page shows the orange band, slots that pass are counted missed;
  the daemon retries opening it (every few seconds) and resumes on its own.
- Journalled as run events `camera_lost` / `camera_recovered` when a run
  owns the timelapse, so they appear in the run's journal. **Not notified
  and not an alarm**. *(My call: constraint 5; can be added to
  notifications later.)*
- "Camera in use" (another program holds it) is shown as such in Settings
  and on the Camera page.

### Storage

- **A new folder applies to timelapses started after the change**; one
  already recording stays in the folder it started in. Each timelapse
  stores its own folder in the database, so a folder changed later never
  loses older images. *(My call.)*
- A folder that becomes unreachable mid-timelapse (a network share gone,
  a USB disk unplugged) is handled like a full disk: the timelapse pauses
  itself, the Camera page says why, it resumes when the folder is back;
  the run is untouched.
- One folder per timelapse under the chosen folder, JPEG files, plus a small
  thumbnail per image made at capture time (for the strip and smooth
  playback).
- Nothing is deleted automatically. **Deleting a run deletes its
  timelapse images; Reset history deletes all captures.** *(My call.)*
- Free space: under 1 GB free the Camera page warns; under 200 MB the
  timelapse pauses itself (journalled) rather than fill the disk the
  journal needs. *(My call.)*

### Timestamps

- Every image stores its clock time; run-linked ones also their run
  elapsed (from the guarded run clock, `run_now()`, never a raw wall-clock
  difference); standalone ones count from the timelapse start.
- Overlays (viewer, video export) use run time "+36 h 00 m" and/or
  "Tue 27 Oct, 21:12" (`fmt.js`, `en-GB`), in IBM Plex Sans.

## What is left to the builder

- **Capture library** on Windows (Media Foundation through the `windows`
  crate, `nokhwa`, or another), as long as constraints 1 and 4 hold and
  devices are listed by friendly name.
- **How the camera is remembered**: by friendly name in `config.toml`
  (`[camera] device`, `width`, `height`, `interval_min`), with a tie-break
  if two identical cameras are plugged in.
- **Live transport**: MJPEG over HTTP, frames over the existing WebSocket,
  or polled JPEGs. The daemon grabs at live rate only while a page shows
  the live view; otherwise only what the timelapse needs.
- **Video export**: the page (WebCodecs H.264 in WebView2 plus a bundled
  MP4 muxer, drawing the overlay on a canvas in Plex Sans) or the daemon
  (Media Foundation's H.264 encoder, with a bundled font for the overlay).
  Either must work offline with nothing installed; if H.264 is missing
  (Windows "N" editions), fall back to another format and say so.
  Recommendation: the page, since the overlay is then the same drawing as
  the viewer's.
- **Zip download**: streamed by the daemon.
- **Schema**: new tables for timelapses and their images. **Migration
  number: 0017** (0016 is taken on `feat/rate-change-reset`); check every
  branch and the bench DB's `pragma user_version` again before numbering
  (memory: migration numbers across branches).
- **API shape** (`/api/camera/...`, `/api/timelapses/...`), following
  `api.rs`: journal reads on `ApiDb`, only what needs the live camera
  through its worker.
- A **test pattern camera** for development and the smoke test on a PC
  without a webcam, off unless set in `config.toml` (like
  `allow_simulator`).
- Pure logic (schedule, missed slots, estimates, overlay text, frame index
  and time) in `ui/src/lib/*.js` and Rust functions with tests.

## Not now

Remote viewing (the o2switch relay, `remote-access-plan`), IP cameras,
several cameras at once, image analysis (foam detection), audio.

## Done means

- The checks in `CLAUDE.md` (cargo test, `npm run check`, `npm test`,
  build, deploy when no run is active, `npm run smoke` extended to the
  Camera page and Settings > Camera without starting anything).
- **Timing, measured.** The daemon records, per minute, the setpoint
  write interval, the balance reading age and the journal tick duration
  (p50, p99, max) in its log; this is the baseline deferred on
  2026-10-07, built first. Two 6 h runs on the bench, simulator pump
  (`allow_simulator`) with the real balance and the real webcam, one
  without the camera and one with a timelapse at 1 min plus the live view
  open for an hour: the p99 of each figure with the camera stays within
  10 % (or 20 ms, whichever is larger) of the run without. *(My call: two
  literal 100 h runs would block the bench for eight days.)*
- Unplug and replug the webcam mid-timelapse: the run is untouched, the
  timelapse resumes, missed images are counted.
- Kill and restart the daemon mid-run: the timelapse resumes on schedule.
- An installer built per `docs/RELEASE.md` records and exports on a clean
  Windows 10/11 PC with nothing else installed.
