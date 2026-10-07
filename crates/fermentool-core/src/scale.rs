//! MT-SICS balance link. Confirmed against a real Ohaus Ranger 7000:
//! `S`/`SI` -> `S <status> <value> g\r\n`, status S = stable, D = dynamic.
//!
//! Deliberately NOT built on fermentool_modbus::serial::SerialTransport: that
//! type hardcodes 8E1 framing and reads a caller-known fixed byte count,
//! neither of which fits a variable-length ASCII line. This driver reads a
//! variable-length line instead, but its framing is fixed at 8-N-1 (the tested
//! Ranger 7000's default); a balance set to e.g. 7-E-1 cannot talk to
//! Fermentool yet, `ScaleConfig` would need `parity`/`data_bits` fields.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use fermentool_modbus::{Transport, TransportError};
use serialport::{DataBits, Parity, StopBits};

use crate::config::ScaleConfig;
use crate::transport::OPEN_WAIT;

pub const SICS_IMMEDIATE: &[u8] = b"SI\r\n";

/// Bound on one scale transaction, mirrors the pump's `OPEN_TIMEOUT`.
pub const SCALE_OPEN_TIMEOUT: Duration = Duration::from_millis(1500);

/// How often the poll thread asks the balance for a weight. The engine reads
/// about once a second, so a reading is at most this old when it is used.
pub const SCALE_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// A result older than this is not handed out: the poll thread is stuck in a
/// syscall, and the caller must see a miss, not the last weight forever.
const SCALE_MAX_AGE: Duration = Duration::from_millis(2500);

/// Open the configured scale behind its own poll thread ([`PolledScale`]).
/// `None` when the port does not open within [`OPEN_WAIT`]: unlike the pump
/// there is no sensible simulated scale, so a scale that isn't really there
/// must look absent, not like a silently broken stand-in. Shared by boot
/// (`main.rs`) and `Engine::recover_scale`.
pub fn open_polled(cfg: &ScaleConfig) -> Option<PolledScale> {
    let path = cfg.path.clone();
    let baud = cfg.baud;
    PolledScale::spawn(
        move || {
            SicsScale::open(&path, baud, SCALE_OPEN_TIMEOUT)
                .map(|s| Box::new(s) as Box<dyn Transport + Send>)
        },
        SCALE_POLL_INTERVAL,
    )
}

/// The balance, read by a thread of its own. The thread asks for a weight
/// every [`SCALE_POLL_INTERVAL`] and keeps the latest answer (or failure);
/// [`Transport::transaction`] hands that answer out at once. A silent balance
/// costs each SICS read its full timeout (1.5 s): done on the control thread,
/// that held up every command behind it (the UI's History and Calibration
/// pages took seconds to load whenever the balance was off). Here it only
/// ever holds up this thread. A read that never returns (adapter yanked
/// mid-syscall) leaves the thread parked; the stale-age check turns that into
/// misses, and the link is dropped and reopened on a fresh thread.
pub struct PolledScale {
    shared: Arc<Shared>,
}

struct Shared {
    latest: Mutex<Option<(Instant, Result<Vec<u8>, TransportError>)>>,
    stop: AtomicBool,
}

impl PolledScale {
    /// Run `open` on a new thread, then poll what it opened. Waits at most
    /// [`OPEN_WAIT`] for the open itself; `None` if it failed or took longer
    /// (the thread then gives up on its own).
    pub fn spawn<F>(open: F, interval: Duration) -> Option<Self>
    where
        F: FnOnce() -> Result<Box<dyn Transport + Send>, TransportError> + Send + 'static,
    {
        let shared = Arc::new(Shared { latest: Mutex::new(None), stop: AtomicBool::new(false) });
        let (opened_tx, opened_rx) = mpsc::channel::<bool>();
        let worker = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("scale-poll".into())
            .spawn(move || {
                let Ok(mut port) = open() else {
                    let _ = opened_tx.send(false);
                    return;
                };
                if opened_tx.send(true).is_err() || worker.stop.load(Ordering::SeqCst) {
                    return; // the caller stopped waiting: nobody reads this link
                }
                while !worker.stop.load(Ordering::SeqCst) {
                    let started = Instant::now();
                    let reply = port.transaction(SICS_IMMEDIATE);
                    *worker.latest.lock().unwrap_or_else(|p| p.into_inner()) = Some((Instant::now(), reply));
                    // Sleep out the interval in short steps, so a dropped link
                    // lets go of its COM port promptly.
                    while !worker.stop.load(Ordering::SeqCst) && started.elapsed() < interval {
                        std::thread::sleep(Duration::from_millis(20).min(interval));
                    }
                }
                // `port` drops here: the COM port is closed.
            });
        if spawned.is_err() {
            return None;
        }
        match opened_rx.recv_timeout(OPEN_WAIT) {
            Ok(true) => Some(Self { shared }),
            _ => {
                shared.stop.store(true, Ordering::SeqCst);
                None
            }
        }
    }
}

impl Transport for PolledScale {
    /// The latest weight the poll thread got, without waiting for the balance.
    /// Only the weight query is served: nothing else is ever sent. A silent
    /// balance is a `Timeout` (the port is fine, keep it: the thread sees the
    /// balance come back); a thread stuck past [`SCALE_MAX_AGE`] is an `Io`
    /// error, like a broken port: the link must be reopened.
    fn transaction(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportError> {
        if request != SICS_IMMEDIATE {
            return Err(TransportError::Io("the polled scale only answers weight queries".into()));
        }
        match &*self.shared.latest.lock().unwrap_or_else(|p| p.into_inner()) {
            Some((at, reply)) if at.elapsed() <= SCALE_MAX_AGE => reply.clone(),
            Some(_) => Err(TransportError::Io("balance poll thread stuck".into())),
            // Opened, first answer not in yet.
            None => Err(TransportError::Timeout),
        }
    }
}

impl Drop for PolledScale {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }
}

pub struct SicsScale {
    port: Box<dyn serialport::SerialPort>,
    timeout: Duration,
}

impl SicsScale {
    pub fn open(path: &str, baud: u32, timeout: Duration) -> Result<Self, TransportError> {
        let port = serialport::new(path, baud)
            .data_bits(DataBits::Eight)
            .parity(Parity::None)
            .stop_bits(StopBits::One)
            .timeout(Duration::from_millis(200))
            .open()
            .map_err(|e| TransportError::Io(e.to_string()))?;
        Ok(Self { port, timeout })
    }
}

impl Transport for SicsScale {
    fn transaction(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportError> {
        self.port
            .clear(serialport::ClearBuffer::Input)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        self.port
            .write_all(request)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        self.port
            .flush()
            .map_err(|e| TransportError::Io(e.to_string()))?;

        let deadline = Instant::now() + self.timeout;
        let mut buf = Vec::with_capacity(32);
        let mut chunk = [0u8; 64];
        loop {
            if buf.ends_with(b"\r\n") {
                return Ok(buf);
            }
            if Instant::now() >= deadline {
                return Err(TransportError::Timeout);
            }
            match self.port.read(&mut chunk) {
                Ok(0) => return Err(TransportError::Timeout),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(TransportError::Io(e.to_string())),
            }
        }
    }
}

#[derive(Debug)]
pub enum ScaleError {
    Transport(TransportError),
    Parse(String),
}

impl std::fmt::Display for ScaleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScaleError::Transport(e) => write!(f, "scale: {e}"),
            ScaleError::Parse(s) => write!(f, "scale: could not parse reply: {s}"),
        }
    }
}
impl std::error::Error for ScaleError {}

impl From<TransportError> for ScaleError {
    fn from(e: TransportError) -> Self {
        ScaleError::Transport(e)
    }
}

/// One MT-SICS weight reply.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SicsReading {
    pub weight_g: f64,
    pub stable: bool,
    /// The display step, read from the number's decimals (`740.5` -> 0.1 g).
    pub resolution_g: f64,
}

/// Parse a MT-SICS "S"/"SI" reply: `"S <status> <value> <unit>\r\n"`.
pub fn parse_sics_weight(reply: &[u8]) -> Result<SicsReading, ScaleError> {
    let text = std::str::from_utf8(reply)
        .map_err(|e| ScaleError::Parse(e.to_string()))?
        .trim();
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() < 4 {
        return Err(ScaleError::Parse(format!("too few fields: {text:?}")));
    }
    let stable = match parts[1] {
        "S" => true,
        "D" => false,
        other => return Err(ScaleError::Parse(format!("unexpected status {other:?}"))),
    };
    let value: f64 = parts[2]
        .parse()
        .map_err(|_| ScaleError::Parse(format!("bad numeric value: {:?}", parts[2])))?;
    // Rust's f64 parser accepts "nan"/"inf"/"infinity" (case-insensitive) as
    // valid floats. A garbled reply that happens to parse to one of those
    // would otherwise poison weight_buffer and panic theil_sen_slope's
    // partial_cmp on the first NaN comparison.
    if !value.is_finite() {
        return Err(ScaleError::Parse(format!("non-finite value: {value}")));
    }
    // The balance's Communications menu can be set to kg; silently accepting
    // that would feed a value 1000x too small straight into the control loop.
    if parts[3] != "g" {
        return Err(ScaleError::Parse(format!(
            "unexpected unit {:?}, expected \"g\"",
            parts[3]
        )));
    }
    let decimals = parts[2].split_once('.').map_or(0, |(_, frac)| frac.len());
    Ok(SicsReading {
        weight_g: value,
        stable,
        resolution_g: 10f64.powi(-(decimals as i32)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_stable_reply() {
        let r = parse_sics_weight(b"S S      740.5 g\r\n").unwrap();
        assert!((r.weight_g - 740.5).abs() < 1e-9);
        assert!(r.stable);
    }

    #[test]
    fn parses_a_dynamic_reply() {
        let r = parse_sics_weight(b"S D      740.6 g\r\n").unwrap();
        assert!((r.weight_g - 740.6).abs() < 1e-9);
        assert!(!r.stable);
    }

    #[test]
    fn reads_the_resolution_from_the_decimals() {
        assert_eq!(parse_sics_weight(b"S S      740.5 g\r\n").unwrap().resolution_g, 0.1);
        assert_eq!(parse_sics_weight(b"S S     740.25 g\r\n").unwrap().resolution_g, 0.01);
        assert_eq!(parse_sics_weight(b"S S        740 g\r\n").unwrap().resolution_g, 1.0);
    }

    #[test]
    fn rejects_a_malformed_reply() {
        assert!(parse_sics_weight(b"garbage\r\n").is_err());
        assert!(parse_sics_weight(b"").is_err());
    }

    #[test]
    fn rejects_a_non_gram_unit() {
        // A balance left in kg would otherwise read 1000x light with no error.
        assert!(parse_sics_weight(b"S S      0.740 kg
").is_err());
        // A reply with no unit at all is not trustworthy either.
        assert!(parse_sics_weight(b"S S      740.5
").is_err());
    }

    #[test]
    fn rejects_non_finite_values() {
        // Rust's f64 parser accepts these tokens; the scale never sends them,
        // but a garbled line must not be allowed to poison downstream math.
        assert!(parse_sics_weight(b"S S      NaN g
").is_err());
        assert!(parse_sics_weight(b"S S      inf g
").is_err());
        assert!(parse_sics_weight(b"S S      -inf g
").is_err());
    }

    #[test]
    fn rejects_an_overload_reply() {
        assert!(parse_sics_weight(b"S +\r\n").is_err());
    }

    // ---- the poll thread ----

    /// Answers each read after `delay`, with a weight one gram lighter each
    /// time; flags when it is dropped (the port closed).
    struct FakeBalance {
        delay: Duration,
        weight: f64,
        dropped: Arc<AtomicBool>,
    }
    impl Transport for FakeBalance {
        fn transaction(&mut self, _: &[u8]) -> Result<Vec<u8>, TransportError> {
            std::thread::sleep(self.delay);
            self.weight -= 1.0;
            Ok(format!("S S {:.1} g\r\n", self.weight).into_bytes())
        }
    }
    impl Drop for FakeBalance {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    fn fake(delay: Duration, dropped: &Arc<AtomicBool>) -> Option<PolledScale> {
        let dropped = Arc::clone(dropped);
        PolledScale::spawn(
            move || Ok(Box::new(FakeBalance { delay, weight: 1000.0, dropped }) as Box<dyn Transport + Send>),
            Duration::from_millis(10),
        )
    }

    fn wait_until(mut ok: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn hands_out_the_latest_weight() {
        let dropped = Arc::new(AtomicBool::new(false));
        let mut s = fake(Duration::ZERO, &dropped).unwrap();
        assert!(wait_until(|| s.transaction(SICS_IMMEDIATE).is_ok()));
        let first = parse_sics_weight(&s.transaction(SICS_IMMEDIATE).unwrap()).unwrap().weight_g;
        assert!(wait_until(|| {
            parse_sics_weight(&s.transaction(SICS_IMMEDIATE).unwrap()).unwrap().weight_g < first
        }));
    }

    #[test]
    fn a_silent_balance_never_holds_up_the_caller() {
        // Each read of this balance takes a full second: the caller still
        // gets its answer (a miss) straight away.
        let dropped = Arc::new(AtomicBool::new(false));
        let mut s = fake(Duration::from_secs(1), &dropped).unwrap();
        let t = Instant::now();
        assert_eq!(s.transaction(SICS_IMMEDIATE), Err(TransportError::Timeout));
        assert!(t.elapsed() < Duration::from_millis(50), "{:?}", t.elapsed());
    }

    #[test]
    fn a_stuck_poll_thread_reads_as_a_broken_link() {
        let dropped = Arc::new(AtomicBool::new(false));
        let mut s = fake(Duration::ZERO, &dropped).unwrap();
        assert!(wait_until(|| s.transaction(SICS_IMMEDIATE).is_ok()));
        // The poll thread wedged long ago: its last answer is not the weight now.
        s.shared.stop.store(true, Ordering::SeqCst);
        assert!(wait_until(|| dropped.load(Ordering::SeqCst)));
        *s.shared.latest.lock().unwrap() =
            Some((Instant::now() - SCALE_MAX_AGE - Duration::from_millis(1), Ok(b"S S 1.0 g\r\n".to_vec())));
        // Not the last weight, and not a mere timeout either: reopen it.
        assert!(matches!(s.transaction(SICS_IMMEDIATE), Err(TransportError::Io(_))));
    }

    #[test]
    fn dropping_the_link_closes_the_port() {
        let dropped = Arc::new(AtomicBool::new(false));
        let s = fake(Duration::ZERO, &dropped).unwrap();
        drop(s);
        assert!(wait_until(|| dropped.load(Ordering::SeqCst)), "the port was never closed");
    }

    #[test]
    fn a_port_that_will_not_open_is_no_link() {
        let s = PolledScale::spawn(|| Err(TransportError::Io("no such port".into())), SCALE_POLL_INTERVAL);
        assert!(s.is_none());
    }

    #[test]
    fn only_weight_queries_are_served() {
        let dropped = Arc::new(AtomicBool::new(false));
        let mut s = fake(Duration::ZERO, &dropped).unwrap();
        assert!(s.transaction(b"Z\r\n").is_err());
    }
}
