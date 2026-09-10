//! Building the pump transport from configuration, and swapping it on a live
//! [`Engine`](crate::engine::Engine) without restarting the daemon.
//!
//! `build_engine` (in `main.rs`) and `POST /api/serial/reconnect` both go through
//! [`open`]: `serial.path = "sim"` (or empty) selects the in-process simulator;
//! anything else is a real serial port, with the same "log the error and fall
//! back to the simulator" behaviour the daemon has always had at boot.
//!
//! The daemon does not hold the opened transport directly, it holds a
//! [`WatchdogTransport`], which runs the real port on a dedicated worker thread.
//! A blocking `read()`/`write()` that never returns (a USB-serial adapter yanked
//! mid-transaction: the OS ignores the port's own timeout) then wedges only that
//! worker, not the control loop, so `/api/status` and *Stop* stay responsive
//! and the auto-recovery can spawn a fresh worker on a clean handle.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use fermentool_modbus::serial::SerialTransport;
use fermentool_modbus::{SimPump, Transport, TransportError};

use crate::config::SerialConfig;

/// A whole serial transaction is bounded by this; matches `build_engine`'s
/// historical value.
const OPEN_TIMEOUT: Duration = Duration::from_millis(1500);

/// The control thread waits at most this long for the serial worker to answer
/// one [`Transport::transaction`]. A healthy round trip is well under 300 ms
/// (200 ms read timeout + inter-frame gap); anything past this means the
/// worker's syscall is stuck, so we return [`TransportError::Timeout`] and let
/// the control loop's recovery take over instead of freezing the daemon.
pub(crate) const HARD_TXN_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a (re)spawn waits for the worker to report what it opened before
/// giving up and returning "unknown", the worker keeps trying in the
/// background and the next recovery pass picks it up.
pub(crate) const OPEN_WAIT: Duration = Duration::from_secs(3);

/// Which transport an engine is currently driving. Surfaced in the status frame
/// so the UI can show "Connected to: COM3" / "simulator".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportKind {
    /// The in-process register simulator.
    Sim,
    /// A real serial port, by OS name (e.g. `"COM3"`, `"/dev/ttyUSB0"`).
    Serial(String),
}

impl TransportKind {
    /// Short label for the status frame: `"sim"` or the port name.
    pub fn label(&self) -> String {
        match self {
            TransportKind::Sim => "sim".to_string(),
            TransportKind::Serial(name) => name.clone(),
        }
    }
}

/// Open the transport named by `serial`. Never fails: an unopenable port is
/// logged and the simulator is returned instead, so the daemon always comes up.
pub fn open(serial: &SerialConfig, pump_addr: u8) -> (Box<dyn Transport + Send>, TransportKind) {
    if serial.use_simulator() {
        tracing::warn!(
            "serial.path = \"{}\": using the pump SIMULATOR",
            serial.path
        );
        return (Box::new(SimPump::new(pump_addr)), TransportKind::Sim);
    }
    match SerialTransport::open(&serial.path, serial.baud, OPEN_TIMEOUT) {
        Ok(t) => {
            tracing::info!(port = %serial.path, baud = serial.baud, "serial port open");
            (Box::new(t), TransportKind::Serial(serial.path.clone()))
        }
        Err(e) => {
            tracing::error!(
                "cannot open {} ({e}); falling back to the SIMULATOR",
                serial.path
            );
            (Box::new(SimPump::new(pump_addr)), TransportKind::Sim)
        }
    }
}

/// A live engine transport that can be rebuilt in place from a [`SerialConfig`].
///
/// Implemented for the daemon's boxed transport (a real swap) and for [`SimPump`]
/// (a no-op, `control.rs`'s simulator-backed tests build an `Engine<SimPump>`).
pub trait SwapTransport {
    fn swap(&mut self, serial: &SerialConfig, pump_addr: u8) -> TransportKind;

    /// Like [`swap`](Self::swap) but for automatic recovery: it does **not**
    /// fall back to the simulator. On failure it leaves a safe no-op sink in
    /// place (so writes don't error-spam) and returns `None`, so the caller
    /// keeps retrying the real port instead of silently driving nothing.
    fn swap_strict(&mut self, serial: &SerialConfig, pump_addr: u8) -> Option<TransportKind>;
}

impl SwapTransport for Box<dyn Transport + Send> {
    fn swap(&mut self, serial: &SerialConfig, pump_addr: u8) -> TransportKind {
        // Drop the current transport *before* opening the new one: Windows
        // refuses a second handle to a COM port that this process already holds,
        // so reopening the same port (or any port while the old one is live)
        // would otherwise fail and silently fall back to the simulator.
        *self = Box::new(SimPump::new(pump_addr));
        let (new, kind) = open(serial, pump_addr);
        *self = new;
        kind
    }

    fn swap_strict(&mut self, serial: &SerialConfig, pump_addr: u8) -> Option<TransportKind> {
        if serial.use_simulator() {
            *self = Box::new(SimPump::new(pump_addr));
            return Some(TransportKind::Sim);
        }
        // release the old handle first (Windows), then try the real port
        *self = Box::new(SimPump::new(pump_addr));
        match fermentool_modbus::serial::SerialTransport::open(&serial.path, serial.baud, OPEN_TIMEOUT)
        {
            Ok(t) => {
                *self = Box::new(t);
                Some(TransportKind::Serial(serial.path.clone()))
            }
            // `*self` is now a SimPump no-op sink: the pump keeps its last real
            // setpoint, and the caller will retry.
            Err(_) => None,
        }
    }
}

impl SwapTransport for SimPump {
    fn swap(&mut self, _serial: &SerialConfig, _pump_addr: u8) -> TransportKind {
        TransportKind::Sim
    }
    fn swap_strict(&mut self, _serial: &SerialConfig, _pump_addr: u8) -> Option<TransportKind> {
        Some(TransportKind::Sim)
    }
}

// ---------------------------------------------------------------------------
// WatchdogTransport, run the real port on a worker thread so a stuck syscall
// can't freeze the control loop (and therefore the whole HTTP API).
// ---------------------------------------------------------------------------

/// What the caller thread asks the serial worker to do.
enum Job {
    /// One MODBUS round trip. The reply carries the transport's own result.
    Txn(Vec<u8>, mpsc::Sender<Result<Vec<u8>, TransportError>>),
    /// Close the port and end the worker.
    Shutdown,
}

/// Opens the real transport (this runs *on the worker thread*, so a blocking
/// open can't stall the caller either) and returns it plus what kind it is
/// (`None` = a real port was wanted but wouldn't open, a `SimPump` no-op sink
/// is in its place and the caller should keep retrying).
type Opener = Box<dyn FnOnce() -> (Box<dyn Transport + Send>, Option<TransportKind>) + Send>;

fn spawn_worker(opener: Opener) -> (mpsc::Sender<Job>, JoinHandle<()>, Option<TransportKind>) {
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (kind_tx, kind_rx) = mpsc::channel::<Option<TransportKind>>();

    let handle = thread::Builder::new()
        .name("serial-worker".into())
        .spawn(move || {
            let (mut real, kind) = opener();
            let _ = kind_tx.send(kind);
            // Blocks here on `real.transaction(..)`. If a `read()` never returns,
            // this thread stays parked until the OS finally errors the syscall
            // (device fully gone / re-enumerated); it then sees `job_rx` closed
            // and exits, dropping `real` (closing the port). A bounded leak,
            // one thread per wedge event.
            while let Ok(job) = job_rx.recv() {
                match job {
                    Job::Txn(req, reply) => {
                        let _ = reply.send(real.transaction(&req));
                    }
                    Job::Shutdown => break,
                }
            }
        })
        .expect("spawn serial-worker thread");

    let kind = match kind_rx.recv_timeout(OPEN_WAIT) {
        Ok(k) => k,
        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => None,
    };
    (job_tx, handle, kind)
}

/// A [`Transport`] whose blocking work happens on a separate thread, so the
/// caller (the daemon's control loop) is never stuck for more than
/// [`HARD_TXN_TIMEOUT`]. See the module docs.
pub struct WatchdogTransport {
    tx: mpsc::Sender<Job>,
    /// Handle to the *current* worker. Retired workers are detached (dropped),
    /// never `join`ed, one of them may be stuck in a syscall forever.
    _worker: JoinHandle<()>,
    /// `true` once a transaction timed out: the worker is presumed stuck, so
    /// further transactions fail fast until a swap respawns it.
    stuck: bool,
    serial: SerialConfig,
    pump_addr: u8,
}

impl WatchdogTransport {
    /// Boot-time constructor. Mirrors [`open`]: a real port that won't open
    /// falls back to the simulator, so the daemon always comes up.
    pub fn spawn(serial: &SerialConfig, pump_addr: u8) -> (Self, TransportKind) {
        let s = serial.clone();
        let (tx, worker, kind) =
            spawn_worker(Box::new(move || {
                let (t, k) = open(&s, pump_addr);
                (t, Some(k))
            }));
        let this = Self {
            tx,
            _worker: worker,
            stuck: false,
            serial: serial.clone(),
            pump_addr,
        };
        (this, kind.unwrap_or(TransportKind::Sim))
    }

    /// Drop the current worker (whatever state it is in) and start a fresh one
    /// on a clean handle.
    fn respawn(&mut self, opener: Opener) -> Option<TransportKind> {
        let (tx, worker, kind) = spawn_worker(opener);
        let old_tx = std::mem::replace(&mut self.tx, tx);
        let _ = old_tx.send(Job::Shutdown); // no-op if it's wedged; harmless if not
        drop(old_tx);
        // Detach the old worker: never block the daemon joining a stuck thread.
        let _old = std::mem::replace(&mut self._worker, worker);
        self.stuck = false;
        kind
    }

    #[cfg(test)]
    pub(crate) fn from_opener(opener: Opener) -> Self {
        let (tx, worker, _kind) = spawn_worker(opener);
        Self {
            tx,
            _worker: worker,
            stuck: false,
            serial: SerialConfig {
                path: "sim".into(),
                baud: 9600,
                allow_simulator: false,
            },
            pump_addr: 1,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_stuck(&self) -> bool {
        self.stuck
    }

    #[cfg(test)]
    pub(crate) fn respawn_with(&mut self, opener: Opener) {
        self.respawn(opener);
    }
}

impl Transport for WatchdogTransport {
    fn transaction(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportError> {
        if self.stuck {
            return Err(TransportError::Timeout);
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        if self.tx.send(Job::Txn(request.to_vec(), reply_tx)).is_err() {
            self.stuck = true;
            return Err(TransportError::Timeout);
        }
        match reply_rx.recv_timeout(HARD_TXN_TIMEOUT) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                self.stuck = true;
                Err(TransportError::Timeout)
            }
        }
    }
}

impl SwapTransport for WatchdogTransport {
    fn swap(&mut self, serial: &SerialConfig, pump_addr: u8) -> TransportKind {
        let s = serial.clone();
        let kind = self.respawn(Box::new(move || {
            let (t, k) = open(&s, pump_addr);
            (t, Some(k))
        }));
        self.serial = serial.clone();
        self.pump_addr = pump_addr;
        kind.unwrap_or(TransportKind::Sim)
    }

    fn swap_strict(&mut self, serial: &SerialConfig, pump_addr: u8) -> Option<TransportKind> {
        let s = serial.clone();
        let kind = self.respawn(Box::new(move || {
            if s.use_simulator() {
                return (
                    Box::new(SimPump::new(pump_addr)) as Box<dyn Transport + Send>,
                    Some(TransportKind::Sim),
                );
            }
            match SerialTransport::open(&s.path, s.baud, OPEN_TIMEOUT) {
                Ok(t) => (
                    Box::new(t) as Box<dyn Transport + Send>,
                    Some(TransportKind::Serial(s.path.clone())),
                ),
                // A SimPump no-op sink holds the last real setpoint; caller retries.
                Err(_) => (Box::new(SimPump::new(pump_addr)) as Box<dyn Transport + Send>, None),
            }
        }));
        self.serial = serial.clone();
        self.pump_addr = pump_addr;
        kind
    }
}

impl Drop for WatchdogTransport {
    fn drop(&mut self) {
        // Ask the worker to close the port. Don't join, on process shutdown a
        // stuck worker must not hold up exit; the OS reaps it.
        let _ = self.tx.send(Job::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(path: &str) -> SerialConfig {
        SerialConfig {
            path: path.to_string(),
            baud: 9600,
            allow_simulator: false,
        }
    }

    #[test]
    fn open_returns_the_simulator_for_sim_or_empty() {
        assert_eq!(open(&sc("sim"), 1).1, TransportKind::Sim);
        assert_eq!(open(&sc("SIM"), 1).1, TransportKind::Sim);
        assert_eq!(open(&sc(""), 1).1, TransportKind::Sim);
    }

    #[test]
    fn open_falls_back_to_the_simulator_when_the_port_cannot_open() {
        // A name that cannot resolve to a real port on any OS.
        let (_t, kind) = open(&sc("NOPE_NOT_A_REAL_PORT_99999"), 1);
        assert_eq!(kind, TransportKind::Sim);
    }

    #[test]
    fn box_swap_rebuilds_in_place_and_reports_the_kind() {
        let mut t: Box<dyn Transport + Send> = Box::new(SimPump::new(1));
        assert_eq!(t.swap(&sc("sim"), 1), TransportKind::Sim);
        assert_eq!(t.swap(&sc("NOPE_NOT_A_REAL_PORT_99999"), 1), TransportKind::Sim);
    }

    #[test]
    fn simpump_swap_is_a_noop() {
        let mut p = SimPump::new(1);
        assert_eq!(p.swap(&sc("COM3"), 1), TransportKind::Sim);
    }

    #[test]
    fn swap_strict_errors_instead_of_falling_back_to_sim() {
        let mut t: Box<dyn Transport + Send> = Box::new(SimPump::new(1));
        // A dead port returns None (auto-recovery keeps retrying) rather than
        // silently becoming the simulator.
        assert!(t.swap_strict(&sc("NOPE_NOT_A_REAL_PORT_99999"), 1).is_none());
        // The simulator is always available.
        assert_eq!(t.swap_strict(&sc("sim"), 1).unwrap(), TransportKind::Sim);
    }

    #[test]
    fn label_is_sim_or_the_port_name() {
        assert_eq!(TransportKind::Sim.label(), "sim");
        assert_eq!(TransportKind::Serial("COM3".to_string()).label(), "COM3");
    }

    #[test]
    fn serial_config_use_simulator() {
        assert!(sc("sim").use_simulator());
        assert!(sc("").use_simulator());
        assert!(!sc("COM3").use_simulator());
    }

    // --- WatchdogTransport --------------------------------------------------

    use std::time::Instant;

    /// A `read()` that never returns: models the OS ignoring the port timeout
    /// after a USB-serial adapter is surprise-removed mid-transaction.
    struct HangingTransport;
    impl Transport for HangingTransport {
        fn transaction(&mut self, _req: &[u8]) -> Result<Vec<u8>, TransportError> {
            loop {
                thread::sleep(Duration::from_secs(3600));
            }
        }
    }

    /// Echoes the request back, a trivially healthy transport.
    struct EchoTransport;
    impl Transport for EchoTransport {
        fn transaction(&mut self, req: &[u8]) -> Result<Vec<u8>, TransportError> {
            Ok(req.to_vec())
        }
    }

    #[test]
    fn watchdog_passes_a_healthy_transport_through() {
        let mut wd = WatchdogTransport::from_opener(Box::new(|| (Box::new(EchoTransport), None)));
        assert_eq!(wd.transaction(b"ping").unwrap(), b"ping");
        assert!(!wd.is_stuck());
    }

    #[test]
    fn watchdog_transaction_times_out_instead_of_blocking_forever() {
        let mut wd = WatchdogTransport::from_opener(Box::new(|| (Box::new(HangingTransport), None)));
        let t0 = Instant::now();
        assert!(matches!(wd.transaction(b"x"), Err(TransportError::Timeout)));
        let dt = t0.elapsed();
        assert!(dt >= HARD_TXN_TIMEOUT - Duration::from_millis(100), "returned too early: {dt:?}");
        assert!(dt < HARD_TXN_TIMEOUT + Duration::from_millis(500), "blocked past the deadline: {dt:?}");
        // Subsequent calls fail fast, no piling onto the wedged worker.
        assert!(wd.is_stuck());
        let t1 = Instant::now();
        assert!(wd.transaction(b"y").is_err());
        assert!(t1.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn watchdog_recovers_on_respawn_after_a_wedge() {
        let mut wd = WatchdogTransport::from_opener(Box::new(|| (Box::new(HangingTransport), None)));
        assert!(wd.transaction(b"x").is_err());
        assert!(wd.is_stuck());
        // Auto-recovery / operator reconnect swaps in a fresh worker.
        wd.respawn_with(Box::new(|| (Box::new(EchoTransport), Some(TransportKind::Sim))));
        assert!(!wd.is_stuck());
        assert_eq!(wd.transaction(b"back").unwrap(), b"back");
    }

    #[test]
    fn watchdog_swap_to_sim_works_after_a_wedge() {
        let mut wd = WatchdogTransport::from_opener(Box::new(|| (Box::new(HangingTransport), None)));
        assert!(wd.transaction(b"x").is_err());
        assert_eq!(wd.swap(&sc("sim"), 1), TransportKind::Sim);
        assert!(!wd.is_stuck());
        assert_eq!(wd.swap_strict(&sc("sim"), 1), Some(TransportKind::Sim));
    }
}
