//! The camera, supervised. Capture runs in a helper process
//! (`fermentool-camera.exe`, crate `fermentool-camera`): a webcam driver that
//! wedges, crashes or spins after an unplug takes only that process with it,
//! never the pump, the balance or the journal. Nothing here is ever called
//! from the control thread.
//!
//! Threads, as for the pump's `WatchdogTransport` and the balance's
//! `PolledScale`:
//! - `camera-supervisor` owns the helper, its retries and the grab loop. It
//!   waits on its command channel with a timeout equal to the next due action
//!   (a grab, a retry), like `control_loop`.
//! - `camera-reader`, one per helper, reads the helper's replies into a
//!   channel. The supervisor waits on that channel with a bound, never on the
//!   pipe itself, so a helper that never answers costs a timeout, then a
//!   kill. The reader is dropped with its helper, never joined: killing the
//!   process closes its stdout and ends the read.
//!
//! **The helper process is the unit of release.** On the webcam backend
//! `Close` does not free the camera, only the helper's exit does. So a want
//! that goes away, a want that changes, a failed `Open`, a grab error and a
//! timeout all end the helper (`Quit`, then a kill after 1 s; a timeout is
//! killed at once), and the next attempt runs on a fresh one. Should the
//! daemon itself die, every helper is in a kill-on-close job (`job`, on
//! Windows) and also exits on its stdin closing, even mid-grab.
//!
//! [`Camera::state`], [`Camera::latest`] and the viewer count read shared
//! state behind short locks and never wait on the helper.

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use fermentool_camera::protocol::{read_reply, write_request, DeviceInfo, ErrorCode, Reply, Request};
use jiff::Timestamp;
use serde::Serialize;

/// Bound on one request (a grab, the device list).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Bound on `Open`: the helper grabs a first frame to confirm the device,
/// and a webcam takes a moment to start streaming.
pub const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// Grabs per second while someone watches the live view. None without.
pub const LIVE_FPS: u32 = 5;

/// Another program holds the camera: asked again this often, never in a
/// tight loop (each attempt is a new helper process).
pub const IN_USE_RETRY: Duration = Duration::from_secs(15);

/// In-use attempts in a row retried every `IN_USE_RETRY`; past them the
/// delay doubles up to `IN_USE_MAX` (another program holding the webcam for
/// a whole run would otherwise launch a helper every 15 s, 24,000 over 100 h).
pub const IN_USE_FAST_TRIES: u32 = 10;
pub const IN_USE_MAX: Duration = Duration::from_secs(120);

/// After a grab error or a failed open, the next attempt on a fresh helper.
pub const LOST_RETRY: Duration = Duration::from_secs(3);

/// Failed attempts in a row retried every `LOST_RETRY`, so a replug is
/// caught quickly; past them the respawn backoff takes over (an unplug for
/// the length of a run would otherwise launch a helper every 3 s, 120,000
/// over 100 h).
pub const LOST_FAST_TRIES: u32 = 10;

/// Respawn backoff after a launch failure, a crash or a timeout: doubles from
/// the first to the max, back to the first after `RESPAWN_HEALTHY` connected.
const RESPAWN_FIRST: Duration = Duration::from_secs(2);
const RESPAWN_MAX: Duration = Duration::from_secs(30);
const RESPAWN_HEALTHY: Duration = Duration::from_secs(60);

/// How long a helper told to `Quit` has to exit before it is killed.
const QUIT_WAIT: Duration = Duration::from_secs(1);

/// The supervisor's longest sleep with nothing due: it checks then whether
/// an idle helper has died.
const IDLE_POLL: Duration = Duration::from_millis(500);

/// The helper's pipes and how to end it. `kill` must not block.
pub struct HelperIo {
    pub to_helper: Box<dyn Write + Send>,
    pub from_helper: Box<dyn Read + Send>,
    pub kill: Box<dyn FnMut() + Send>,
}

/// Starts one helper. The daemon's is [`ProcessLaunch`]; tests use an
/// in-process fake.
pub trait Launch: Send + 'static {
    fn launch(&mut self) -> io::Result<HelperIo>;
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CameraState {
    Off,
    Connecting { device: String },
    Connected { device: String, width: u32, height: u32 },
    InUse { device: String },
    Lost { device: String, since: Timestamp },
    /// `fermentool-camera.exe` is not next to the daemon.
    NoHelper,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub jpeg: Arc<Vec<u8>>,
    pub at: Timestamp,
    /// Rises by one per frame, across helpers, for the life of the daemon.
    pub seq: u64,
}

/// The camera to connect to: the `index`-th device named `device`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraWant {
    pub device: String,
    pub index: u32,
    pub width: u32,
    pub height: u32,
}

/// The supervisor's bounds and delays: the constants above in the daemon,
/// shortened by tests.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timings {
    pub request: Duration,
    pub open: Duration,
    pub live_interval: Duration,
    pub in_use_retry: Duration,
    pub in_use_max: Duration,
    pub lost_retry: Duration,
    pub respawn_first: Duration,
    pub respawn_max: Duration,
    pub healthy_reset: Duration,
    pub quit_wait: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            request: REQUEST_TIMEOUT,
            open: OPEN_TIMEOUT,
            live_interval: Duration::from_secs(1) / LIVE_FPS,
            in_use_retry: IN_USE_RETRY,
            in_use_max: IN_USE_MAX,
            lost_retry: LOST_RETRY,
            respawn_first: RESPAWN_FIRST,
            respawn_max: RESPAWN_MAX,
            healthy_reset: RESPAWN_HEALTHY,
            quit_wait: QUIT_WAIT,
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// What the supervisor and the API share. Every lock is held for a copy or
/// an assignment, never across a request.
struct Shared {
    state: Mutex<CameraState>,
    latest: Mutex<Option<Frame>>,
    fresh: Condvar,
    viewers: AtomicUsize,
}

impl Shared {
    fn set_state(&self, s: CameraState) {
        *lock(&self.state) = s;
    }

    fn state(&self) -> CameraState {
        lock(&self.state).clone()
    }
}

enum Cmd {
    Want(Option<CameraWant>),
    /// A viewer arrived: grab now rather than at the next idle poll.
    Wake,
    Shutdown(mpsc::Sender<()>),
}

type SharedLaunch = Arc<Mutex<Box<dyn Launch>>>;

/// Handle on the camera supervisor. Cheap to clone; the supervisor ends
/// (and its helper with it) on [`shutdown`](Self::shutdown) or once every
/// handle is dropped.
#[derive(Clone)]
pub struct Camera {
    inner: Arc<Inner>,
}

struct Inner {
    shared: Arc<Shared>,
    cmd: mpsc::Sender<Cmd>,
    launch: SharedLaunch,
    t: Timings,
    /// The device list, from the last `List`. Its lock also keeps two
    /// listings from running at once.
    devices: Mutex<Option<Vec<DeviceInfo>>>,
}

/// One watcher of the live view: grabs run at [`LIVE_FPS`] while any is held.
pub struct ViewerGuard {
    inner: Arc<Inner>,
}

impl Drop for ViewerGuard {
    fn drop(&mut self) {
        self.inner.shared.viewers.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Camera {
    pub fn spawn(launch: impl Launch, want: Option<CameraWant>) -> Camera {
        Self::spawn_with(launch, want, Timings::default())
    }

    pub(crate) fn spawn_with(launch: impl Launch, want: Option<CameraWant>, t: Timings) -> Camera {
        let shared = Arc::new(Shared {
            state: Mutex::new(CameraState::Off),
            latest: Mutex::new(None),
            fresh: Condvar::new(),
            viewers: AtomicUsize::new(0),
        });
        let launch: SharedLaunch = Arc::new(Mutex::new(Box::new(launch)));
        let (cmd, rx) = mpsc::channel();
        let mut sup = Supervisor {
            shared: Arc::clone(&shared),
            launch: Arc::clone(&launch),
            t,
            want: None,
            helper: None,
            next_attempt: Instant::now(),
            backoff: t.respawn_first,
            healthy_since: None,
            next_grab: Instant::now(),
            seq: 0,
            lost_tries: 0,
            in_use_tries: 0,
        };
        sup.set_want(want);
        let spawned = std::thread::Builder::new().name("camera-supervisor".into()).spawn(move || sup.run(rx));
        if let Err(e) = spawned {
            // No thread, no camera; the daemon runs on without one.
            tracing::error!("cannot start the camera supervisor: {e}");
            shared.set_state(CameraState::Off);
        }
        Camera { inner: Arc::new(Inner { shared, cmd, launch, t, devices: Mutex::new(None) }) }
    }

    pub fn state(&self) -> CameraState {
        self.inner.shared.state()
    }

    pub fn latest(&self) -> Option<Frame> {
        lock(&self.inner.shared.latest).clone()
    }

    /// Connect to `want`, or disconnect with `None`. The same want again
    /// retries at once (Connect after closing the program that held it).
    pub fn set_want(&self, want: Option<CameraWant>) {
        let _ = self.inner.cmd.send(Cmd::Want(want));
    }

    /// The PC's cameras, listed by a short-lived helper of its own (never the
    /// live one: a listing leaves the devices it looked at activated until
    /// its process exits). Cached until `refresh`. Blocking: at most
    /// [`REQUEST_TIMEOUT`] for the answer plus the 1 s quit wait, and a second
    /// caller waits for the first (the cache's lock is held throughout).
    /// Call it on a blocking thread (`spawn_blocking`), never on the async
    /// runtime nor on the control thread.
    pub fn devices(&self, refresh: bool) -> Result<Vec<DeviceInfo>, String> {
        let mut cache = lock(&self.inner.devices);
        if let (Some(list), false) = (&*cache, refresh) {
            return Ok(list.clone());
        }
        let t = self.inner.t;
        let mut helper =
            Helper::start(&self.inner.launch).map_err(|e| format!("cannot start the camera helper: {e}"))?;
        match helper.request(&Request::List, t.request) {
            Ok(Reply::Devices(list)) => {
                retire(helper, t.quit_wait);
                *cache = Some(list.clone());
                Ok(list)
            }
            Ok(Reply::Error { message, .. }) => {
                retire(helper, t.quit_wait);
                Err(message)
            }
            Ok(other) => {
                helper.kill_now();
                Err(format!("unexpected answer from the camera helper: {other:?}"))
            }
            Err(Fail::Timeout) => {
                tracing::warn!("camera helper did not list the cameras in time, killed");
                helper.kill_now();
                Err(format!("the camera helper did not answer within {} s", t.request.as_secs()))
            }
            Err(Fail::Dead(why)) => {
                helper.kill_now();
                Err(format!("the camera helper ended: {why}"))
            }
        }
    }

    pub fn viewer(&self) -> ViewerGuard {
        self.inner.shared.viewers.fetch_add(1, Ordering::SeqCst);
        let _ = self.inner.cmd.send(Cmd::Wake);
        ViewerGuard { inner: Arc::clone(&self.inner) }
    }

    pub fn viewers(&self) -> usize {
        self.inner.shared.viewers.load(Ordering::SeqCst)
    }

    /// The first frame newer than `seq`, waiting at most `max` for it.
    pub fn wait_frame_after(&self, seq: u64, max: Duration) -> Option<Frame> {
        let shared = &self.inner.shared;
        let guard = lock(&shared.latest);
        let (guard, _) = shared
            .fresh
            .wait_timeout_while(guard, max, |f| f.as_ref().is_none_or(|f| f.seq <= seq))
            .unwrap_or_else(|p| p.into_inner());
        guard.as_ref().filter(|f| f.seq > seq).cloned()
    }

    /// Ends the helper (`Quit`, a kill after 1 s) and the supervisor. Waits
    /// for that, a little over 1 s at most.
    pub fn shutdown(&self) {
        let (done, wait) = mpsc::channel();
        if self.inner.cmd.send(Cmd::Shutdown(done)).is_ok() {
            let _ = wait.recv_timeout(self.inner.t.quit_wait + Duration::from_millis(500));
        }
    }
}

/// Why a request got no answer.
enum Fail {
    /// Nothing within the bound: the helper is hung.
    Timeout,
    /// The pipe broke or the helper ended.
    Dead(String),
}

/// One running helper, as the supervisor (or a listing) sees it.
struct Helper {
    to: Box<dyn Write + Send>,
    replies: mpsc::Receiver<io::Result<Reply>>,
    kill: Box<dyn FnMut() + Send>,
}

impl Helper {
    fn start(launch: &SharedLaunch) -> io::Result<Helper> {
        let HelperIo { to_helper, mut from_helper, mut kill } = lock(launch).launch()?;
        let (tx, replies) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("camera-reader".into()).spawn(move || loop {
            let reply = read_reply(&mut from_helper);
            let end = reply.is_err();
            if tx.send(reply).is_err() || end {
                break;
            }
        });
        if let Err(e) = spawned {
            kill();
            return Err(e);
        }
        Ok(Helper { to: to_helper, replies, kill })
    }

    /// Sends `request` and waits at most `within` for its reply. One request
    /// at a time, a few dozen bytes each: the write lands in the pipe's
    /// buffer and does not block, even on a hung helper.
    fn request(&mut self, request: &Request, within: Duration) -> Result<Reply, Fail> {
        write_request(&mut self.to, request).map_err(|e| Fail::Dead(e.to_string()))?;
        match self.replies.recv_timeout(within) {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(e)) => Err(Fail::Dead(e.to_string())),
            Err(RecvTimeoutError::Timeout) => Err(Fail::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(Fail::Dead("the helper's output closed".into())),
        }
    }

    /// An idle helper whose output closed (it crashed or was killed).
    fn has_ended(&self) -> bool {
        matches!(self.replies.try_recv(), Ok(Err(_)) | Err(TryRecvError::Disconnected))
    }

    fn kill_now(mut self) {
        (self.kill)();
    }
}

/// `Quit`, then wait up to `within` for the helper's output to close (its
/// exit); kill it if it has not. True when it had to be killed.
fn retire(mut helper: Helper, within: Duration) -> bool {
    if write_request(&mut helper.to, &Request::Quit).is_ok() {
        // Its stdin closes too: a helper that missed the Quit sees EOF.
        helper.to = Box::new(io::sink());
        let end = Instant::now() + within;
        loop {
            match helper.replies.recv_timeout(end.saturating_duration_since(Instant::now())) {
                Ok(Ok(_)) => continue, // a late reply to an abandoned request
                Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => return false,
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
    }
    helper.kill_now();
    true
}

struct Supervisor {
    shared: Arc<Shared>,
    launch: SharedLaunch,
    t: Timings,
    want: Option<CameraWant>,
    /// Present only while connected: every failure ends the helper.
    helper: Option<Helper>,
    next_attempt: Instant,
    backoff: Duration,
    healthy_since: Option<Instant>,
    next_grab: Instant,
    seq: u64,
    /// Failed attempts since the last frame (or the last healthy minute).
    lost_tries: u32,
    /// In-use attempts in a row, since the last frame or want.
    in_use_tries: u32,
}

impl Supervisor {
    fn run(&mut self, rx: mpsc::Receiver<Cmd>) {
        loop {
            // A panic here must end neither the daemon nor the camera: it
            // is logged and the loop goes on, like the control loop's.
            let stepped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.step()));
            if stepped.is_err() {
                tracing::error!("camera supervisor step panicked");
                if let Some(h) = self.helper.take() {
                    h.kill_now();
                }
                self.next_attempt = Instant::now() + self.t.lost_retry;
            }
            let wait = self.next_due().saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(Cmd::Want(w)) => self.set_want(w),
                Ok(Cmd::Wake) | Err(RecvTimeoutError::Timeout) => {}
                Ok(Cmd::Shutdown(done)) => {
                    self.end_helper();
                    self.shared.set_state(CameraState::Off);
                    let _ = done.send(());
                    return;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.end_helper();
                    return;
                }
            }
        }
    }

    fn next_due(&self) -> Instant {
        let mut due = Instant::now() + IDLE_POLL;
        if self.want.is_some() && self.helper.is_none() {
            due = due.min(self.next_attempt);
        }
        if self.helper.is_some() && self.shared.viewers.load(Ordering::SeqCst) > 0 {
            due = due.min(self.next_grab);
        }
        due
    }

    fn step(&mut self) {
        if self.helper.as_ref().is_some_and(Helper::has_ended) {
            let device = self.device();
            if let Some(h) = self.helper.take() {
                h.kill_now();
            }
            self.healthy_since = None;
            self.lost(&device, "the camera helper ended");
            self.backoff_attempt();
        }
        if self.want.is_some() && self.helper.is_none() && Instant::now() >= self.next_attempt {
            self.attempt();
        }
        if self.helper.is_some() {
            if self.healthy_since.is_some_and(|t| t.elapsed() >= self.t.healthy_reset) {
                self.backoff = self.t.respawn_first;
                self.lost_tries = 0;
                self.in_use_tries = 0;
            }
            if self.shared.viewers.load(Ordering::SeqCst) > 0 && Instant::now() >= self.next_grab {
                self.grab();
            }
        }
    }

    fn device(&self) -> String {
        self.want.as_ref().map(|w| w.device.clone()).unwrap_or_default()
    }

    fn set_want(&mut self, want: Option<CameraWant>) {
        if want == self.want {
            if want.is_some() && self.helper.is_none() {
                self.next_attempt = Instant::now();
                self.backoff = self.t.respawn_first;
                self.in_use_tries = 0;
            }
            return;
        }
        self.shared.set_state(match &want {
            None => CameraState::Off,
            Some(w) => CameraState::Connecting { device: w.device.clone() },
        });
        self.end_helper();
        self.want = want;
        self.next_attempt = Instant::now();
        self.backoff = self.t.respawn_first;
        self.lost_tries = 0;
        self.in_use_tries = 0;
    }

    /// Quit the helper, killed if it does not exit in time.
    fn end_helper(&mut self) {
        if let Some(h) = self.helper.take() {
            if retire(h, self.t.quit_wait) {
                tracing::warn!("camera helper did not quit within {} ms, killed", self.t.quit_wait.as_millis());
            }
        }
        self.healthy_since = None;
    }

    /// Kill the helper at once: it is hung, or it said something it should not.
    fn kill_helper(&mut self, h: Helper, why: &str) {
        tracing::warn!("camera helper killed: {why}");
        h.kill_now();
        self.healthy_since = None;
    }

    fn backoff_attempt(&mut self) {
        self.next_attempt = Instant::now() + self.backoff;
        self.backoff = (self.backoff * 2).min(self.t.respawn_max);
    }

    /// The camera answered but failed: `LOST_RETRY` for the first
    /// [`LOST_FAST_TRIES`] in a row, then the backoff (never under `LOST_RETRY`).
    fn lost_attempt(&mut self) {
        self.lost_tries += 1;
        if self.lost_tries <= LOST_FAST_TRIES {
            self.next_attempt = Instant::now() + self.t.lost_retry;
        } else {
            self.next_attempt = Instant::now() + self.backoff.max(self.t.lost_retry);
            self.backoff = (self.backoff * 2).min(self.t.respawn_max);
        }
    }

    /// Another program holds the camera: `InUse`, and the next attempt
    /// `IN_USE_RETRY` later for the first [`IN_USE_FAST_TRIES`] in a row, then
    /// doubling up to `IN_USE_MAX`.
    fn in_use(&mut self, device: &str, message: &str) {
        if !matches!(self.shared.state(), CameraState::InUse { .. }) {
            tracing::info!(device, "camera in use by another program: {message}");
        }
        self.shared.set_state(CameraState::InUse { device: device.to_string() });
        self.in_use_tries += 1;
        let over = self.in_use_tries.saturating_sub(IN_USE_FAST_TRIES).min(16);
        let delay = self.t.in_use_retry.saturating_mul(1 << over).min(self.t.in_use_max.max(self.t.in_use_retry));
        self.next_attempt = Instant::now() + delay;
    }

    /// `Lost`, keeping the time of the first loss across retries.
    fn lost(&mut self, device: &str, why: &str) {
        let since = match self.shared.state() {
            CameraState::Lost { device: d, since } if d == device => {
                tracing::debug!(device, "camera still not answering: {why}");
                since
            }
            _ => {
                tracing::info!(device, "camera lost: {why}");
                Timestamp::now()
            }
        };
        self.shared.set_state(CameraState::Lost { device: device.to_string(), since });
    }

    fn attempt(&mut self) {
        let Some(want) = self.want.clone() else { return };
        let mut helper = match Helper::start(&self.launch) {
            Ok(h) => h,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if self.shared.state() != CameraState::NoHelper {
                    tracing::warn!("camera helper not found: {e}");
                }
                self.shared.set_state(CameraState::NoHelper);
                self.backoff_attempt();
                return;
            }
            Err(e) => {
                self.lost(&want.device, &format!("cannot start the camera helper: {e}"));
                self.backoff_attempt();
                return;
            }
        };
        let open = Request::Open {
            device: want.device.clone(),
            index: want.index,
            width: want.width,
            height: want.height,
        };
        match helper.request(&open, self.t.open) {
            Ok(Reply::Opened { width, height }) => {
                match self.shared.state() {
                    CameraState::Lost { .. } | CameraState::InUse { .. } | CameraState::NoHelper => {
                        tracing::info!(device = %want.device, width, height, "camera recovered")
                    }
                    _ => tracing::info!(device = %want.device, width, height, "camera connected"),
                }
                self.shared.set_state(CameraState::Connected { device: want.device, width, height });
                self.helper = Some(helper);
                self.healthy_since = Some(Instant::now());
                self.next_grab = Instant::now();
            }
            Ok(Reply::Error { code: ErrorCode::InUse, message }) => {
                retire(helper, self.t.quit_wait);
                self.in_use(&want.device, &message);
            }
            Ok(Reply::Error { message, .. }) => {
                retire(helper, self.t.quit_wait);
                self.lost(&want.device, &message);
                self.lost_attempt();
            }
            Ok(other) => {
                self.kill_helper(helper, &format!("unexpected answer to Open: {other:?}"));
                self.lost(&want.device, "the camera helper answered out of turn");
                self.backoff_attempt();
            }
            Err(Fail::Timeout) => {
                self.kill_helper(helper, &format!("no answer to Open within {} s", self.t.open.as_secs()));
                self.lost(&want.device, "the camera did not open in time");
                self.backoff_attempt();
            }
            Err(Fail::Dead(why)) => {
                self.kill_helper(helper, &why);
                self.lost(&want.device, &format!("the camera helper ended: {why}"));
                self.backoff_attempt();
            }
        }
    }

    fn grab(&mut self) {
        let Some(helper) = self.helper.as_mut() else { return };
        let started = Instant::now();
        let reply = helper.request(&Request::Grab, self.t.request);
        if let Ok(Reply::Frame(jpeg)) = reply {
            self.seq += 1;
            let frame = Frame { jpeg: Arc::new(jpeg), at: Timestamp::now(), seq: self.seq };
            *lock(&self.shared.latest) = Some(frame);
            self.shared.fresh.notify_all();
            self.next_grab = started + self.t.live_interval;
            self.lost_tries = 0;
            self.in_use_tries = 0;
            return;
        }
        // Any other answer ends this helper; the next attempt is a fresh one.
        let device = self.device();
        let Some(h) = self.helper.take() else { return };
        match reply {
            // Another program took the webcam mid-stream (its HRESULT comes
            // at a frame): in use, retried at the in-use pace, never lost.
            Ok(Reply::Error { code: ErrorCode::InUse, message }) => {
                retire(h, self.t.quit_wait);
                self.healthy_since = None;
                self.in_use(&device, &message);
            }
            Ok(Reply::Error { message, .. }) => {
                retire(h, self.t.quit_wait);
                self.healthy_since = None;
                self.lost(&device, &message);
                self.lost_attempt();
            }
            Err(Fail::Timeout) => {
                self.kill_helper(h, &format!("no frame within {} s", self.t.request.as_secs()));
                self.lost(&device, "no frame in time");
                self.backoff_attempt();
            }
            Err(Fail::Dead(why)) => {
                self.kill_helper(h, &why);
                self.lost(&device, &format!("the camera helper ended: {why}"));
                self.backoff_attempt();
            }
            Ok(other) => {
                self.kill_helper(h, &format!("unexpected answer to Grab: {other:?}"));
                self.lost(&device, "the camera helper answered out of turn");
                self.backoff_attempt();
            }
        }
    }
}

/// `fermentool-camera.exe` next to the daemon: the plain name (a dev build,
/// `target/release`), then the name Tauri gives a sidecar.
pub fn helper_exe() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    ["fermentool-camera.exe", "fermentool-camera-x86_64-pc-windows-msvc.exe"]
        .iter()
        .map(|name| dir.join(name))
        .find(|p| p.is_file())
}

/// The real helper: a child process with piped stdin, stdout and stderr (the
/// last into the log), started by [`spawn_hidden`].
pub struct ProcessLaunch {
    pub exe: PathBuf,
    pub test_pattern: bool,
}

impl Launch for ProcessLaunch {
    fn launch(&mut self) -> io::Result<HelperIo> {
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(&self.exe);
        if self.test_pattern {
            cmd.arg("--test-pattern");
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = spawn_hidden(&mut cmd)?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err(io::Error::other("the camera helper's pipes did not open"));
        };
        if let Some(stderr) = child.stderr.take() {
            forward_stderr(stderr);
        }
        let mut child = Some(child);
        Ok(HelperIo {
            to_helper: Box::new(stdin),
            from_helper: Box::new(stdout),
            kill: Box::new(move || {
                if let Some(mut c) = child.take() {
                    let _ = c.kill();
                    // Reaped off the caller's thread: never wait here.
                    let _ = std::thread::Builder::new().name("camera-reap".into()).spawn(move || {
                        let _ = c.wait();
                    });
                }
            }),
        })
    }
}

/// Spawns `cmd` without a console window, below normal priority (the pump
/// and the balance come first), inside the daemon's kill-on-close job.
pub(crate) fn spawn_hidden(cmd: &mut std::process::Command) -> io::Result<std::process::Child> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        cmd.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
    }
    let child = cmd.spawn()?;
    #[cfg(windows)]
    job::assign(&child);
    Ok(child)
}

/// One job object for the daemon's life, set to kill its processes when its
/// last handle closes. The daemon never closes it, so the OS does at the
/// daemon's exit, however it exits (Task Manager, a crash, a shutdown that
/// gave up waiting): no helper outlives the daemon holding the webcam, even
/// one stuck in a driver call.
#[cfg(windows)]
pub(crate) mod job {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    #[cfg(test)]
    use windows_sys::Win32::System::JobObjects::{IsProcessInJob, QueryInformationJobObject};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    struct Job(HANDLE);

    // A job handle is a kernel handle, usable from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    static JOB: OnceLock<Option<Job>> = OnceLock::new();

    fn job() -> Option<HANDLE> {
        JOB.get_or_init(|| {
            // SAFETY: plain Win32 calls on a handle created here; `info` is a
            // zeroed plain-data struct of the size passed.
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    tracing::warn!("no job object for the camera helper: {}", io_err());
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const std::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    tracing::warn!("cannot set the camera job to kill on close: {}", io_err());
                    CloseHandle(h);
                    return None;
                }
                Some(Job(h))
            }
        })
        .as_ref()
        .map(|j| j.0)
    }

    fn io_err() -> std::io::Error {
        std::io::Error::last_os_error()
    }

    /// Puts `child` in the job. A failure is logged, never fatal: the helper
    /// still ends on its stdin closing, unless stuck in a driver call.
    pub(crate) fn assign(child: &std::process::Child) {
        let Some(job) = job() else { return };
        // SAFETY: both handles are valid for the call (the child is borrowed).
        if unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) } == 0 {
            tracing::warn!("camera helper not in the job: {}", io_err());
        }
    }

    #[cfg(test)]
    pub(crate) fn holds(child: &std::process::Child) -> bool {
        let Some(job) = job() else { return false };
        let mut inside = 0;
        // SAFETY: valid handles, `inside` outlives the call.
        let ok = unsafe { IsProcessInJob(child.as_raw_handle() as HANDLE, job, &mut inside) };
        ok != 0 && inside != 0
    }

    #[cfg(test)]
    pub(crate) fn kills_on_close() -> bool {
        let Some(job) = job() else { return false };
        // SAFETY: `info` is a zeroed plain-data struct of the size passed.
        unsafe {
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            let ok = QueryInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &mut info as *mut _ as *mut std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                std::ptr::null_mut(),
            );
            ok != 0 && info.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE != 0
        }
    }
}

/// Whether a line of the helper's stderr reports a problem (logged as a
/// warning) or is mere chatter (debug).
pub(crate) fn looks_like_error(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    ["error", "fail", "panic"].iter().any(|w| l.contains(w))
}

/// The helper's stderr, line by line, into the daemon's log, on a thread of
/// its own that ends at EOF (the helper's exit). Never joined.
fn forward_stderr(stderr: impl Read + Send + 'static) {
    use std::io::BufRead;
    let _ = std::thread::Builder::new().name("camera-stderr".into()).spawn(move || {
        for line in io::BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if looks_like_error(&line) {
                tracing::warn!("camera helper: {line}");
            } else {
                tracing::debug!("camera helper: {line}");
            }
        }
    });
}

/// A helper for tests: an in-process thread that speaks the protocol over
/// channel-backed pipes, scripted per launch to answer, fail or hang.
#[cfg(test)]
pub(crate) mod fake {
    use std::io::{self, Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::Instant;

    use fermentool_camera::protocol::{read_request, write_reply, DeviceInfo, ErrorCode, Reply, Request};

    use super::{HelperIo, Launch};

    /// What the fake does with a request.
    #[derive(Clone, Debug)]
    pub(crate) enum Act {
        Answer,
        Hang,
        Fail(ErrorCode),
    }

    /// How one helper instance behaves.
    #[derive(Clone, Debug)]
    pub(crate) struct Script {
        /// `launch` itself fails with this.
        pub launch_fails: Option<io::ErrorKind>,
        pub list: Act,
        pub open: Act,
        /// Grabs answered with a frame before `grab_then` applies.
        pub grab_ok: usize,
        pub grab_then: Act,
    }

    impl Script {
        pub(crate) fn healthy() -> Self {
            Self { launch_fails: None, list: Act::Answer, open: Act::Answer, grab_ok: usize::MAX, grab_then: Act::Answer }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub(crate) enum End {
        Quit,
        Eof,
        Killed,
        WriteFailed,
    }

    /// Everything the fakes saw, by launch number (0 = first launch).
    #[derive(Default)]
    pub(crate) struct Log {
        pub launches: Mutex<Vec<Instant>>,
        pub kills: AtomicUsize,
        pub requests: Mutex<Vec<(usize, Request, Instant)>>,
        pub ends: Mutex<Vec<(usize, End, Instant)>>,
    }

    impl Log {
        pub(crate) fn launch_count(&self) -> usize {
            self.launches.lock().unwrap().len()
        }
        pub(crate) fn kills(&self) -> usize {
            self.kills.load(Ordering::SeqCst)
        }
        /// When each request matching `f` arrived, with its helper's number.
        pub(crate) fn seen(&self, f: impl Fn(&Request) -> bool) -> Vec<(usize, Instant)> {
            self.requests.lock().unwrap().iter().filter(|(_, r, _)| f(r)).map(|(n, _, at)| (*n, *at)).collect()
        }
        pub(crate) fn end_of(&self, n: usize) -> Option<(End, Instant)> {
            self.ends.lock().unwrap().iter().find(|(m, _, _)| *m == n).map(|(_, e, at)| (*e, *at))
        }
    }

    pub(crate) struct FakeLaunch {
        log: Arc<Log>,
        plan: Box<dyn Fn(usize) -> Script + Send>,
    }

    impl FakeLaunch {
        pub(crate) fn new(plan: impl Fn(usize) -> Script + Send + 'static) -> (Self, Arc<Log>) {
            let log = Arc::new(Log::default());
            (Self { log: Arc::clone(&log), plan: Box::new(plan) }, log)
        }
        pub(crate) fn healthy() -> (Self, Arc<Log>) {
            Self::new(|_| Script::healthy())
        }
    }

    struct ChanWriter(mpsc::Sender<Vec<u8>>);

    impl Write for ChanWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.send(buf.to_vec()).map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Reads what a `ChanWriter` sent; EOF once the writer is dropped.
    struct ChanReader {
        rx: mpsc::Receiver<Vec<u8>>,
        buf: Vec<u8>,
        pos: usize,
    }

    impl Read for ChanReader {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            while self.pos >= self.buf.len() {
                match self.rx.recv() {
                    Ok(b) => {
                        self.buf = b;
                        self.pos = 0;
                    }
                    Err(_) => return Ok(0),
                }
            }
            let n = out.len().min(self.buf.len() - self.pos);
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    fn pipe() -> (ChanWriter, ChanReader) {
        let (tx, rx) = mpsc::channel();
        (ChanWriter(tx), ChanReader { rx, buf: Vec::new(), pos: 0 })
    }

    impl Launch for FakeLaunch {
        fn launch(&mut self) -> io::Result<HelperIo> {
            let n = {
                let mut l = self.log.launches.lock().unwrap();
                l.push(Instant::now());
                l.len() - 1
            };
            let script = (self.plan)(n);
            if let Some(kind) = script.launch_fails {
                return Err(kind.into());
            }
            let (to_helper, input) = pipe();
            let (output, from_helper) = pipe();
            let (kill_tx, kill_rx) = mpsc::channel::<()>();
            let log = Arc::clone(&self.log);
            std::thread::spawn(move || run_fake(n, script, input, output, kill_rx, log));
            let log = Arc::clone(&self.log);
            Ok(HelperIo {
                to_helper: Box::new(to_helper),
                from_helper: Box::new(from_helper),
                kill: Box::new(move || {
                    log.kills.fetch_add(1, Ordering::SeqCst);
                    let _ = kill_tx.send(());
                }),
            })
        }
    }

    fn run_fake(
        n: usize,
        script: Script,
        mut input: ChanReader,
        mut output: ChanWriter,
        kill: mpsc::Receiver<()>,
        log: Arc<Log>,
    ) {
        let mut grabs = 0usize;
        let act = |a: &Act, ok: Reply| match a {
            Act::Answer => Some(ok),
            Act::Fail(code) => Some(Reply::Error { code: code.clone(), message: "scripted failure".into() }),
            Act::Hang => None,
        };
        let end = loop {
            if kill.try_recv().is_ok() {
                break End::Killed;
            }
            let request = match read_request(&mut input) {
                Ok(Some(r)) => r,
                _ => break End::Eof,
            };
            log.requests.lock().unwrap().push((n, request.clone(), Instant::now()));
            let reply = match request {
                Request::Quit => break End::Quit,
                Request::List => act(
                    &script.list,
                    Reply::Devices(vec![DeviceInfo {
                        name: "Test pattern".into(),
                        index: 0,
                        sizes: vec![(640, 480), (1280, 720), (1920, 1080)],
                    }]),
                ),
                Request::Open { width, height, .. } => act(&script.open, Reply::Opened { width, height }),
                Request::Grab => {
                    grabs += 1;
                    let frame = Reply::Frame(vec![0xFF, 0xD8, grabs as u8]);
                    if grabs <= script.grab_ok { Some(frame) } else { act(&script.grab_then, frame) }
                }
                Request::Close => Some(Reply::Closed),
            };
            let Some(reply) = reply else {
                // Hung: only a kill (or the daemon dropping it) ends this.
                let _ = kill.recv();
                break End::Killed;
            };
            if write_reply(&mut output, &reply).is_err() {
                break End::WriteFailed;
            }
        };
        log.ends.lock().unwrap().push((n, end, Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{Act, End, FakeLaunch, Script};
    use super::*;
    use fermentool_camera::protocol::{ErrorCode, Request};
    use std::time::Instant;

    fn quick() -> Timings {
        Timings {
            request: Duration::from_secs(1),
            open: Duration::from_secs(1),
            in_use_retry: Duration::from_millis(1500),
            lost_retry: Duration::from_millis(500),
            respawn_first: Duration::from_millis(200),
            respawn_max: Duration::from_secs(1),
            ..Timings::default()
        }
    }

    fn want(width: u32, height: u32) -> CameraWant {
        CameraWant { device: "Test pattern".into(), index: 0, width, height }
    }

    fn wait_for(max: Duration, mut f: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + max;
        loop {
            if f() {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn connected(cam: &Camera) -> bool {
        matches!(cam.state(), CameraState::Connected { .. })
    }

    fn is_grab(r: &Request) -> bool {
        *r == Request::Grab
    }

    fn is_open(r: &Request) -> bool {
        matches!(r, Request::Open { .. })
    }

    #[test]
    fn connects_and_reports_connected() {
        let (launch, _) = FakeLaunch::healthy();
        let cam = Camera::spawn_with(launch, Some(want(640, 480)), quick());
        assert!(wait_for(Duration::from_secs(2), || matches!(
            cam.state(),
            CameraState::Connected { width: 640, height: 480, .. }
        )));
        cam.shutdown();
    }

    #[test]
    fn no_grab_without_viewers() {
        let (launch, log) = FakeLaunch::healthy();
        let cam = Camera::spawn_with(launch, Some(want(640, 480)), quick());
        assert!(wait_for(Duration::from_secs(2), || connected(&cam)));
        std::thread::sleep(Duration::from_secs(1));
        assert!(log.seen(is_grab).is_empty());
        assert!(cam.latest().is_none());
        cam.shutdown();
    }

    #[test]
    fn viewer_drives_live_grabs() {
        let (launch, log) = FakeLaunch::healthy();
        let cam = Camera::spawn_with(launch, Some(want(640, 480)), quick());
        assert!(wait_for(Duration::from_secs(2), || connected(&cam)));
        let t0 = Instant::now();
        let guard = cam.viewer();
        assert_eq!(cam.viewers(), 1);
        std::thread::sleep(Duration::from_millis(500));
        let first = cam.latest().expect("a frame within 500 ms").seq;
        std::thread::sleep(Duration::from_millis(500));
        let t1 = Instant::now();
        drop(guard);
        assert_eq!(cam.viewers(), 0);
        let grabs = log.seen(is_grab).iter().filter(|(_, at)| *at >= t0 && *at <= t1).count();
        assert!((4..=6).contains(&grabs), "{grabs} grabs in 1 s");
        assert!(cam.latest().unwrap().seq > first);
        // Without a viewer the grabs stop.
        std::thread::sleep(Duration::from_millis(300));
        let after = log.seen(is_grab).len();
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(log.seen(is_grab).len(), after);
        cam.shutdown();
    }

    #[test]
    fn wait_frame_after_returns_the_next_frame() {
        let (launch, _) = FakeLaunch::healthy();
        let cam = Camera::spawn_with(launch, Some(want(640, 480)), quick());
        assert!(wait_for(Duration::from_secs(2), || connected(&cam)));
        assert!(cam.wait_frame_after(0, Duration::from_millis(100)).is_none());
        let _guard = cam.viewer();
        let f = cam.wait_frame_after(0, Duration::from_secs(2)).expect("a frame");
        let g = cam.wait_frame_after(f.seq, Duration::from_secs(2)).expect("the next frame");
        assert!(g.seq > f.seq);
        cam.shutdown();
    }

    #[test]
    fn hung_helper_is_killed_and_respawned() {
        let t = quick();
        let (launch, log) = FakeLaunch::new(|n| {
            if n == 0 {
                Script { grab_ok: 1, grab_then: Act::Hang, ..Script::healthy() }
            } else {
                Script::healthy()
            }
        });
        let cam = Camera::spawn_with(launch, Some(want(640, 480)), t);
        assert!(wait_for(Duration::from_secs(2), || connected(&cam)));
        let _guard = cam.viewer();
        // The second grab of the first helper is the one it never answers.
        assert!(wait_for(Duration::from_secs(2), || log.seen(is_grab).len() >= 2));
        let hung_at = log.seen(is_grab)[1].1;
        let mut worst = Duration::ZERO;
        let respawned = wait_for(t.request + Duration::from_secs(2), || {
            let started = Instant::now();
            let _ = cam.state();
            worst = worst.max(started.elapsed());
            log.kills() >= 1 && log.launch_count() >= 2
        });
        assert!(respawned, "killed {} launched {}", log.kills(), log.launch_count());
        let launched_again = log.launches.lock().unwrap()[1];
        assert!(
            launched_again.duration_since(hung_at) <= t.request + Duration::from_secs(1),
            "respawned {:?} after the hang",
            launched_again.duration_since(hung_at)
        );
        assert!(worst <= Duration::from_millis(50), "state() took {worst:?}");
        assert!(wait_for(Duration::from_secs(3), || connected(&cam)));
        cam.shutdown();
    }

    #[test]
    fn grab_error_marks_lost_then_recovers() {
        let t = quick();
        let (launch, log) = FakeLaunch::new(|n| {
            if n == 0 {
                Script { grab_ok: 0, grab_then: Act::Fail(ErrorCode::Failed), ..Script::healthy() }
            } else {
                Script::healthy()
            }
        });
        let cam = Camera::spawn_with(launch, Some(want(640, 480)), t);
        let _guard = cam.viewer();
        assert!(wait_for(Duration::from_secs(2), || matches!(cam.state(), CameraState::Lost { .. })));
        let lost_at = Instant::now();
        assert!(wait_for(t.lost_retry + Duration::from_secs(1), || connected(&cam)));
        assert!(lost_at.elapsed() <= t.lost_retry + Duration::from_secs(1));
        // The second attempt ran on a fresh helper; the first was told to quit.
        assert_eq!(log.seen(is_open).iter().map(|(n, _)| *n).collect::<Vec<_>>(), vec![0, 1]);
        assert!(wait_for(Duration::from_secs(1), || log.end_of(0).is_some()));
        assert_eq!(log.end_of(0).unwrap().0, End::Quit);
        assert!(wait_for(Duration::from_secs(1), || cam.latest().is_some()));
        cam.shutdown();
    }

    #[test]
    fn in_use_is_reported_and_retried_slowly() {
        let t = quick();
        let (launch, log) = FakeLaunch::new(|_| Script { open: Act::Fail(ErrorCode::InUse), ..Script::healthy() });
        let cam = Camera::spawn_with(launch, Some(want(640, 480)), t);
        assert!(wait_for(Duration::from_secs(2), || matches!(cam.state(), CameraState::InUse { .. })));
        assert!(wait_for(t.in_use_retry + Duration::from_secs(1), || log.seen(is_open).len() >= 2));
        let opens = log.seen(is_open);
        assert!(opens[1].1.duration_since(opens[0].1) >= t.in_use_retry);
        assert_eq!((opens[0].0, opens[1].0), (0, 1), "each attempt on a fresh helper");
        assert!(matches!(cam.state(), CameraState::InUse { .. }));
        cam.shutdown();
    }

    #[test]
    fn in_use_at_a_grab_is_in_use_not_lost() {
        let t = quick();
        // Opens, gives two frames, then another program takes the webcam.
        let (launch, log) =
            FakeLaunch::new(|_| Script { grab_ok: 2, grab_then: Act::Fail(ErrorCode::InUse), ..Script::healthy() });
        let cam = Camera::spawn_with(launch, Some(want(640, 480)), t);
        assert!(wait_for(Duration::from_secs(2), || connected(&cam)));
        let _viewer = cam.viewer();
        assert!(wait_for(Duration::from_secs(3), || matches!(cam.state(), CameraState::InUse { .. })), "{:?}", cam.state());
        let in_use_at = Instant::now();
        // Retried at the in-use pace, not the lost one.
        std::thread::sleep(t.lost_retry * 2);
        assert_eq!(log.seen(is_open).len(), 1, "retried at the lost pace");
        assert!(matches!(cam.state(), CameraState::InUse { .. }));
        assert!(wait_for(t.in_use_retry + Duration::from_secs(1), || log.seen(is_open).len() >= 2));
        assert!(in_use_at.elapsed() >= t.in_use_retry - Duration::from_millis(100));
        cam.shutdown();
    }

    #[test]
    fn a_long_in_use_slows_its_retries_down() {
        // Bounds below allow for a busy PC (tests run at low priority beside
        // a live run): a gap is never shorter than asked, often longer.
        let t = Timings {
            in_use_retry: Duration::from_millis(60),
            in_use_max: Duration::from_millis(960),
            ..quick()
        };
        let (launch, log) = FakeLaunch::new(|_| Script { open: Act::Fail(ErrorCode::InUse), ..Script::healthy() });
        let cam = Camera::spawn_with(launch, Some(want(640, 480)), t);
        let fast = IN_USE_FAST_TRIES as usize;
        assert!(wait_for(Duration::from_secs(12), || log.seen(is_open).len() >= fast + 6));
        let at: Vec<Instant> = log.seen(is_open).iter().map(|(_, at)| *at).collect();
        let gaps: Vec<Duration> = at.windows(2).map(|w| w[1].duration_since(w[0])).collect();
        // The first tries come every in_use_retry, well under the max.
        for g in &gaps[..fast] {
            assert!(*g >= t.in_use_retry && *g < t.in_use_max / 2, "{gaps:?}");
        }
        // Then doubling, up to in_use_max, and no further.
        assert!(gaps[fast] >= t.in_use_retry * 2, "{gaps:?}");
        assert!(gaps[fast + 1] >= t.in_use_retry * 4, "{gaps:?}");
        assert!(gaps[fast + 2] >= t.in_use_retry * 8, "{gaps:?}");
        assert!(gaps[fast + 3] >= t.in_use_max, "{gaps:?}");
        assert!(gaps[fast + 4] >= t.in_use_max && gaps[fast + 4] < t.in_use_max * 2, "{gaps:?}");
        // A new want starts the fast pace again.
        let before = log.seen(is_open).len();
        cam.set_want(Some(want(1280, 720)));
        assert!(wait_for(Duration::from_secs(3), || log.seen(is_open).len() >= before + 3));
        let at: Vec<Instant> = log.seen(is_open).iter().map(|(_, at)| *at).collect();
        let g = at[before + 2].duration_since(at[before + 1]);
        assert!(g < t.in_use_max / 2, "not reset by a new want: {g:?}");
        cam.shutdown();
    }

    #[test]
    fn set_want_none_closes() {
        let (launch, log) = FakeLaunch::healthy();
        let cam = Camera::spawn_with(launch, Some(want(640, 480)), quick());
        assert!(wait_for(Duration::from_secs(2), || connected(&cam)));
        let t0 = Instant::now();
        cam.set_want(None);
        assert!(wait_for(Duration::from_millis(1500), || log.end_of(0).is_some() || log.kills() > 0));
        assert!(t0.elapsed() <= Duration::from_millis(1200), "released after {:?}", t0.elapsed());
        assert!(matches!(log.end_of(0), Some((End::Quit | End::Eof, _))), "{:?}", log.end_of(0));
        assert!(log.seen(|r| *r == Request::Close).is_empty(), "Close does not free a webcam");
        assert_eq!(cam.state(), CameraState::Off);
        cam.shutdown();
    }

    #[test]
    fn missing_helper_is_no_helper() {
        let t = quick();
        let (launch, log) =
            FakeLaunch::new(|_| Script { launch_fails: Some(io::ErrorKind::NotFound), ..Script::healthy() });
        let cam = Camera::spawn_with(launch, Some(want(640, 480)), t);
        assert!(wait_for(Duration::from_secs(1), || cam.state() == CameraState::NoHelper));
        // Backoff 200, 400, 800 ms: attempts at about 0, 0.2, 0.6 s, 1.4 s.
        std::thread::sleep(Duration::from_millis(1000));
        let at = log.launches.lock().unwrap().clone();
        assert!((2..=4).contains(&at.len()), "{} launches in 1 s", at.len());
        assert!(at[1].duration_since(at[0]) >= t.respawn_first);
        if at.len() >= 3 {
            assert!(at[2].duration_since(at[1]) >= t.respawn_first * 2);
        }
        assert_eq!(cam.state(), CameraState::NoHelper);
        cam.shutdown();
    }

    #[test]
    fn a_long_loss_slows_its_retries_down() {
        // Wide margins: on a busy PC (tests at low priority beside a run) a
        // 50 ms retry came 220 ms late.
        let t = Timings {
            lost_retry: Duration::from_millis(50),
            respawn_first: Duration::from_millis(500),
            respawn_max: Duration::from_millis(2000),
            ..quick()
        };
        let (unplugged, log) =
            FakeLaunch::new(|_| Script { open: Act::Fail(ErrorCode::NotFound), ..Script::healthy() });
        let cam = Camera::spawn_with(unplugged, Some(want(640, 480)), t);
        let fast = LOST_FAST_TRIES as usize;
        assert!(wait_for(Duration::from_secs(8), || log.seen(is_open).len() >= fast + 3));
        let at: Vec<Instant> = log.seen(is_open).iter().map(|(_, at)| *at).collect();
        let gaps: Vec<Duration> = at.windows(2).map(|w| w[1].duration_since(w[0])).collect();
        // The first tries come every LOST_RETRY: a replug is caught quickly.
        for g in &gaps[..fast] {
            assert!(*g >= t.lost_retry && *g < t.respawn_first, "{gaps:?}");
        }
        // Then the backoff, doubling.
        assert!(gaps[fast] >= t.respawn_first, "{gaps:?}");
        assert!(gaps[fast + 1] >= t.respawn_first * 2, "{gaps:?}");
        assert!(matches!(cam.state(), CameraState::Lost { .. }));
        cam.shutdown();
    }

    #[test]
    fn helper_errors_are_told_apart_from_chatter() {
        assert!(looks_like_error("camera list failed: no media foundation"));
        assert!(looks_like_error("fermentool-camera: Error reading frame"));
        assert!(looks_like_error("thread 'main' panicked at src/main.rs"));
        assert!(!looks_like_error("opened Integrated Webcam at 1280 x 720"));
    }

    #[cfg(windows)]
    #[test]
    fn helpers_join_a_job_that_dies_with_the_daemon() {
        use std::process::{Command, Stdio};
        let mut cmd = Command::new("ping");
        cmd.args(["-n", "30", "127.0.0.1"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let mut child = spawn_hidden(&mut cmd).unwrap();
        let inside = job::holds(&child);
        let kills = job::kills_on_close();
        let _ = child.kill();
        let _ = child.wait();
        assert!(inside, "the child is not in the daemon's job");
        assert!(kills, "the job does not kill its processes on close");
    }

    #[test]
    fn a_new_want_opens_on_a_fresh_helper() {
        let (launch, log) = FakeLaunch::healthy();
        let cam = Camera::spawn_with(launch, Some(want(640, 480)), quick());
        assert!(wait_for(Duration::from_secs(2), || connected(&cam)));
        cam.set_want(Some(want(1280, 720)));
        assert!(wait_for(Duration::from_secs(3), || matches!(
            cam.state(),
            CameraState::Connected { width: 1280, .. }
        )));
        assert_eq!(log.launch_count(), 2);
        assert_eq!(log.end_of(0).map(|e| e.0), Some(End::Quit));
        cam.shutdown();
    }

    #[test]
    fn devices_run_on_their_own_helper_and_are_cached() {
        let (launch, log) = FakeLaunch::healthy();
        let cam = Camera::spawn_with(launch, None, quick());
        let list = cam.devices(false).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Test pattern");
        assert_eq!(log.launch_count(), 1);
        assert_eq!(log.end_of(0).map(|e| e.0), Some(End::Quit));
        cam.devices(false).unwrap();
        assert_eq!(log.launch_count(), 1, "cached");
        cam.devices(true).unwrap();
        assert_eq!(log.launch_count(), 2, "refreshed");
        assert_eq!(cam.state(), CameraState::Off);
        cam.shutdown();
    }

    #[test]
    fn devices_on_a_hung_helper_give_up_in_time() {
        let t = quick();
        let (launch, log) = FakeLaunch::new(|_| Script { list: Act::Hang, ..Script::healthy() });
        let cam = Camera::spawn_with(launch, None, t);
        let started = Instant::now();
        let probe = cam.clone();
        let asked = std::thread::spawn(move || probe.devices(false));
        // Meanwhile the state answers at once.
        std::thread::sleep(Duration::from_millis(100));
        let s = Instant::now();
        assert_eq!(cam.state(), CameraState::Off);
        assert!(s.elapsed() <= Duration::from_millis(50));
        assert!(asked.join().unwrap().is_err());
        assert!(started.elapsed() <= t.request + Duration::from_millis(500), "{:?}", started.elapsed());
        assert_eq!(log.kills(), 1);
        cam.shutdown();
    }

    #[test]
    fn shutdown_quits_the_helper() {
        let (launch, log) = FakeLaunch::healthy();
        let cam = Camera::spawn_with(launch, Some(want(640, 480)), quick());
        assert!(wait_for(Duration::from_secs(2), || connected(&cam)));
        cam.shutdown();
        assert!(wait_for(Duration::from_millis(1500), || log.end_of(0).is_some()));
        assert_eq!(log.end_of(0).unwrap().0, End::Quit);
        assert_eq!(cam.state(), CameraState::Off);
    }

    /// Pids of the running `fermentool-camera.exe` processes (Windows).
    fn helper_pids() -> Vec<u32> {
        let out = std::process::Command::new("tasklist")
            .args(["/fi", "imagename eq fermentool-camera.exe", "/fo", "csv", "/nh"])
            .output()
            .expect("tasklist");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.split("\",\"").nth(1)?.parse().ok())
            .collect()
    }

    /// The real helper process on its test pattern (never the webcam): needs
    /// `cargo build -p fermentool-camera --features capture` first.
    /// `cargo test -p fermentool-core real_helper_end_to_end -- --ignored`
    #[test]
    #[ignore = "needs target/debug/fermentool-camera.exe built with --features capture"]
    fn real_helper_end_to_end() {
        let exe = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/fermentool-camera.exe");
        assert!(exe.is_file(), "build the helper first: {}", exe.display());
        let before = helper_pids();
        let cam = Camera::spawn(ProcessLaunch { exe, test_pattern: true }, Some(want(640, 480)));
        assert!(wait_for(Duration::from_secs(10), || connected(&cam)), "{:?}", cam.state());
        let ours: Vec<u32> = helper_pids().into_iter().filter(|p| !before.contains(p)).collect();
        assert_eq!(ours.len(), 1, "one new helper process: {ours:?}");
        let _viewer = cam.viewer();
        let frame = cam.wait_frame_after(0, Duration::from_secs(5)).expect("a frame");
        assert_eq!(&frame.jpeg[..2], &[0xFF, 0xD8], "a JPEG");
        eprintln!("frame {} bytes, helper pid {}", frame.jpeg.len(), ours[0]);
        cam.shutdown();
        assert!(
            wait_for(Duration::from_secs(2), || !helper_pids().contains(&ours[0])),
            "helper {} still running after shutdown",
            ours[0]
        );
    }
}
