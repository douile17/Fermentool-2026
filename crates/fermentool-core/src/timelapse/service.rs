//! The `timelapse` thread: one timelapse at a time, its schedule, its images.
//!
//! It owns its own `Store` connection and the captures folder, follows the
//! run through [`RunView`]s (fed from the status broadcast, never from the
//! control thread) and asks the camera supervisor for stills. Its handle,
//! [`Timelapses`], answers the API: commands wait on the thread, a few
//! seconds at most; [`Timelapses::current`] reads shared state and never
//! waits.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fermentool_camera::backend::STILL_QUALITY;
use fermentool_camera::protocol::ControlInfo;
use jiff::{SignedDuration, Timestamp, Zoned};
use serde::Serialize;

use super::files::{self, CsvRow};
use super::schedule::Schedule;
use crate::camera::{Camera, CameraState, Still};
use crate::control::DaemonStatus;
use crate::store::{
    EventLevel, NewEvent, NewImage, NewTimelapse, Store, TimelapseKind, TimelapseRow, TimelapseState,
};

/// What the timelapse needs from a camera; [`Camera`] in the daemon, a fake
/// in tests.
pub trait StillSource: Send + 'static {
    fn still(&self, quality: u8) -> Result<Still, String>;
    fn controls(&self) -> Result<Vec<ControlInfo>, String>;
    fn connected(&self) -> bool;
}

impl StillSource for Camera {
    fn still(&self, quality: u8) -> Result<Still, String> {
        Camera::still(self, quality)
    }
    fn controls(&self) -> Result<Vec<ControlInfo>, String> {
        Camera::controls(self)
    }
    fn connected(&self) -> bool {
        matches!(self.state(), CameraState::Connected { .. })
    }
}

/// The run as the timelapse sees it, from one status broadcast.
#[derive(Debug, Clone, Default)]
pub struct RunView {
    pub active: Option<RunNow>,
    /// A run waits for Resume (crash resume, the grace window).
    pub pending_recovery: bool,
}

/// The driven run and its figures of the moment (kept with each image).
#[derive(Debug, Clone, Default)]
pub struct RunNow {
    pub run_id: i64,
    pub elapsed_s: f64,
    pub setpoint: Option<f64>,
    /// `rpm` or `ml_min`.
    pub unit: Option<String>,
    pub delivered_ml: Option<f64>,
    pub weight_g: Option<f64>,
    pub trim_c: Option<f64>,
}

impl From<&DaemonStatus> for RunView {
    fn from(s: &DaemonStatus) -> Self {
        RunView {
            active: s.active.as_ref().map(|a| RunNow {
                run_id: a.run_id,
                elapsed_s: a.elapsed_s,
                setpoint: a.last_target,
                unit: serde_json::to_value(a.control_var).ok().and_then(|v| v.as_str().map(str::to_string)),
                delivered_ml: s.tracking.as_ref().map(|t| t.delivered_ml).or(a.volume_added_ml),
                weight_g: s.scale_weight_g,
                trim_c: s.trim_c,
            }),
            pending_recovery: s.has_pending_recovery,
        }
    }
}

/// Feeds `tl` a [`RunView`] of every status broadcast, on a tokio task (the
/// daemon's and the API tests').
pub fn follow_status(events: &tokio::sync::broadcast::Sender<DaemonStatus>, tl: &Timelapses) {
    let mut rx = events.subscribe();
    let sink = tl.status_sink();
    tokio::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match rx.recv().await {
                Ok(st) => {
                    if sink.send(RunView::from(&st)).is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
        }
    });
}

/// The thread's paces, shortened by tests.
#[derive(Debug, Clone, Copy)]
pub struct Timings {
    /// Between two looks at the schedule and the run.
    pub poll: Duration,
    /// A run neither driven nor awaiting resume this long has ended (a status
    /// queued from before the timelapse started must not end it).
    pub run_gone_grace: Duration,
    /// A paused-by-trouble timelapse (disk, folder) looks again this often.
    pub trouble_recheck: Duration,
    /// Free bytes on a folder's disk ([`files::free_bytes`]).
    pub free_bytes: fn(&Path) -> Option<u64>,
}

impl Default for Timings {
    fn default() -> Self {
        Timings {
            poll: Duration::from_millis(250),
            run_gone_grace: Duration::from_secs(3),
            trouble_recheck: Duration::from_secs(30),
            free_bytes: files::free_bytes,
        }
    }
}

/// The timelapse recording or paused, for the Camera page.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CurrentTimelapse {
    pub id: i64,
    pub kind: TimelapseKind,
    pub run_id: Option<i64>,
    pub name: String,
    /// `recording`, `paused`, `waiting_camera` or `awaiting_resume`.
    pub state: &'static str,
    /// `user`, `disk` or `folder` while paused.
    pub pause_reason: Option<String>,
    pub interval_s: i64,
    pub images: i64,
    pub missed: i64,
    pub bytes: i64,
    pub next_at: Option<Timestamp>,
    pub ends_at: Option<Timestamp>,
    pub started_at: Timestamp,
    pub free_bytes: Option<u64>,
    pub folder: String,
}

type Reply<T> = mpsc::Sender<Result<T, String>>;

enum Cmd {
    StartRun { run_id: i64, name: String, interval_s: i64, reply: Reply<i64> },
    StartStandalone { name: String, interval_s: i64, stop_after_s: Option<i64>, reply: Reply<i64> },
    Stop(Reply<()>),
    Pause(Reply<()>),
    Resume(Reply<()>),
    SetInterval(i64, Reply<()>),
    TakePicture(Reply<i64>),
    DeleteForRun(i64, Reply<()>),
    DeleteAll(Reply<()>),
    Shutdown,
}

/// A command's answer is waited for this long at most (a still is a few
/// seconds; the camera's own bound is 5 s plus an open's 10 s).
const ANSWER_WITHIN: Duration = Duration::from_secs(20);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Handle on the `timelapse` thread. Cheap to clone.
#[derive(Clone)]
pub struct Timelapses {
    tx: mpsc::Sender<Cmd>,
    status: mpsc::Sender<RunView>,
    current: Arc<Mutex<Option<CurrentTimelapse>>>,
    join: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl Timelapses {
    pub fn spawn(store: Store, camera: impl StillSource, root: impl Fn() -> PathBuf + Send + 'static) -> Timelapses {
        Self::spawn_with(store, camera, root, Timings::default())
    }

    pub fn spawn_with(
        store: Store,
        camera: impl StillSource,
        root: impl Fn() -> PathBuf + Send + 'static,
        t: Timings,
    ) -> Timelapses {
        let (tx, rx) = mpsc::channel();
        let (status, status_rx) = mpsc::channel();
        let current = Arc::new(Mutex::new(None));
        let mut svc = Svc {
            store,
            cam: camera,
            root: Box::new(root),
            t,
            status_rx,
            run: None,
            live: None,
            current: Arc::clone(&current),
        };
        let join = std::thread::Builder::new()
            .name("timelapse".into())
            .spawn(move || svc.run(rx))
            .map_err(|e| tracing::error!("cannot start the timelapse thread: {e}"))
            .ok();
        Timelapses { tx, status, current, join: Arc::new(Mutex::new(join)) }
    }

    /// Where the run's statuses go (`RunView::from` each broadcast).
    pub fn status_sink(&self) -> mpsc::Sender<RunView> {
        self.status.clone()
    }

    /// The timelapse recording or paused, `None` without one. Never waits.
    pub fn current(&self) -> Option<CurrentTimelapse> {
        lock(&self.current).clone()
    }

    fn ask<T>(&self, cmd: impl FnOnce(Reply<T>) -> Cmd) -> Result<T, String> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(cmd(tx)).map_err(|_| "the timelapse thread has stopped".to_string())?;
        rx.recv_timeout(ANSWER_WITHIN).unwrap_or_else(|_| Err("the timelapse did not answer in time".to_string()))
    }

    /// The run's timelapse, from now on; ends a standalone one first.
    pub fn start_with_run(&self, run_id: i64, name: String, interval_s: i64) -> Result<i64, String> {
        self.ask(|reply| Cmd::StartRun { run_id, name, interval_s, reply })
    }

    /// A timelapse with no run; refused while a run is going or one records.
    pub fn start_standalone(&self, name: String, interval_s: i64, stop_after_s: Option<i64>) -> Result<i64, String> {
        self.ask(|reply| Cmd::StartStandalone { name, interval_s, stop_after_s, reply })
    }

    /// Ends the standalone timelapse (a run's stops with its run).
    pub fn stop(&self) -> Result<(), String> {
        self.ask(Cmd::Stop)
    }

    pub fn pause(&self) -> Result<(), String> {
        self.ask(Cmd::Pause)
    }

    pub fn resume(&self) -> Result<(), String> {
        self.ask(Cmd::Resume)
    }

    /// From the next image on; the one already due keeps its time.
    pub fn set_interval(&self, interval_s: i64) -> Result<(), String> {
        self.ask(|r| Cmd::SetInterval(interval_s, r))
    }

    /// One image now, into the current timelapse or today's pictures.
    pub fn take_picture(&self) -> Result<i64, String> {
        self.ask(Cmd::TakePicture)
    }

    /// The run's timelapses, rows and folders.
    pub fn delete_for_run(&self, run_id: i64) -> Result<(), String> {
        self.ask(|r| Cmd::DeleteForRun(run_id, r))
    }

    /// Every timelapse but the one recording, rows and folders.
    pub fn delete_all(&self) -> Result<(), String> {
        self.ask(Cmd::DeleteAll)
    }

    /// Ends the thread (a timelapse recording stays open in the database and
    /// resumes at the next start).
    pub fn shutdown(&self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(j) = lock(&self.join).take() {
            let _ = j.join();
        }
    }
}

/// The open timelapse, as the thread follows it.
struct Live {
    row: TimelapseRow,
    /// Its run waits for Resume: no image, the slots are not missed.
    awaiting: bool,
    /// Since when its run is neither driven nor awaiting resume.
    gone_since: Option<Instant>,
    /// Its run has been seen driven since this thread took it on: a status
    /// without it now is the run's end, not one from before its start.
    run_seen: bool,
    /// The last still failed: waiting for the camera.
    camera_lost: bool,
    /// Paused for the disk or the folder: when to look again.
    recheck_at: Option<Instant>,
}

impl Live {
    fn schedule(&self) -> Schedule {
        Schedule { anchor: self.row.anchor_at, anchor_slot: self.row.anchor_slot, interval_s: self.row.interval_s }
    }

    /// The slots up to now are passed without being missed (a pause, a wait
    /// for Resume).
    fn skip_to_now(&mut self, now: Timestamp) {
        let next = self.schedule().next_slot(self.row.last_slot, now);
        if next > self.row.anchor_slot && self.row.last_slot.is_none_or(|l| next - 1 > l) {
            self.row.last_slot = Some(next - 1);
        }
    }

    fn next_at(&self, now: Timestamp) -> Timestamp {
        let s = self.schedule();
        s.due_at(s.next_slot(self.row.last_slot, now))
    }
}

/// Why an image was not kept.
enum CaptureError {
    Camera(String),
    Folder(String),
}

impl CaptureError {
    fn message(self) -> String {
        match self {
            CaptureError::Camera(m) | CaptureError::Folder(m) => m,
        }
    }
}

struct Svc<S: StillSource> {
    store: Store,
    cam: S,
    root: Box<dyn Fn() -> PathBuf + Send>,
    t: Timings,
    status_rx: mpsc::Receiver<RunView>,
    /// The last run status and when it came.
    run: Option<(RunView, Instant)>,
    live: Option<Live>,
    current: Arc<Mutex<Option<CurrentTimelapse>>>,
}

fn local_stamp(format: &str) -> String {
    Zoned::now().strftime(format).to_string()
}

impl<S: StillSource> Svc<S> {
    fn run(&mut self, rx: mpsc::Receiver<Cmd>) {
        match self.store.open_timelapse() {
            Ok(Some(row)) => {
                tracing::info!(id = row.id, name = %row.name, "timelapse resumes after a restart");
                self.live = Some(Live { row, awaiting: false, gone_since: None, run_seen: false, camera_lost: false, recheck_at: None });
            }
            Ok(None) => {}
            Err(e) => tracing::error!("cannot read the open timelapse: {e}"),
        }
        loop {
            // A panic must end neither the daemon nor the timelapses.
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.tick())).is_err() {
                tracing::error!("timelapse step panicked");
            }
            match rx.recv_timeout(self.t.poll) {
                Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(cmd) => {
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.handle(cmd))).is_err() {
                        tracing::error!("timelapse command panicked");
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
        *lock(&self.current) = None;
    }

    fn tick(&mut self) {
        while let Ok(v) = self.status_rx.try_recv() {
            self.run = Some((v, Instant::now()));
        }
        self.follow_run();
        self.check_stop_at();
        self.recheck_trouble();
        self.take_due();
        self.publish();
    }

    // ---- the run ----

    /// The driven run's figures, when it is this timelapse's run.
    fn run_now(&self, run_id: Option<i64>) -> Option<(RunNow, Instant)> {
        let (v, at) = self.run.as_ref()?;
        let a = v.active.as_ref()?;
        (Some(a.run_id) == run_id).then(|| (a.clone(), *at))
    }

    fn follow_run(&mut self) {
        let Some(live) = self.live.as_mut() else { return };
        if live.row.kind != TimelapseKind::Run {
            return;
        }
        // No status yet (a daemon start): nothing known about the run.
        let Some((v, _)) = &self.run else { return };
        let now = Timestamp::now();
        let ours = v.active.as_ref().is_some_and(|a| Some(a.run_id) == live.row.run_id);
        if ours {
            if live.awaiting {
                live.awaiting = false;
                live.skip_to_now(now);
                tracing::info!(id = live.row.id, "run resumed: timelapse goes on");
            }
            live.gone_since = None;
            live.run_seen = true;
        } else if v.active.is_none() && v.pending_recovery {
            if !live.awaiting {
                tracing::info!(id = live.row.id, "run awaits resume: timelapse waits");
            }
            live.awaiting = true;
            live.gone_since = None;
        } else {
            // Seen driven, then gone: the run was stopped, end now. Never
            // seen yet: the status may predate the run, wait the grace.
            let since = *live.gone_since.get_or_insert_with(Instant::now);
            if live.run_seen || since.elapsed() >= self.t.run_gone_grace {
                self.end("its run ended");
            }
        }
    }

    fn check_stop_at(&mut self) {
        let due = self.live.as_ref().is_some_and(|l| l.row.stop_at.is_some_and(|s| Timestamp::now() >= s));
        if due {
            self.end("its stop time came");
        }
    }

    /// Ends the open timelapse (the row stays, its images too).
    fn end(&mut self, why: &str) {
        let Some(mut live) = self.live.take() else { return };
        live.row.state = TimelapseState::Ended;
        live.row.ended_at = Some(Timestamp::now());
        self.save(&live.row);
        tracing::info!(id = live.row.id, images = live.row.images, missed = live.row.missed, "timelapse ended: {why}");
        if live.row.kind == TimelapseKind::Run {
            self.event(
                live.row.run_id,
                "timelapse_end",
                Some(format!("{} images, {} missed", live.row.images, live.row.missed)),
            );
        }
    }

    // ---- images ----

    fn take_due(&mut self) {
        let now = Timestamp::now();
        let Some(live) = self.live.as_ref() else { return };
        if live.row.state != TimelapseState::Recording || live.awaiting {
            return;
        }
        // a run's images only while it is driven
        if live.row.kind == TimelapseKind::Run && self.run_now(live.row.run_id).is_none() {
            return;
        }
        let Some(live) = self.live.as_ref() else { return };
        let sched = live.schedule();
        let next = sched.next_slot(live.row.last_slot, now);
        if sched.due_at(next) > now {
            return;
        }
        // the disk or the folder first: a pause, the slot not missed
        if let Some(reason) = self.trouble(&live.row.folder) {
            self.pause_for(reason);
            return;
        }
        let Some(live) = self.live.as_mut() else { return };
        live.row.missed += sched.missed_between(live.row.last_slot, next);
        live.row.last_slot = Some(next);
        let mut row = live.row.clone();
        let result = self.capture(&mut row, Some(next));
        let Some(live) = self.live.as_mut() else { return };
        live.row = row;
        let (run_id, was_lost) = (live.row.run_id, live.camera_lost);
        match result {
            Ok(_) => {
                live.camera_lost = false;
                if was_lost {
                    tracing::info!(id = live.row.id, "timelapse camera back");
                    self.event(run_id, "camera_recovered", None);
                }
            }
            Err(CaptureError::Camera(e)) => {
                live.row.missed += 1;
                live.camera_lost = true;
                if !was_lost {
                    tracing::warn!(id = live.row.id, "timelapse waits for the camera: {e}");
                    self.event(run_id, "camera_lost", Some(e));
                }
            }
            Err(CaptureError::Folder(e)) => {
                tracing::warn!(id = live.row.id, "timelapse image not written: {e}");
                self.pause_for("folder");
            }
        }
        if let Some(l) = self.live.as_ref() {
            let row = l.row.clone();
            self.save(&row);
        }
    }

    /// Why images cannot be written to `folder` now: `folder` (gone and not
    /// creatable, a USB disk unplugged) or `disk` (under `PAUSE_FREE_BYTES`).
    fn trouble(&self, folder: &str) -> Option<&'static str> {
        let p = Path::new(folder);
        if !p.is_dir() && files::ensure_writable(p).is_err() {
            return Some("folder");
        }
        if (self.t.free_bytes)(p).is_some_and(|f| f < files::PAUSE_FREE_BYTES) {
            return Some("disk");
        }
        None
    }

    /// Paused by the disk or the folder, looked at again every
    /// `trouble_recheck`, resumed by itself.
    fn pause_for(&mut self, reason: &'static str) {
        let recheck = self.t.trouble_recheck;
        let Some(live) = self.live.as_mut() else { return };
        live.row.state = TimelapseState::Paused;
        live.row.pause_reason = Some(reason.to_string());
        live.recheck_at = Some(Instant::now() + recheck);
        let row = live.row.clone();
        tracing::warn!(id = row.id, folder = %row.folder, "timelapse paused: {reason}");
        self.save(&row);
        self.event(row.run_id, "timelapse_paused", Some(reason.to_string()));
    }

    fn recheck_trouble(&mut self) {
        let Some(live) = self.live.as_ref() else { return };
        let by_trouble = live.row.state == TimelapseState::Paused && live.row.pause_reason.as_deref() != Some("user");
        if !by_trouble || live.recheck_at.is_some_and(|t| Instant::now() < t) {
            return;
        }
        let folder = live.row.folder.clone();
        let still = self.trouble(&folder);
        let recheck = self.t.trouble_recheck;
        let Some(live) = self.live.as_mut() else { return };
        match still {
            Some(reason) => {
                live.row.pause_reason = Some(reason.to_string());
                live.recheck_at = Some(Instant::now() + recheck);
            }
            None => {
                live.row.state = TimelapseState::Recording;
                live.row.pause_reason = None;
                live.recheck_at = None;
                live.skip_to_now(Timestamp::now());
                let row = live.row.clone();
                tracing::info!(id = row.id, "timelapse resumed: folder and disk fine again");
                self.save(&row);
                self.event(row.run_id, "timelapse_resumed", None);
            }
        }
    }

    /// One still into `row`'s folder and the database; `row` counts it.
    fn capture(&mut self, row: &mut TimelapseRow, slot: Option<i64>) -> Result<i64, CaptureError> {
        let still = self.cam.still(STILL_QUALITY).map_err(CaptureError::Camera)?;
        let controls = self
            .cam
            .controls()
            .ok()
            .and_then(|c| serde_json::to_string(&c).ok())
            .unwrap_or_else(|| "[]".to_string());
        let run = self.run_now(row.run_id).filter(|_| row.kind == TimelapseKind::Run);
        let elapsed_s = match &run {
            Some((r, at)) => r.elapsed_s + at.elapsed().as_secs_f64(),
            None => still.at.duration_since(row.started_at).as_secs_f64().max(0.0),
        };
        let r = run.map(|(r, _)| r).unwrap_or_default();
        let name = files::image_name(slot, row.images + 1, elapsed_s);
        let folder = PathBuf::from(&row.folder);
        files::write_image(&folder, &name, &still.jpeg, &still.thumb)
            .map_err(|e| CaptureError::Folder(format!("cannot write the image: {e}")))?;
        let csv = CsvRow {
            file: name.clone(),
            taken_at: still.at.to_string(),
            elapsed_s,
            manual: slot.is_none(),
            camera: still.device.clone(),
            width: still.width,
            height: still.height,
            setpoint: r.setpoint,
            unit: r.unit.clone(),
            delivered_ml: r.delivered_ml,
            weight_g: r.weight_g,
            trim_c: r.trim_c,
            controls: controls.clone(),
        };
        if let Err(e) = files::append_csv(&folder, &csv) {
            tracing::warn!(id = row.id, "images.csv not written: {e}");
        }
        let bytes = still.jpeg.len() as i64;
        let id = self
            .store
            .insert_image(&NewImage {
                timelapse_id: row.id,
                slot,
                taken_at: still.at,
                elapsed_s,
                file: name.clone(),
                thumb: name,
                bytes,
                width: still.width,
                height: still.height,
                camera: still.device,
                controls,
                setpoint: r.setpoint,
                unit: r.unit,
                delivered_ml: r.delivered_ml,
                weight_g: r.weight_g,
                trim_c: r.trim_c,
            })
            .map_err(|e| CaptureError::Folder(format!("cannot record the image: {e}")))?;
        row.images += 1;
        row.bytes += bytes;
        Ok(id)
    }

    // ---- commands ----

    /// Each command publishes the state it leaves before answering: the
    /// caller reads `current()` right after.
    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::StartRun { run_id, name, interval_s, reply } => {
                let r = self.start_run(run_id, name, interval_s);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::StartStandalone { name, interval_s, stop_after_s, reply } => {
                let r = self.start_standalone(name, interval_s, stop_after_s);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Stop(reply) => {
                let r = match self.live.as_ref().map(|l| l.row.kind) {
                    Some(TimelapseKind::Run) => Err("A run's timelapse stops with its run".to_string()),
                    Some(_) => {
                        self.end("stopped from the Camera page");
                        Ok(())
                    }
                    None => Err("No timelapse is recording".to_string()),
                };
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Pause(reply) => {
                let r = match self.live.as_mut() {
                    Some(l) if l.row.state == TimelapseState::Recording => {
                        l.row.state = TimelapseState::Paused;
                        l.row.pause_reason = Some("user".into());
                        let row = l.row.clone();
                        self.save(&row);
                        Ok(())
                    }
                    Some(_) => Err("The timelapse is already paused".to_string()),
                    None => Err("No timelapse is recording".to_string()),
                };
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Resume(reply) => {
                let r = match self.live.as_mut() {
                    Some(l) if l.row.state == TimelapseState::Paused => {
                        l.row.state = TimelapseState::Recording;
                        l.row.pause_reason = None;
                        l.skip_to_now(Timestamp::now());
                        let row = l.row.clone();
                        self.save(&row);
                        Ok(())
                    }
                    Some(_) => Err("The timelapse is not paused".to_string()),
                    None => Err("No timelapse is recording".to_string()),
                };
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::SetInterval(interval_s, reply) => {
                let r = match self.live.as_mut() {
                    _ if interval_s < 1 => Err("the interval must be at least 1 s".to_string()),
                    Some(l) => {
                        let s = l.schedule().with_interval(interval_s, l.row.last_slot, Timestamp::now());
                        l.row.anchor_at = s.anchor;
                        l.row.anchor_slot = s.anchor_slot;
                        l.row.interval_s = s.interval_s;
                        let row = l.row.clone();
                        self.save(&row);
                        Ok(())
                    }
                    None => Err("No timelapse is recording".to_string()),
                };
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::TakePicture(reply) => {
                let r = self.take_picture();
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::DeleteForRun(run_id, reply) => {
                let r = self.delete_for_run(run_id);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::DeleteAll(reply) => {
                let r = self.delete_all();
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Shutdown => {}
        }
    }

    /// A new row and its folder under the captures folder.
    fn create(&mut self, t: NewTimelapse) -> Result<TimelapseRow, String> {
        let id = self.store.insert_timelapse(&t).map_err(|e| e.to_string())?;
        let folder = (self.root)().join(files::folder_name(&local_stamp("%Y-%m-%d_%H%M"), &t.name, id));
        if let Err(e) = files::ensure_writable(&folder) {
            let _ = self.store.delete_timelapse(id);
            return Err(e);
        }
        let mut row = self.store.timelapse(id).map_err(|e| e.to_string())?.ok_or("timelapse not recorded")?;
        row.folder = folder.to_string_lossy().into_owned();
        self.store.update_timelapse(&row).map_err(|e| e.to_string())?;
        Ok(row)
    }

    fn start_run(&mut self, run_id: i64, name: String, interval_s: i64) -> Result<i64, String> {
        if interval_s < 1 {
            return Err("the interval must be at least 1 s".to_string());
        }
        if self.live.is_some() {
            self.end("a run started with its own timelapse");
        }
        let row = self.create(NewTimelapse {
            run_id: Some(run_id),
            kind: TimelapseKind::Run,
            name,
            folder: String::new(),
            interval_s,
            started_at: Timestamp::now(),
            stop_at: None,
        })?;
        self.event(Some(run_id), "timelapse_start", Some(format!("every {interval_s} s, in {}", row.folder)));
        let id = row.id;
        self.live = Some(Live { row, awaiting: false, gone_since: None, run_seen: false, camera_lost: false, recheck_at: None });
        Ok(id)
    }

    fn start_standalone(&mut self, name: String, interval_s: i64, stop_after_s: Option<i64>) -> Result<i64, String> {
        if self.run.as_ref().is_some_and(|(v, _)| v.active.is_some()) {
            return Err("A run is going: switch its timelapse on in New run".to_string());
        }
        if self.live.is_some() {
            return Err("A timelapse is already recording".to_string());
        }
        if name.trim().is_empty() {
            return Err("give the timelapse a name".to_string());
        }
        if interval_s < 1 {
            return Err("the interval must be at least 1 s".to_string());
        }
        let now = Timestamp::now();
        let row = self.create(NewTimelapse {
            run_id: None,
            kind: TimelapseKind::Standalone,
            name: name.trim().to_string(),
            folder: String::new(),
            interval_s,
            started_at: now,
            stop_at: stop_after_s.map(|s| now + SignedDuration::from_secs(s)),
        })?;
        let id = row.id;
        self.live = Some(Live { row, awaiting: false, gone_since: None, run_seen: false, camera_lost: false, recheck_at: None });
        Ok(id)
    }

    fn take_picture(&mut self) -> Result<i64, String> {
        let mut row = match self.live.as_ref() {
            Some(l) => l.row.clone(),
            None => {
                let day = local_stamp("%Y-%m-%d");
                match self.store.pictures_of_day(&day).map_err(|e| e.to_string())? {
                    Some(r) => r,
                    None => self.create(NewTimelapse {
                        run_id: None,
                        kind: TimelapseKind::Pictures,
                        name: format!("Pictures {day}"),
                        folder: String::new(),
                        interval_s: 0,
                        started_at: Timestamp::now(),
                        stop_at: None,
                    })?,
                }
            }
        };
        let id = self.capture(&mut row, None).map_err(CaptureError::message)?;
        match self.live.as_mut() {
            Some(l) if l.row.id == row.id => {
                l.row.images = row.images;
                l.row.bytes = row.bytes;
            }
            _ => {}
        }
        Ok(id)
    }

    fn delete_rows_and_folder(&self, t: &TimelapseRow) -> Result<(), String> {
        if let Err(e) = files::remove_folder(Path::new(&t.folder)) {
            tracing::warn!(id = t.id, folder = %t.folder, "timelapse folder not removed: {e}");
        }
        self.store.delete_timelapse(t.id).map_err(|e| e.to_string())
    }

    fn delete_for_run(&mut self, run_id: i64) -> Result<(), String> {
        while let Some(t) = self.store.timelapse_of_run(run_id).map_err(|e| e.to_string())? {
            if self.live.as_ref().is_some_and(|l| l.row.id == t.id) {
                return Err("this run's timelapse is recording".to_string());
            }
            self.delete_rows_and_folder(&t)?;
        }
        Ok(())
    }

    fn delete_all(&mut self) -> Result<(), String> {
        let live = self.live.as_ref().map(|l| l.row.id);
        for t in self.store.all_timelapses().map_err(|e| e.to_string())? {
            if Some(t.id) != live {
                self.delete_rows_and_folder(&t)?;
            }
        }
        Ok(())
    }

    // ---- bookkeeping ----

    fn save(&self, row: &TimelapseRow) {
        if let Err(e) = self.store.update_timelapse(row) {
            tracing::warn!(id = row.id, "timelapse not saved: {e}");
        }
    }

    fn event(&self, run_id: Option<i64>, kind: &str, detail: Option<String>) {
        if run_id.is_none() {
            return;
        }
        let e = NewEvent { run_id, wall_time: Timestamp::now(), level: EventLevel::Info, kind: kind.into(), detail };
        if let Err(err) = self.store.log_event(&e) {
            tracing::warn!("timelapse event {kind} not journalled: {err}");
        }
    }

    fn publish(&self) {
        let now = Timestamp::now();
        let cur = self.live.as_ref().map(|l| {
            let state = if l.row.state == TimelapseState::Paused {
                "paused"
            } else if l.awaiting {
                "awaiting_resume"
            } else if l.camera_lost {
                "waiting_camera"
            } else {
                "recording"
            };
            CurrentTimelapse {
                id: l.row.id,
                kind: l.row.kind,
                run_id: l.row.run_id,
                name: l.row.name.clone(),
                state,
                pause_reason: l.row.pause_reason.clone(),
                interval_s: l.row.interval_s,
                images: l.row.images,
                missed: l.row.missed,
                bytes: l.row.bytes,
                next_at: (l.row.state == TimelapseState::Recording && !l.awaiting).then(|| l.next_at(now)),
                ends_at: l.row.stop_at,
                started_at: l.row.started_at,
                free_bytes: (self.t.free_bytes)(Path::new(&l.row.folder)),
                folder: l.row.folder.clone(),
            }
        });
        *lock(&self.current) = cur;
    }
}
