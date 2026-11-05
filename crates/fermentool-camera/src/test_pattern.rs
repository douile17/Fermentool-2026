//! A device that needs no webcam: a grey field with a bar that moves a step
//! per frame, so a live view visibly lives. Its brightness control shifts the
//! grey, and its exposure has an auto mode, so Settings can be tried on it.

use jpeg_encoder::{ColorType, Encoder};

use crate::backend::{Backend, BackendError, GLOBAL_SIZES, JPEG_QUALITY};
use crate::protocol::{Control, ControlInfo, ControlValue, DeviceInfo, ErrorCode};

pub const TEST_PATTERN_NAME: &str = "Test pattern";

pub struct TestPattern {
    size: Option<(u32, u32)>,
    frame: u32,
    controls: [ControlInfo; 2],
}

impl Default for TestPattern {
    fn default() -> Self {
        let info = |control, min, max, default, can_auto| ControlInfo {
            control,
            min,
            max,
            step: 1,
            default,
            value: default,
            can_auto,
            auto: can_auto,
        };
        Self {
            size: None,
            frame: 0,
            controls: [info(Control::Exposure, -11, -2, -6, true), info(Control::Brightness, -64, 64, 0, false)],
        }
    }
}

impl Backend for TestPattern {
    fn list(&mut self) -> Result<Vec<DeviceInfo>, BackendError> {
        Ok(vec![DeviceInfo { name: TEST_PATTERN_NAME.into(), index: 0, sizes: GLOBAL_SIZES.to_vec() }])
    }

    fn open(&mut self, device: &str, index: u32, width: u32, height: u32) -> Result<(u32, u32), BackendError> {
        if device != TEST_PATTERN_NAME || index != 0 {
            return Err((ErrorCode::NotFound, format!("no camera named {device}")));
        }
        if !GLOBAL_SIZES.contains(&(width, height)) {
            return Err((ErrorCode::Failed, format!("no {width} x {height} picture on this device")));
        }
        self.size = Some((width, height));
        self.frame = 0;
        Ok((width, height))
    }

    fn grab(&mut self) -> Result<Vec<u8>, BackendError> {
        let (w, h) = self.size.ok_or((ErrorCode::Failed, "no camera is open".to_string()))?;
        let bar_w = (w / 10).max(1);
        let bar_x = (self.frame * (w / 40).max(1)) % w;
        self.frame = self.frame.wrapping_add(1);
        let grey = (128 + self.controls[1].value).clamp(0, 255) as u8;
        let mut rgb = vec![grey; (w * h * 3) as usize];
        for y in 0..h {
            for x in bar_x..(bar_x + bar_w).min(w) {
                let i = ((y * w + x) * 3) as usize;
                rgb[i..i + 3].copy_from_slice(&[230, 230, 230]);
            }
        }
        let mut jpeg = Vec::new();
        Encoder::new(&mut jpeg, JPEG_QUALITY)
            .encode(&rgb, w as u16, h as u16, ColorType::Rgb)
            .map_err(|e| (ErrorCode::Failed, e.to_string()))?;
        Ok(jpeg)
    }

    fn controls(&mut self) -> Result<Vec<ControlInfo>, BackendError> {
        self.size.ok_or((ErrorCode::Failed, "no camera is open".to_string()))?;
        Ok(self.controls.to_vec())
    }

    fn set_control(&mut self, v: &ControlValue) -> Result<(), BackendError> {
        let c = self
            .controls
            .iter_mut()
            .find(|c| c.control == v.control)
            .ok_or((ErrorCode::Failed, format!("this camera has no {} control", v.control.name())))?;
        if !(c.min..=c.max).contains(&v.value) || (v.auto && !c.can_auto) {
            return Err((ErrorCode::Failed, format!("{} {} is out of range", v.control.name(), v.value)));
        }
        c.value = if v.auto { c.default } else { v.value };
        c.auto = v.auto;
        Ok(())
    }

    fn close(&mut self) {
        self.size = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_outside_the_global_list_are_refused() {
        let mut p = TestPattern::default();
        for (w, h) in [(0, 0), (0, 480), (100_000, 100_000)] {
            assert_eq!(p.open(TEST_PATTERN_NAME, 0, w, h).unwrap_err().0, ErrorCode::Failed);
        }
        assert!(p.grab().is_err());
    }
}
