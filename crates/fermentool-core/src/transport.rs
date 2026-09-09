//! Building the pump transport from configuration, and swapping it on a live
//! [`Engine`](crate::engine::Engine) without restarting the daemon.
//!
//! `build_engine` (in `main.rs`) and `POST /api/serial/reconnect` both go through
//! [`open`]: `serial.path = "sim"` (or empty) selects the in-process simulator;
//! anything else is a real serial port, with the same "log the error and fall
//! back to the simulator" behaviour the daemon has always had at boot.

use std::time::Duration;

use fermentool_modbus::serial::SerialTransport;
use fermentool_modbus::{SimPump, Transport};

use crate::config::SerialConfig;

/// A whole serial transaction is bounded by this; matches `build_engine`'s
/// historical value.
const OPEN_TIMEOUT: Duration = Duration::from_millis(1500);

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
/// (a no-op — `control.rs`'s simulator-backed tests build an `Engine<SimPump>`).
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
}
