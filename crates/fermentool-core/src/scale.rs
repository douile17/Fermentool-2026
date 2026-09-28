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
use std::time::{Duration, Instant};

use fermentool_modbus::{Transport, TransportError};
use serialport::{DataBits, Parity, StopBits};

use crate::config::ScaleConfig;
use crate::transport::{TransportKind, WatchdogTransport};

pub const SICS_IMMEDIATE: &[u8] = b"SI\r\n";

/// Bound on one scale transaction, mirrors the pump's `OPEN_TIMEOUT`.
pub const SCALE_OPEN_TIMEOUT: Duration = Duration::from_millis(1500);

/// Open the configured scale on its own watchdog worker, like the pump's
/// port, so a wedged balance read only ever blocks that worker. `None` when
/// the port does not open: unlike the pump there is no sensible simulated
/// scale, so a scale that isn't really there must look absent, not like a
/// silently broken stand-in. Shared by boot (`main.rs`) and
/// `Engine::recover_scale`.
pub fn open_watchdogged(cfg: &ScaleConfig) -> Option<WatchdogTransport> {
    let path = cfg.path.clone();
    let baud = cfg.baud;
    let (watchdog, kind) = WatchdogTransport::spawn_with(Box::new(move || {
        match SicsScale::open(&path, baud, SCALE_OPEN_TIMEOUT) {
            Ok(s) => (Box::new(s) as Box<dyn Transport + Send>, Some(TransportKind::Serial(path))),
            Err(_) => (Box::new(fermentool_modbus::SimPump::new(1)) as Box<dyn Transport + Send>, None),
        }
    }));
    kind.map(|_| watchdog)
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

/// Parse a MT-SICS "S"/"SI" reply: `"S <status> <value> <unit>\r\n"`.
pub fn parse_sics_weight(reply: &[u8]) -> Result<(f64, bool), ScaleError> {
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
    Ok((value, stable))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_stable_reply() {
        let (w, stable) = parse_sics_weight(b"S S      740.5 g\r\n").unwrap();
        assert!((w - 740.5).abs() < 1e-9);
        assert!(stable);
    }

    #[test]
    fn parses_a_dynamic_reply() {
        let (w, stable) = parse_sics_weight(b"S D      740.6 g\r\n").unwrap();
        assert!((w - 740.6).abs() < 1e-9);
        assert!(!stable);
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
}
