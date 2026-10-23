//! MODBUS-RTU support for the Baoding Shenchen **LabQ**, **LabV** and the
//! Innofluid **PDS II** peristaltic pumps (see [`PumpKind`] and [`RegisterMap`]).
//!
//! Layers, bottom to top:
//!
//! * [`crc16`] / [`with_crc`] / [`f32_be`], wire primitives, checked byte-for-byte
//!   against the vendor examples in `LabQ Series MODBUS protocol.md`.
//! * [`frame`], pure request builders and response parsers (`Vec<u8>` in/out,
//!   no I/O).
//! * [`Transport`], a blocking request→response byte channel. Implemented by
//!   [`SimPump`] (a full register-level simulator with fault injection) and, with
//!   the `serial` feature, [`serial::SerialTransport`] over a real RS-485 adapter.
//! * [`PumpTransport`] / [`Pump`], pump-level operations (set speed, start, …)
//!   built on any [`Transport`].
//!
//! The control path is deliberately **synchronous**: one transaction every few
//! seconds, run on a dedicated thread, is simpler and steadier for a 100 h job
//! than an async stack (`docs/IMPLEMENTATION_PLAN.md` §4.3).

/// LabQ holding-register addresses (decimal). The tests and the simulator use
/// these; the daemon drives a pump through its [`RegisterMap`] instead.
pub mod reg {
    /// Pump head type, `u16`, function code `06H`. See Chart 1 (KT15 = 0).
    pub const PUMP_HEAD_TYPE: u16 = 1000;
    /// Tubing size, `u16`, `06H`.
    pub const TUBING_SIZE: u16 = 1001;
    /// Motor speed, 0.1..=350 rpm, `f32` (2 registers), function code `10H`.
    pub const MOTOR_SPEED: u16 = 1002;
    /// Flow rate, 0..=99999 ml/min, `f32` (2 registers), `10H`.
    pub const FLOW_RATE: u16 = 1004;
    /// Start/stop, `u16`, `06H`. `1` = start, `0` = stop.
    pub const START_STOP: u16 = 1006;
    /// Direction, `u16`, `06H`. `1` = clockwise, `0` = counter-clockwise.
    pub const DIRECTION: u16 = 1007;
    /// Full-speed run, `u16`, `06H`. `1` = start, `0` = stop.
    pub const FULL_SPEED: u16 = 1008;
    /// Back-suction angle, 0..=360°, `u16`, `06H`.
    pub const BACK_SUCTION_ANGLE: u16 = 1009;
}

/// Setpoint limits, for the engine to intersect with a run's own clamps.
pub mod limits {
    /// Minimum settable motor speed (rpm).
    pub const RPM_MIN: f64 = 0.1;
    // Temporary: Task 4 removes it (the maximum is the pump's own `max_rpm`).
    /// Maximum settable motor speed of a LabQ (rpm).
    pub const RPM_MAX: f64 = 350.0;
    /// Minimum settable flow rate (ml/min).
    pub const FLOW_MIN: f64 = 0.0;
    /// Maximum settable flow rate (ml/min).
    pub const FLOW_MAX: f64 = 99_999.0;
}

/// MODBUS CRC-16 (polynomial `0xA001`, initial value `0xFFFF`).
///
/// The returned word is transmitted **low byte first, then high byte**
/// (see [`with_crc`]).
pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in bytes {
        crc ^= b as u16;
        for _ in 0..8 {
            if crc & 0x0001 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

/// Append the CRC to a frame body: low byte, then high byte.
pub fn with_crc(mut frame: Vec<u8>) -> Vec<u8> {
    let crc = crc16(&frame);
    frame.push((crc & 0x00FF) as u8);
    frame.push((crc >> 8) as u8);
    frame
}

/// Encode an `f32` the way the pump expects for function code `10H`:
/// IEEE-754, big-endian byte order (`8.9` → `41 0E 66 66`).
pub fn f32_be(value: f32) -> [u8; 4] {
    value.to_be_bytes()
}

/// Decode a big-endian IEEE-754 `f32` from a 4-byte register pair.
pub fn f32_from_be(bytes: [u8; 4]) -> f32 {
    f32::from_be_bytes(bytes)
}

/// Split an `f32` into two big-endian holding-register words `(high, low)`.
pub fn f32_to_words(value: f32) -> (u16, u16) {
    let b = value.to_be_bytes();
    (
        u16::from_be_bytes([b[0], b[1]]),
        u16::from_be_bytes([b[2], b[3]]),
    )
}

/// Reassemble an `f32` from two big-endian holding-register words.
pub fn f32_from_words(high: u16, low: u16) -> f32 {
    let h = high.to_be_bytes();
    let l = low.to_be_bytes();
    f32::from_be_bytes([h[0], h[1], l[0], l[1]])
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A malformed or negative MODBUS response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PduError {
    /// Fewer bytes than the smallest valid frame.
    TooShort,
    /// CRC did not match the body.
    BadCrc,
    /// Response function byte was neither the request's nor its exception form.
    UnexpectedFunction(u8),
    /// Device returned an exception response; payload is the exception code.
    Exception(u8),
    /// Right length and CRC, but the fields don't match what was requested.
    MalformedResponse,
}

/// Human-readable name for a MODBUS exception code, so a log line reads
/// "0x03 (illegal data value)" instead of a bare number the operator has to
/// look up. Codes per the MODBUS Application Protocol spec, §7.
pub fn exception_name(code: u8) -> &'static str {
    match code {
        0x01 => "illegal function",
        0x02 => "illegal data address",
        0x03 => "illegal data value",
        0x04 => "device failure",
        0x05 => "acknowledge (long operation in progress)",
        0x06 => "device busy",
        0x08 => "memory parity error",
        0x0A => "gateway path unavailable",
        0x0B => "gateway target device failed to respond",
        _ => "unknown",
    }
}

impl core::fmt::Display for PduError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PduError::TooShort => write!(f, "response too short"),
            PduError::BadCrc => write!(f, "response CRC mismatch"),
            PduError::UnexpectedFunction(x) => write!(f, "unexpected function byte 0x{x:02X}"),
            PduError::Exception(c) => {
                write!(f, "MODBUS exception 0x{c:02X} ({})", exception_name(*c))
            }
            PduError::MalformedResponse => write!(f, "malformed response"),
        }
    }
}

impl std::error::Error for PduError {}

/// A failure of the underlying byte channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// No (complete) response within the timeout.
    Timeout,
    /// Underlying I/O error.
    Io(String),
    /// The channel is not open.
    Closed,
}

impl core::fmt::Display for TransportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            TransportError::Timeout => write!(f, "serial transaction timed out"),
            TransportError::Io(e) => write!(f, "serial I/O error: {e}"),
            TransportError::Closed => write!(f, "serial channel is closed"),
        }
    }
}

impl std::error::Error for TransportError {}

/// Anything that can go wrong performing a pump operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PumpError {
    Pdu(PduError),
    Transport(TransportError),
    /// A caller-supplied setpoint was outside the pump's accepted range.
    OutOfRange(String),
}

impl core::fmt::Display for PumpError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PumpError::Pdu(e) => write!(f, "{e}"),
            PumpError::Transport(e) => write!(f, "{e}"),
            PumpError::OutOfRange(w) => write!(f, "value out of range: {w}"),
        }
    }
}

impl std::error::Error for PumpError {}

impl From<PduError> for PumpError {
    fn from(e: PduError) -> Self {
        PumpError::Pdu(e)
    }
}

impl From<TransportError> for PumpError {
    fn from(e: TransportError) -> Self {
        PumpError::Transport(e)
    }
}

// ---------------------------------------------------------------------------
// Pump models and register maps
// ---------------------------------------------------------------------------

/// The "transferring" value of a pump's operating-mode register: the only mode
/// in which speed and direction can be written.
pub const TRANSFERRING: u16 = 0;

/// A pump family, as the operator chooses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpModel {
    LabQ,
    LabV,
    PdsII,
}

impl PumpModel {
    /// Stable key used in `config.toml` and the API.
    pub fn as_str(self) -> &'static str {
        match self {
            PumpModel::LabQ => "labq",
            PumpModel::LabV => "labv",
            PumpModel::PdsII => "pds2",
        }
    }

    pub fn parse(s: &str) -> Option<PumpModel> {
        match s {
            "labq" => Some(PumpModel::LabQ),
            "labv" => Some(PumpModel::LabV),
            "pds2" => Some(PumpModel::PdsII),
            _ => None,
        }
    }

    /// The maximum speed assumed when the operator does not set one (rpm).
    pub fn default_max_rpm(self) -> f64 {
        match self {
            PumpModel::LabQ => 350.0,
            PumpModel::LabV => 150.0,
            PumpModel::PdsII => 600.0,
        }
    }

    /// The highest `max_rpm` an operator may configure (rpm).
    pub fn max_rpm_ceiling(self) -> f64 {
        match self {
            PumpModel::LabQ => 350.0,
            PumpModel::LabV | PumpModel::PdsII => 600.0,
        }
    }

    /// The highest MODBUS slave address.
    pub fn max_address(self) -> u8 {
        match self {
            PumpModel::LabQ => 247,
            PumpModel::LabV | PumpModel::PdsII => 32,
        }
    }

    /// Name shown to the operator.
    pub fn label(self) -> &'static str {
        match self {
            PumpModel::LabQ => "LabQ",
            PumpModel::LabV => "LabV",
            PumpModel::PdsII => "PDS II",
        }
    }
}

/// A model plus, for a PDS II, which of its heads (1..=8) is meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpKind {
    LabQ,
    LabV,
    PdsII { head: u8 },
}

impl PumpKind {
    /// `head` (1..=8) is checked for a PDS II and ignored for the others.
    pub fn new(model: PumpModel, head: u8) -> Result<PumpKind, PumpError> {
        match model {
            PumpModel::LabQ => Ok(PumpKind::LabQ),
            PumpModel::LabV => Ok(PumpKind::LabV),
            PumpModel::PdsII if (1..=8).contains(&head) => Ok(PumpKind::PdsII { head }),
            PumpModel::PdsII => Err(PumpError::OutOfRange(format!(
                "PDS II head must be 1..=8, not {head}"
            ))),
        }
    }

    pub fn model(self) -> PumpModel {
        match self {
            PumpKind::LabQ => PumpModel::LabQ,
            PumpKind::LabV => PumpModel::LabV,
            PumpKind::PdsII { .. } => PumpModel::PdsII,
        }
    }

    pub fn head(self) -> Option<u8> {
        match self {
            PumpKind::PdsII { head } => Some(head),
            _ => None,
        }
    }

    /// The registers this kind of pump is driven through. Never includes the
    /// head, tubing or "all start" registers.
    pub fn map(self) -> RegisterMap {
        match self {
            PumpKind::LabQ => RegisterMap {
                speed: 1002,
                flow: 1004,
                start_stop: 1006,
                direction: 1007,
                mode: None,
            },
            PumpKind::LabV => RegisterMap {
                speed: 1002,
                flow: 1004,
                start_stop: 1008,
                direction: 1009,
                mode: Some(1020),
            },
            PumpKind::PdsII { head } => {
                let base = u16::from(head) * 1000;
                RegisterMap {
                    speed: base + 32,
                    flow: base + 30,
                    start_stop: base + 3,
                    direction: base + 4,
                    mode: Some(base + 5),
                }
            }
        }
    }
}

/// Holding registers a pump is driven through (speed and flow are `f32`
/// pairs, the rest `u16`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisterMap {
    pub speed: u16,
    pub flow: u16,
    pub start_stop: u16,
    pub direction: u16,
    /// Operating mode, where the pump has one ([`TRANSFERRING`] is the mode we need).
    pub mode: Option<u16>,
}

/// Which pump a [`Pump`] talks to: its address, kind and top speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PumpTarget {
    pub address: u8,
    pub kind: PumpKind,
    pub max_rpm: f64,
}

impl PumpTarget {
    /// A LabQ at `address`, limited to 350 rpm.
    pub fn labq(address: u8) -> PumpTarget {
        PumpTarget {
            address,
            kind: PumpKind::LabQ,
            max_rpm: PumpModel::LabQ.default_max_rpm(),
        }
    }
}

// ---------------------------------------------------------------------------
// frame: pure request builders + response parsers
// ---------------------------------------------------------------------------

/// Pure MODBUS-RTU request builders and response parsers. No I/O.
pub mod frame {
    use super::{crc16, f32_be, with_crc, PduError};

    /// `03H` Read Holding Registers request.
    pub fn build_read_holding(addr: u8, start: u16, qty: u16) -> Vec<u8> {
        with_crc(vec![
            addr,
            0x03,
            (start >> 8) as u8,
            start as u8,
            (qty >> 8) as u8,
            qty as u8,
        ])
    }

    /// `06H` Write Single Register request.
    pub fn build_write_single(addr: u8, reg: u16, value: u16) -> Vec<u8> {
        with_crc(vec![
            addr,
            0x06,
            (reg >> 8) as u8,
            reg as u8,
            (value >> 8) as u8,
            value as u8,
        ])
    }

    /// `10H` Write Multiple Registers request carrying one big-endian `f32`
    /// (2 registers, 4 bytes).
    pub fn build_write_f32(addr: u8, reg: u16, value: f32) -> Vec<u8> {
        let b = f32_be(value);
        with_crc(vec![
            addr,
            0x10,
            (reg >> 8) as u8,
            reg as u8,
            0x00,
            0x02,
            0x04,
            b[0],
            b[1],
            b[2],
            b[3],
        ])
    }

    /// Verify length + CRC, returning the frame body without the trailing CRC.
    fn body_of(frame: &[u8]) -> Result<&[u8], PduError> {
        if frame.len() < 4 {
            return Err(PduError::TooShort);
        }
        let (body, crc) = frame.split_at(frame.len() - 2);
        if crc16(body) != u16::from_le_bytes([crc[0], crc[1]]) {
            return Err(PduError::BadCrc);
        }
        Ok(body)
    }

    /// Common checks: address echo, and exception vs. expected function.
    fn check_head(body: &[u8], addr: u8, want_fn: u8) -> Result<(), PduError> {
        if body.len() < 2 {
            return Err(PduError::TooShort);
        }
        if body[0] != addr {
            return Err(PduError::MalformedResponse);
        }
        if body[1] == want_fn | 0x80 {
            return Err(PduError::Exception(*body.get(2).unwrap_or(&0)));
        }
        if body[1] != want_fn {
            return Err(PduError::UnexpectedFunction(body[1]));
        }
        Ok(())
    }

    /// Parse a `06H` response and confirm it echoes `reg`/`value`.
    pub fn parse_write_single_response(
        frame: &[u8],
        addr: u8,
        reg: u16,
        value: u16,
    ) -> Result<(), PduError> {
        let body = body_of(frame)?;
        check_head(body, addr, 0x06)?;
        if body.len() != 6 {
            return Err(PduError::MalformedResponse);
        }
        let echo = [(reg >> 8) as u8, reg as u8, (value >> 8) as u8, value as u8];
        if body[2..6] != echo {
            return Err(PduError::MalformedResponse);
        }
        Ok(())
    }

    /// Parse a `10H` response and confirm `reg`/`qty`.
    pub fn parse_write_multiple_response(
        frame: &[u8],
        addr: u8,
        reg: u16,
        qty: u16,
    ) -> Result<(), PduError> {
        let body = body_of(frame)?;
        check_head(body, addr, 0x10)?;
        if body.len() != 6 {
            return Err(PduError::MalformedResponse);
        }
        let echo = [(reg >> 8) as u8, reg as u8, (qty >> 8) as u8, qty as u8];
        if body[2..6] != echo {
            return Err(PduError::MalformedResponse);
        }
        Ok(())
    }

    /// Parse a `03H` response into `qty` register words.
    pub fn parse_read_holding_response(
        frame: &[u8],
        addr: u8,
        qty: u16,
    ) -> Result<Vec<u16>, PduError> {
        let body = body_of(frame)?;
        check_head(body, addr, 0x03)?;
        let want_bytes = 2 * qty as usize;
        if body.len() != 3 + want_bytes || body[2] as usize != want_bytes {
            return Err(PduError::MalformedResponse);
        }
        Ok((0..qty as usize)
            .map(|i| u16::from_be_bytes([body[3 + 2 * i], body[4 + 2 * i]]))
            .collect())
    }
}

// ---------------------------------------------------------------------------
// transport + pump
// ---------------------------------------------------------------------------

/// A blocking MODBUS-RTU byte channel: send a framed request, get the framed
/// response back (CRC included, unvalidated).
pub trait Transport {
    fn transaction(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportError>;
}

/// Lets the daemon pick the real serial backend or the simulator at runtime and
/// still hold one concrete `Pump<Box<dyn Transport + Send>>`.
impl Transport for Box<dyn Transport + Send> {
    fn transaction(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportError> {
        (**self).transaction(request)
    }
}

/// Pump-level operations over any [`Transport`].
pub trait PumpTransport {
    fn set_direction(&mut self, clockwise: bool) -> Result<(), PumpError>;
    fn set_head_type(&mut self, code: u16) -> Result<(), PumpError>;
    fn set_tubing_size(&mut self, code: u16) -> Result<(), PumpError>;
    fn set_speed_rpm(&mut self, rpm: f32) -> Result<(), PumpError>;
    fn set_flow_ml_min(&mut self, ml_min: f32) -> Result<(), PumpError>;
    fn start(&mut self) -> Result<(), PumpError>;
    fn stop(&mut self) -> Result<(), PumpError>;
    fn read_speed_rpm(&mut self) -> Result<f32, PumpError>;
    fn read_flow_ml_min(&mut self) -> Result<f32, PumpError>;
    /// Put the pump in transferring mode if it has a mode register and is not
    /// there: `Ok(false)` when nothing was needed (no mode register, or it reads
    /// [`TRANSFERRING`]), `Ok(true)` after stopping it and writing the mode.
    fn ensure_transferring(&mut self) -> Result<bool, PumpError>;
}

/// A pump (LabQ, LabV or PDS II head) reachable through `transport`.
pub struct Pump<T: Transport> {
    transport: T,
    target: PumpTarget,
}

impl<T: Transport> Pump<T> {
    /// A LabQ at `address`.
    pub fn new(transport: T, address: u8) -> Self {
        Self::with_target(transport, PumpTarget::labq(address))
    }

    pub fn with_target(transport: T, target: PumpTarget) -> Self {
        Self { transport, target }
    }

    pub fn target(&self) -> PumpTarget {
        self.target
    }

    pub fn set_target(&mut self, target: PumpTarget) {
        self.target = target;
    }

    /// Run `f` against another pump on the same bus (the refill pump), then
    /// target this one again, whatever `f` returned.
    pub fn with_target_of<R>(&mut self, target: PumpTarget, f: impl FnOnce(&mut Self) -> R) -> R {
        let own = self.target;
        self.target = target;
        let out = f(self);
        self.target = own;
        out
    }

    /// Borrow the underlying transport (e.g. a [`SimPump`] in tests).
    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// The MODBUS slave address this pump is addressed at.
    pub fn address(&self) -> u8 {
        self.target.address
    }

    /// Re-target this pump at a different MODBUS slave address (same bus).
    /// Used when the operator corrects the address without a daemon restart.
    pub fn set_address(&mut self, address: u8) {
        self.target.address = address;
    }

    /// Like [`Pump::with_target_of`], same kind and top speed at another address.
    pub fn with_address<R>(&mut self, address: u8, f: impl FnOnce(&mut Self) -> R) -> R {
        let target = PumpTarget { address, ..self.target };
        self.with_target_of(target, f)
    }

    /// One transaction, retried once on any failure.
    ///
    /// The LabQ silently discards a frame that lands inside its inter-frame gap,
    /// and an RS-485 line picks up the occasional glitch, both surface here as a
    /// timeout or a bad frame. A single re-send (already spaced by the
    /// transport's own inter-frame gap) clears almost all of them, so a
    /// long run doesn't accumulate one-off `write_fail`s.
    fn transact_retrying(&mut self, req: &[u8]) -> Result<Vec<u8>, PumpError> {
        match self.transport.transaction(req) {
            Ok(r) => Ok(r),
            Err(_first) => Ok(self.transport.transaction(req)?),
        }
    }

    fn write_reg(&mut self, reg: u16, value: u16) -> Result<(), PumpError> {
        let req = frame::build_write_single(self.target.address, reg, value);
        let resp = self.transact_retrying(&req)?;
        frame::parse_write_single_response(&resp, self.target.address, reg, value)?;
        Ok(())
    }

    fn write_f32(&mut self, reg: u16, value: f32) -> Result<(), PumpError> {
        let req = frame::build_write_f32(self.target.address, reg, value);
        let resp = self.transact_retrying(&req)?;
        frame::parse_write_multiple_response(&resp, self.target.address, reg, 2)?;
        Ok(())
    }

    fn read_u16(&mut self, reg: u16) -> Result<u16, PumpError> {
        let req = frame::build_read_holding(self.target.address, reg, 1);
        let resp = self.transact_retrying(&req)?;
        let words = frame::parse_read_holding_response(&resp, self.target.address, 1)?;
        Ok(words[0])
    }

    fn read_f32(&mut self, reg: u16) -> Result<f32, PumpError> {
        let req = frame::build_read_holding(self.target.address, reg, 2);
        let resp = self.transact_retrying(&req)?;
        let words = frame::parse_read_holding_response(&resp, self.target.address, 2)?;
        Ok(f32_from_words(words[0], words[1]))
    }
}

impl<T: Transport> PumpTransport for Pump<T> {
    fn set_direction(&mut self, clockwise: bool) -> Result<(), PumpError> {
        self.write_reg(self.target.kind.map().direction, u16::from(clockwise))
    }

    fn set_head_type(&mut self, code: u16) -> Result<(), PumpError> {
        self.write_reg(reg::PUMP_HEAD_TYPE, code)
    }

    fn set_tubing_size(&mut self, code: u16) -> Result<(), PumpError> {
        self.write_reg(reg::TUBING_SIZE, code)
    }

    fn set_speed_rpm(&mut self, rpm: f32) -> Result<(), PumpError> {
        let max = self.target.max_rpm;
        if !(limits::RPM_MIN..=max).contains(&(rpm as f64)) {
            return Err(PumpError::OutOfRange(format!(
                "motor speed must be 0.1..={max} rpm"
            )));
        }
        self.write_f32(self.target.kind.map().speed, rpm)
    }

    fn set_flow_ml_min(&mut self, ml_min: f32) -> Result<(), PumpError> {
        if !(limits::FLOW_MIN..=limits::FLOW_MAX).contains(&(ml_min as f64)) {
            return Err(PumpError::OutOfRange("flow rate must be 0..=99999 ml/min".to_string()));
        }
        self.write_f32(self.target.kind.map().flow, ml_min)
    }

    fn start(&mut self) -> Result<(), PumpError> {
        self.write_reg(self.target.kind.map().start_stop, 1)
    }

    fn stop(&mut self) -> Result<(), PumpError> {
        self.write_reg(self.target.kind.map().start_stop, 0)
    }

    fn read_speed_rpm(&mut self) -> Result<f32, PumpError> {
        self.read_f32(self.target.kind.map().speed)
    }

    fn read_flow_ml_min(&mut self) -> Result<f32, PumpError> {
        self.read_f32(self.target.kind.map().flow)
    }

    fn ensure_transferring(&mut self) -> Result<bool, PumpError> {
        let Some(mode) = self.target.kind.map().mode else {
            return Ok(false);
        };
        if self.read_u16(mode)? == TRANSFERRING {
            return Ok(false);
        }
        self.stop()?;
        self.write_reg(mode, TRANSFERRING)?;
        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// SimPump: register-level simulator with fault injection
// ---------------------------------------------------------------------------

/// An in-memory LabQ pump for tests: a real register model driven by real
/// request frames, with hooks to inject faults.
#[derive(Debug, Clone)]
pub struct SimPump {
    address: u8,
    /// Every holding register written so far (any address is accepted).
    regs: std::collections::BTreeMap<u16, u16>,
    /// Swallow this many upcoming transactions (return [`TransportError::Timeout`]).
    pub drop_next: usize,
    /// Answer the next transaction with this exception code instead of acting.
    pub next_exception: Option<u8>,
    /// When true, every `10H` write to `MOTOR_SPEED` / `FLOW_RATE` is answered
    /// with exception `0x03` (illegal data value), models a LabQ that refuses a
    /// setpoint (e.g. ml/min with no head/tubing calibration set on the pump).
    pub reject_setpoint_writes: bool,
    /// When set, a read of MOTOR_SPEED / FLOW_RATE reports this value instead of
    /// what was written, models a pump that stops tracking the setpoint.
    pub frozen_readback: Option<f32>,
    /// Number of transactions seen (successful or dropped).
    pub transactions: usize,
}

impl SimPump {
    pub fn new(address: u8) -> Self {
        Self {
            address,
            regs: std::collections::BTreeMap::new(),
            drop_next: 0,
            next_exception: None,
            reject_setpoint_writes: false,
            frozen_readback: None,
            transactions: 0,
        }
    }

    fn set(&mut self, reg: u16, value: u16) {
        self.regs.insert(reg, value);
    }

    fn get(&self, reg: u16) -> u16 {
        self.regs.get(&reg).copied().unwrap_or(0)
    }

    /// A speed or flow register of any map (LabQ / LabV 1002, 1004; PDS II n030, n032).
    fn is_setpoint_reg(reg: u16) -> bool {
        matches!(reg % 1000, 2 | 4 | 30 | 32)
    }

    /// Any holding register (0 if never written).
    pub fn reg(&self, reg: u16) -> u16 {
        self.get(reg)
    }

    /// Set any holding register directly (a test setting up the pump's state).
    pub fn set_reg(&mut self, reg: u16, value: u16) {
        self.set(reg, value);
    }

    /// The speed held in `map`'s speed registers (rpm).
    pub fn speed_rpm_of(&self, map: RegisterMap) -> f32 {
        f32_from_words(self.get(map.speed), self.get(map.speed + 1))
    }

    /// Whether `map`'s start/stop register is set to run.
    pub fn running_of(&self, map: RegisterMap) -> bool {
        self.get(map.start_stop) == 1
    }

    /// Pump-head code currently set (register 1000).
    pub fn head_code(&self) -> u16 {
        self.get(reg::PUMP_HEAD_TYPE)
    }

    /// Tubing-size code currently set (register 1001).
    pub fn tubing_code(&self) -> u16 {
        self.get(reg::TUBING_SIZE)
    }

    /// Current commanded motor speed (rpm), as the pump would report it.
    pub fn speed_rpm(&self) -> f32 {
        self.speed_rpm_of(PumpKind::LabQ.map())
    }

    /// Current commanded flow rate (ml/min).
    pub fn flow_ml_min(&self) -> f32 {
        f32_from_words(self.get(reg::FLOW_RATE), self.get(reg::FLOW_RATE + 1))
    }

    /// Whether start/stop is set to run.
    pub fn running(&self) -> bool {
        self.running_of(PumpKind::LabQ.map())
    }

    /// Whether direction is clockwise.
    pub fn direction_cw(&self) -> bool {
        self.get(reg::DIRECTION) == 1
    }
}

impl Transport for SimPump {
    fn transaction(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportError> {
        self.transactions += 1;

        if request.len() < 4 {
            return Err(TransportError::Timeout);
        }
        let (body, crc) = request.split_at(request.len() - 2);
        if crc16(body) != u16::from_le_bytes([crc[0], crc[1]]) {
            // A real device ignores a corrupt frame, caller sees a timeout.
            return Err(TransportError::Timeout);
        }
        if self.drop_next > 0 {
            self.drop_next -= 1;
            return Err(TransportError::Timeout);
        }
        if body[0] != self.address && body[0] != 0 {
            return Err(TransportError::Timeout);
        }

        let func = body[1];
        if let Some(code) = self.next_exception.take() {
            return Ok(with_crc(vec![body[0], func | 0x80, code]));
        }
        if body.len() < 6 {
            return Err(TransportError::Timeout);
        }

        match func {
            0x06 => {
                let reg = u16::from_be_bytes([body[2], body[3]]);
                let val = u16::from_be_bytes([body[4], body[5]]);
                self.set(reg, val);
                Ok(request.to_vec()) // 06H response echoes the request
            }
            0x10 => {
                let reg = u16::from_be_bytes([body[2], body[3]]);
                let qty = u16::from_be_bytes([body[4], body[5]]);
                if body.len() < 7 + 2 * qty as usize {
                    return Err(TransportError::Timeout);
                }
                if self.reject_setpoint_writes && Self::is_setpoint_reg(reg) {
                    return Ok(with_crc(vec![body[0], 0x10 | 0x80, 0x03]));
                }
                for i in 0..qty as usize {
                    let hi = body[7 + 2 * i];
                    let lo = body[8 + 2 * i];
                    self.set(reg + i as u16, u16::from_be_bytes([hi, lo]));
                }
                Ok(with_crc(vec![
                    body[0], 0x10, body[2], body[3], body[4], body[5],
                ]))
            }
            0x03 => {
                let reg = u16::from_be_bytes([body[2], body[3]]);
                let qty = u16::from_be_bytes([body[4], body[5]]);
                // Optional fault: report a stuck setpoint on the value registers.
                let frozen_words = match self.frozen_readback {
                    Some(v) if qty == 2 && Self::is_setpoint_reg(reg) => Some(f32_to_words(v)),
                    _ => None,
                };
                let mut out = vec![body[0], 0x03, (2 * qty) as u8];
                for i in 0..qty {
                    let w = match (frozen_words, i) {
                        (Some((hi, _)), 0) => hi,
                        (Some((_, lo)), 1) => lo,
                        _ => self.get(reg + i),
                    };
                    out.push((w >> 8) as u8);
                    out.push(w as u8);
                }
                Ok(with_crc(out))
            }
            other => Ok(with_crc(vec![body[0], other | 0x80, 0x01])), // illegal function
        }
    }
}

/// Several simulated pumps on one RS-485 line: every address answers with its
/// own [`SimPump`], created the first time it is addressed, unless it was
/// unplugged. What the daemon drives on `serial.path = "sim"`, so the refill
/// pump (a second address) answers as well as the feed pump.
#[derive(Debug, Clone, Default)]
pub struct SimBus {
    pumps: std::collections::BTreeMap<u8, SimPump>,
    unplugged: std::collections::BTreeSet<u8>,
}

impl SimBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// The pump at `address`, plugging it in if it was not yet.
    pub fn pump(&mut self, address: u8) -> &mut SimPump {
        self.pumps.entry(address).or_insert_with(|| SimPump::new(address))
    }

    /// The pump at `address`, if it has been addressed.
    pub fn get(&self, address: u8) -> Option<&SimPump> {
        self.pumps.get(&address)
    }

    /// Take the pump at `address` off the line (`false`) or put it back.
    pub fn set_plugged(&mut self, address: u8, plugged: bool) {
        if plugged {
            self.unplugged.remove(&address);
        } else {
            self.unplugged.insert(address);
        }
    }
}

impl Transport for SimBus {
    fn transaction(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportError> {
        let Some(&address) = request.first() else {
            return Err(TransportError::Timeout);
        };
        if self.unplugged.contains(&address) {
            return Err(TransportError::Timeout);
        }
        self.pump(address).transaction(request)
    }
}

// ---------------------------------------------------------------------------
// serial: blocking RS-485 backend (feature = "serial")
// ---------------------------------------------------------------------------

#[cfg(feature = "serial")]
pub mod serial {
    //! Blocking [`Transport`](super::Transport) over a real serial adapter, plus
    //! port enumeration for the "choose port & connect" flow in the UI.

    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    use serialport::{ClearBuffer, DataBits, Parity, SerialPort, StopBits};

    use super::{Transport, TransportError};

    /// A serial port the UI can offer the user.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct PortInfo {
        /// OS name, e.g. `COM4` or `/dev/ttyUSB0`.
        pub name: String,
        /// Coarse kind: `"usb"`, `"bluetooth"`, `"pci"`, `"unknown"`.
        pub kind: &'static str,
        /// USB manufacturer string, if the OS reports one.
        pub manufacturer: Option<String>,
        /// USB product string, if the OS reports one.
        pub product: Option<String>,
        /// `USB VID:PID` as `"0403:6001"`, if applicable.
        pub usb_id: Option<String>,
    }

    /// List serial ports currently present on the system.
    pub fn available_ports() -> Result<Vec<PortInfo>, TransportError> {
        let ports = serialport::available_ports().map_err(|e| TransportError::Io(e.to_string()))?;
        Ok(ports
            .into_iter()
            .map(|p| match p.port_type {
                serialport::SerialPortType::UsbPort(u) => PortInfo {
                    name: p.port_name,
                    kind: "usb",
                    manufacturer: u.manufacturer,
                    product: u.product,
                    usb_id: Some(format!("{:04x}:{:04x}", u.vid, u.pid)),
                },
                serialport::SerialPortType::BluetoothPort => PortInfo {
                    name: p.port_name,
                    kind: "bluetooth",
                    manufacturer: None,
                    product: None,
                    usb_id: None,
                },
                serialport::SerialPortType::PciPort => PortInfo {
                    name: p.port_name,
                    kind: "pci",
                    manufacturer: None,
                    product: None,
                    usb_id: None,
                },
                serialport::SerialPortType::Unknown => PortInfo {
                    name: p.port_name,
                    kind: "unknown",
                    manufacturer: None,
                    product: None,
                    usb_id: None,
                },
            })
            .collect())
    }

    /// Blocking RTU transport over one serial port, fixed at the LabQ frame
    /// format (8 data bits, even parity, 1 stop bit).
    pub struct SerialTransport {
        port: Box<dyn SerialPort>,
        timeout: Duration,
        /// Minimum silence between the end of one transaction and the start of
        /// the next. The LabQ pump silently drops a request that lands too soon
        /// after its previous reply, so we hold this gap ourselves, otherwise
        /// the daemon fires the start sequence (direction, speed, start)
        /// back-to-back and the pump answers only the first frame, failing the
        /// run with "serial transaction timed out". Bench-measured on an FTDI
        /// RS-485 adapter: ~25 ms is not enough, 100 ms is reliable.
        min_gap: Duration,
        /// When the previous transaction finished, for the `min_gap` guard.
        last_end: Option<Instant>,
    }

    /// The [`SerialTransport::min_gap`] for a baud: the MODBUS-RTU t3.5 silence
    /// (3.5 character times, 11 bits each), floored at 100 ms because the LabQ
    /// pump needs far more slack than the standard. Effectively always the 100 ms
    /// floor at the pump's ≤ 9600 baud. Cheap: the daemon only chains frames
    /// during the start / stop / resume sequences; ticks are a second apart.
    fn inter_frame_gap(baud: u32) -> Duration {
        let t35 = Duration::from_micros(3_500_000 * 11 / u64::from(baud.max(1)));
        t35.max(Duration::from_millis(100))
    }

    impl SerialTransport {
        /// Open `path` at `baud` (1200 / 2400 / 4800 / 9600). `timeout` bounds a
        /// whole transaction.
        pub fn open(path: &str, baud: u32, timeout: Duration) -> Result<Self, TransportError> {
            let port = serialport::new(path, baud)
                .data_bits(DataBits::Eight)
                .parity(Parity::Even)
                .stop_bits(StopBits::One)
                .timeout(Duration::from_millis(200))
                .open()
                .map_err(|e| TransportError::Io(e.to_string()))?;
            Ok(Self {
                port,
                timeout,
                min_gap: inter_frame_gap(baud),
                last_end: None,
            })
        }

        fn read_until(
            &mut self,
            buf: &mut Vec<u8>,
            target: usize,
            deadline: Instant,
        ) -> Result<(), TransportError> {
            let mut chunk = [0u8; 64];
            while buf.len() < target {
                if Instant::now() >= deadline {
                    return Err(TransportError::Timeout);
                }
                let want = (target - buf.len()).min(chunk.len());
                match self.port.read(&mut chunk[..want]) {
                    Ok(0) => return Err(TransportError::Timeout),
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(TransportError::Io(e.to_string())),
                }
            }
            Ok(())
        }

        /// One framed request/response, without the inter-frame gap bookkeeping.
        fn exchange(
            &mut self,
            request: &[u8],
            deadline: Instant,
        ) -> Result<Vec<u8>, TransportError> {
            self.port
                .clear(ClearBuffer::Input)
                .map_err(|e| TransportError::Io(e.to_string()))?;
            self.port
                .write_all(request)
                .map_err(|e| TransportError::Io(e.to_string()))?;
            self.port
                .flush()
                .map_err(|e| TransportError::Io(e.to_string()))?;

            let mut buf: Vec<u8> = Vec::with_capacity(16);
            self.read_until(&mut buf, 2, deadline)?;
            let func = buf[1];
            let total = if func & 0x80 != 0 {
                5
            } else if func == 0x03 {
                self.read_until(&mut buf, 3, deadline)?;
                3 + buf[2] as usize + 2
            } else {
                8
            };
            self.read_until(&mut buf, total, deadline)?;
            Ok(buf)
        }
    }

    impl Transport for SerialTransport {
        fn transaction(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportError> {
            if let Some(end) = self.last_end {
                let idle = end.elapsed();
                if idle < self.min_gap {
                    std::thread::sleep(self.min_gap - idle);
                }
            }
            let deadline = Instant::now() + self.timeout;
            let out = self.exchange(request, deadline);
            self.last_end = Some(Instant::now());
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::frame::*;
    use super::*;

    #[test]
    fn crc_matches_vendor_start_frame() {
        let body = [0x01, 0x06, 0x03, 0xEE, 0x00, 0x01];
        let crc = crc16(&body);
        assert_eq!((crc & 0x00FF) as u8, 0x28);
        assert_eq!((crc >> 8) as u8, 0x7B);
    }

    #[test]
    fn with_crc_matches_vendor_direction_frame() {
        assert_eq!(
            with_crc(vec![0x01, 0x06, 0x03, 0xEF, 0x00, 0x01]),
            vec![0x01, 0x06, 0x03, 0xEF, 0x00, 0x01, 0x79, 0xBB]
        );
    }

    #[test]
    fn float_encoding_matches_vendor_example() {
        assert_eq!(f32_be(8.9), [0x41, 0x0E, 0x66, 0x66]);
        assert_eq!(f32_from_be([0x41, 0x0E, 0x66, 0x66]), 8.9);
        assert_eq!(f32_from_words(0x410E, 0x6666), 8.9_f32);
    }

    #[test]
    fn f32_word_round_trip() {
        let (h, l) = f32_to_words(58.8);
        assert_eq!((h, l), (0x426B, 0x3333));
        assert_eq!(f32_from_words(h, l), 58.8_f32);
    }

    #[test]
    fn build_write_f32_matches_vendor_speed_frame() {
        assert_eq!(
            build_write_f32(1, reg::MOTOR_SPEED, 58.8),
            vec![0x01, 0x10, 0x03, 0xEA, 0x00, 0x02, 0x04, 0x42, 0x6B, 0x33, 0x33, 0x58, 0x29]
        );
    }

    #[test]
    fn build_write_f32_matches_vendor_flow_frame() {
        assert_eq!(
            build_write_f32(1, reg::FLOW_RATE, 50.0),
            vec![0x01, 0x10, 0x03, 0xEC, 0x00, 0x02, 0x04, 0x42, 0x48, 0x00, 0x00, 0x7D, 0x2C]
        );
    }

    #[test]
    fn build_write_single_matches_vendor_start_frame() {
        assert_eq!(
            build_write_single(1, reg::START_STOP, 1),
            vec![0x01, 0x06, 0x03, 0xEE, 0x00, 0x01, 0x28, 0x7B]
        );
    }

    #[test]
    fn build_read_holding_has_valid_crc() {
        let f = build_read_holding(1, reg::MOTOR_SPEED, 2);
        assert_eq!(&f[..6], &[0x01, 0x03, 0x03, 0xEA, 0x00, 0x02]);
        assert_eq!(crc16(&f[..6]), u16::from_le_bytes([f[6], f[7]]));
    }

    #[test]
    fn parse_write_multiple_accepts_vendor_ack() {
        // "01 10 03 EA 00 02 60 78"
        let ack = with_crc(vec![0x01, 0x10, 0x03, 0xEA, 0x00, 0x02]);
        assert_eq!(ack, vec![0x01, 0x10, 0x03, 0xEA, 0x00, 0x02, 0x60, 0x78]);
        assert!(parse_write_multiple_response(&ack, 1, reg::MOTOR_SPEED, 2).is_ok());
    }

    #[test]
    fn parse_rejects_bad_crc_and_exception() {
        let mut ack = build_write_single(1, reg::START_STOP, 1);
        *ack.last_mut().unwrap() ^= 0xFF;
        assert_eq!(
            parse_write_single_response(&ack, 1, reg::START_STOP, 1),
            Err(PduError::BadCrc)
        );

        let exc = with_crc(vec![0x01, 0x86, 0x06]);
        assert_eq!(
            parse_write_single_response(&exc, 1, reg::START_STOP, 1),
            Err(PduError::Exception(0x06))
        );
    }

    #[test]
    fn parse_read_holding_round_trips_registers() {
        let resp = with_crc(vec![0x01, 0x03, 0x04, 0x42, 0x6B, 0x33, 0x33]);
        let words = parse_read_holding_response(&resp, 1, 2).unwrap();
        assert_eq!(words, vec![0x426B, 0x3333]);
        assert_eq!(f32_from_words(words[0], words[1]), 58.8_f32);
    }

    #[test]
    fn pump_over_sim_sets_and_reads_back_speed() {
        let mut pump = Pump::new(SimPump::new(1), 1);
        pump.set_direction(false).unwrap();
        pump.set_speed_rpm(123.4).unwrap();
        pump.start().unwrap();

        assert!(pump.transport().running());
        assert!(!pump.transport().direction_cw());
        assert!((pump.read_speed_rpm().unwrap() - 123.4).abs() < 1e-3);
    }

    #[test]
    fn pump_rejects_out_of_range_setpoints() {
        let mut pump = Pump::new(SimPump::new(1), 1);
        assert!(matches!(
            pump.set_speed_rpm(999.0),
            Err(PumpError::OutOfRange(_))
        ));
        assert!(matches!(
            pump.set_flow_ml_min(-1.0),
            Err(PumpError::OutOfRange(_))
        ));
    }

    #[test]
    fn a_single_dropped_frame_is_transparently_retried() {
        let mut pump = Pump::new(SimPump::new(1), 1);
        pump.transport_mut().drop_next = 1; // one drop, the built-in retry clears it
        pump.set_speed_rpm(50.0).unwrap();
        assert!((pump.transport().speed_rpm() - 50.0).abs() < 1e-3);
    }

    #[test]
    fn sim_fault_injection_surfaces_as_errors() {
        let mut pump = Pump::new(SimPump::new(1), 1);

        pump.transport_mut().drop_next = 2; // both the write and its retry are dropped
        assert!(matches!(
            pump.set_speed_rpm(50.0),
            Err(PumpError::Transport(TransportError::Timeout))
        ));
        // the pump recovers on the next attempt
        pump.set_speed_rpm(50.0).unwrap();
        assert!((pump.transport().speed_rpm() - 50.0).abs() < 1e-3);

        pump.transport_mut().next_exception = Some(0x02);
        assert!(matches!(
            pump.start(),
            Err(PumpError::Pdu(PduError::Exception(0x02)))
        ));
    }

    #[test]
    fn exception_display_names_the_code() {
        assert_eq!(
            PduError::Exception(0x03).to_string(),
            "MODBUS exception 0x03 (illegal data value)"
        );
        assert_eq!(
            PduError::Exception(0x02).to_string(),
            "MODBUS exception 0x02 (illegal data address)"
        );
        assert!(PduError::Exception(0x7F).to_string().contains("unknown"));
    }

    #[test]
    fn sim_can_reject_setpoint_writes_with_0x03() {
        let mut pump = Pump::new(SimPump::new(1), 1);
        pump.transport_mut().reject_setpoint_writes = true;
        assert!(matches!(
            pump.set_flow_ml_min(12.0),
            Err(PumpError::Pdu(PduError::Exception(0x03)))
        ));
        assert!(matches!(
            pump.set_speed_rpm(50.0),
            Err(PumpError::Pdu(PduError::Exception(0x03)))
        ));
        // start/stop and direction still work, only the value registers are refused.
        pump.start().unwrap();
        assert!(pump.transport().running());
    }

    #[test]
    fn with_address_drives_another_pump_and_comes_back() {
        let mut pump = Pump::new(SimBus::new(), 1);
        pump.with_address(2, |p| p.start()).unwrap();
        assert_eq!(pump.address(), 1);
        assert!(pump.transport().get(2).unwrap().running());
        assert!(pump.transport().get(1).is_none(), "the feed pump was never addressed");
        // An error inside still puts the address back.
        pump.transport_mut().set_plugged(3, false);
        assert!(pump.with_address(3, |p| p.stop()).is_err());
        assert_eq!(pump.address(), 1);
    }

    #[test]
    fn register_maps_follow_the_protocols() {
        assert_eq!(
            PumpKind::LabQ.map(),
            RegisterMap { speed: 1002, flow: 1004, start_stop: 1006, direction: 1007, mode: None }
        );
        assert_eq!(
            PumpKind::LabV.map(),
            RegisterMap { speed: 1002, flow: 1004, start_stop: 1008, direction: 1009, mode: Some(1020) }
        );
        assert_eq!(
            PumpKind::PdsII { head: 3 }.map(),
            RegisterMap { speed: 3032, flow: 3030, start_stop: 3003, direction: 3004, mode: Some(3005) }
        );
        assert!(PumpKind::new(PumpModel::PdsII, 0).is_err());
        assert!(PumpKind::new(PumpModel::PdsII, 9).is_err());
        assert_eq!(PumpKind::new(PumpModel::PdsII, 8).unwrap().head(), Some(8));
        assert_eq!(PumpKind::new(PumpModel::LabV, 0).unwrap(), PumpKind::LabV);
    }

    #[test]
    fn models_parse_and_carry_their_limits() {
        for m in [PumpModel::LabQ, PumpModel::LabV, PumpModel::PdsII] {
            assert_eq!(PumpModel::parse(m.as_str()), Some(m));
        }
        assert_eq!(PumpModel::parse("nope"), None);
        assert_eq!(PumpModel::LabV.default_max_rpm(), 150.0);
        assert_eq!(PumpModel::PdsII.max_rpm_ceiling(), 600.0);
        assert_eq!(PumpModel::LabQ.max_address(), 247);
        assert_eq!(PumpModel::PdsII.max_address(), 32);
        assert_eq!(PumpModel::PdsII.label(), "PDS II");
    }

    #[test]
    fn labq_frames_unchanged() {
        let mut p = Pump::new(SimPump::new(1), 1);
        p.start().unwrap();
        p.set_speed_rpm(58.8).unwrap();
        assert!(p.transport().running());
        assert!((p.transport().speed_rpm() - 58.8).abs() < 1e-4);
        assert_eq!(
            frame::build_write_single(1, PumpKind::LabQ.map().start_stop, 1),
            frame::build_write_single(1, reg::START_STOP, 1)
        );
        assert_eq!(p.target(), PumpTarget::labq(1));
    }

    #[test]
    fn a_labv_target_writes_the_labv_registers() {
        let target = PumpTarget { address: 4, kind: PumpKind::LabV, max_rpm: 150.0 };
        let mut p = Pump::with_target(SimPump::new(4), target);
        p.start().unwrap();
        assert_eq!(p.transport().reg(1008), 1);
        assert_eq!(p.transport().reg(1006), 0);
        p.set_direction(true).unwrap();
        assert_eq!(p.transport().reg(1009), 1);
        assert_eq!(p.transport().reg(1007), 0);
    }

    #[test]
    fn a_pds2_head_is_driven_at_its_own_registers() {
        let kind = PumpKind::new(PumpModel::PdsII, 2).unwrap();
        let target = PumpTarget { address: 5, kind, max_rpm: 600.0 };
        let mut p = Pump::with_target(SimPump::new(5), target);
        p.start().unwrap();
        p.set_speed_rpm(321.5).unwrap();
        assert!(p.transport().running_of(kind.map()));
        assert!((p.transport().speed_rpm_of(kind.map()) - 321.5).abs() < 1e-3);
        assert!((p.read_speed_rpm().unwrap() - 321.5).abs() < 1e-3);
    }

    #[test]
    fn speed_over_the_target_max_is_refused() {
        let target = PumpTarget { address: 4, kind: PumpKind::LabV, max_rpm: 150.0 };
        let mut p = Pump::with_target(SimPump::new(4), target);
        assert!(matches!(p.set_speed_rpm(150.1), Err(PumpError::OutOfRange(_))));
        p.set_speed_rpm(150.0).unwrap();
        // the message names the figure
        let msg = p.set_speed_rpm(200.0).unwrap_err().to_string();
        assert!(msg.contains("150"), "{msg}");
    }

    #[test]
    fn ensure_transferring_reads_first() {
        let mut labq = Pump::new(SimPump::new(1), 1);
        assert_eq!(labq.ensure_transferring(), Ok(false));
        assert_eq!(labq.transport().transactions, 0);

        let kind = PumpKind::new(PumpModel::PdsII, 2).unwrap();
        let target = PumpTarget { address: 3, kind, max_rpm: 600.0 };
        let mut p = Pump::with_target(SimPump::new(3), target);
        p.transport_mut().set_reg(2005, 1);
        p.transport_mut().set_reg(2003, 1);
        assert_eq!(p.ensure_transferring(), Ok(true));
        assert_eq!(p.transport().reg(2003), 0);
        assert_eq!(p.transport().reg(2005), 0);
        let seen = p.transport().transactions;
        assert_eq!(p.ensure_transferring(), Ok(false));
        assert_eq!(p.transport().transactions, seen + 1, "one read, no write");
    }

    #[test]
    fn no_frame_for_head_tubing_or_all_start() {
        let mut kinds = vec![PumpKind::LabQ, PumpKind::LabV];
        kinds.extend((1..=8).map(|head| PumpKind::PdsII { head }));
        for k in kinds {
            let m = k.map();
            let mut regs = vec![m.speed, m.flow, m.start_stop, m.direction];
            regs.extend(m.mode);
            for r in regs {
                for banned in [1000u16, 1001, 10] {
                    assert_ne!(r, banned, "{k:?}");
                }
                assert!(!matches!(r % 1000, 0 | 1), "{k:?} reg {r}");
            }
        }
    }

    #[test]
    fn sim_faults_apply_to_a_pds2_setpoint() {
        let kind = PumpKind::PdsII { head: 1 };
        let target = PumpTarget { address: 1, kind, max_rpm: 600.0 };
        let mut p = Pump::with_target(SimPump::new(1), target);
        p.transport_mut().reject_setpoint_writes = true;
        assert!(matches!(
            p.set_speed_rpm(50.0),
            Err(PumpError::Pdu(PduError::Exception(0x03)))
        ));
        p.transport_mut().reject_setpoint_writes = false;
        p.set_speed_rpm(50.0).unwrap();
        p.transport_mut().frozen_readback = Some(10.0);
        assert!((p.read_speed_rpm().unwrap() - 10.0).abs() < 1e-3);
    }

    #[test]
    fn with_target_of_restores_the_own_target() {
        let own = PumpTarget::labq(1);
        let other = PumpTarget { address: 2, kind: PumpKind::LabV, max_rpm: 150.0 };
        let mut p = Pump::with_target(SimBus::new(), own);
        p.with_target_of(other, |p| p.start()).unwrap();
        assert_eq!(p.target(), own);
        assert_eq!(p.transport().get(2).unwrap().reg(1008), 1);
    }

    #[test]
    fn sim_ignores_frames_for_other_addresses() {
        let mut pump = Pump::new(SimPump::new(2), 7); // address mismatch
        assert!(matches!(
            pump.start(),
            Err(PumpError::Transport(TransportError::Timeout))
        ));
    }
}
