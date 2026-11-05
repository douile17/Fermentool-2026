//! What the helper needs from a camera source, so the request loop can be
//! tested without a webcam.

use crate::protocol::{ControlInfo, ControlValue, DeviceInfo, ErrorCode};
use crate::test_pattern::{TestPattern, TEST_PATTERN_NAME};

/// Picture sizes the app offers, whatever a device reports beyond them.
pub const GLOBAL_SIZES: [(u32, u32); 3] = [(640, 480), (1280, 720), (1920, 1080)];

/// JPEG quality of every frame sent to the daemon.
pub const JPEG_QUALITY: u8 = 85;

pub type BackendError = (ErrorCode, String);

pub trait Backend {
    fn list(&mut self) -> Result<Vec<DeviceInfo>, BackendError>;
    /// Opens the `index`-th device named `device` (0 = first) and returns the
    /// size it really delivers.
    fn open(&mut self, device: &str, index: u32, width: u32, height: u32) -> Result<(u32, u32), BackendError>;
    fn grab(&mut self) -> Result<Vec<u8>, BackendError>;
    /// The open camera's picture controls; none on a camera without any.
    fn controls(&mut self) -> Result<Vec<ControlInfo>, BackendError> {
        Ok(Vec::new())
    }
    fn set_control(&mut self, v: &ControlValue) -> Result<(), BackendError> {
        Err((ErrorCode::Failed, format!("this camera has no {} control", v.control.name())))
    }
    /// Stops streaming; harmless when none is open. The daemon still counts
    /// on the helper process exiting to free the device.
    fn close(&mut self);
}

/// Sorts a capture error by its text: Media Foundation reports a camera held
/// by another app only at the first frame, as 0xC00D3704 ("lack of hardware
/// resources").
pub fn classify_error(message: &str) -> ErrorCode {
    let m = message.to_ascii_lowercase();
    if m.contains("0xc00d3704") {
        ErrorCode::InUse
    } else if m.contains("no such camera") {
        ErrorCode::NotFound
    } else {
        ErrorCode::Failed
    }
}

/// A real backend plus the test pattern device (`--test-pattern`).
pub struct Combined {
    real: Box<dyn Backend>,
    pattern: TestPattern,
    pattern_open: bool,
}

impl Combined {
    pub fn new(real: Box<dyn Backend>) -> Self {
        Self { real, pattern: TestPattern::default(), pattern_open: false }
    }
}

impl Backend for Combined {
    fn list(&mut self) -> Result<Vec<DeviceInfo>, BackendError> {
        // A failing camera API must not hide the test pattern.
        let mut devices = self.real.list().unwrap_or_else(|(_, message)| {
            eprintln!("camera list failed: {message}");
            Vec::new()
        });
        devices.extend(self.pattern.list()?);
        Ok(devices)
    }

    fn open(&mut self, device: &str, index: u32, width: u32, height: u32) -> Result<(u32, u32), BackendError> {
        self.close();
        if device == TEST_PATTERN_NAME {
            let size = self.pattern.open(device, index, width, height)?;
            self.pattern_open = true;
            Ok(size)
        } else {
            self.real.open(device, index, width, height)
        }
    }

    fn grab(&mut self) -> Result<Vec<u8>, BackendError> {
        if self.pattern_open { self.pattern.grab() } else { self.real.grab() }
    }

    fn controls(&mut self) -> Result<Vec<ControlInfo>, BackendError> {
        if self.pattern_open { self.pattern.controls() } else { self.real.controls() }
    }

    fn set_control(&mut self, v: &ControlValue) -> Result<(), BackendError> {
        if self.pattern_open { self.pattern.set_control(v) } else { self.real.set_control(v) }
    }

    fn close(&mut self) {
        self.pattern.close();
        self.pattern_open = false;
        self.real.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_camera_is_in_use() {
        let e = "Could not capture frame: Hardware MFT failed to start streaming due to lack of hardware resources. (0xC00D3704)";
        assert_eq!(classify_error(e), ErrorCode::InUse);
    }

    #[test]
    fn missing_camera_is_not_found() {
        assert_eq!(classify_error("Could not open device No Such Camera: Not Found"), ErrorCode::NotFound);
        assert_eq!(classify_error("Element not found. (0x80070490)"), ErrorCode::Failed);
    }

    #[test]
    fn anything_else_fails() {
        assert_eq!(classify_error("decode error"), ErrorCode::Failed);
    }
}
