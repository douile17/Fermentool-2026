//! A device that needs no webcam: a grey field with a bar that moves a step
//! per frame, so a live view visibly lives.

use jpeg_encoder::{ColorType, Encoder};

use crate::backend::{Backend, BackendError, GLOBAL_SIZES, JPEG_QUALITY};
use crate::protocol::{DeviceInfo, ErrorCode};

pub const TEST_PATTERN_NAME: &str = "Test pattern";

#[derive(Default)]
pub struct TestPattern {
    size: Option<(u32, u32)>,
    frame: u32,
}

impl Backend for TestPattern {
    fn list(&mut self) -> Result<Vec<DeviceInfo>, BackendError> {
        Ok(vec![DeviceInfo { name: TEST_PATTERN_NAME.into(), index: 0, sizes: GLOBAL_SIZES.to_vec() }])
    }

    fn open(&mut self, device: &str, index: u32, width: u32, height: u32) -> Result<(u32, u32), BackendError> {
        if device != TEST_PATTERN_NAME || index != 0 {
            return Err((ErrorCode::NotFound, format!("no camera named {device}")));
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
        let mut rgb = vec![128u8; (w * h * 3) as usize];
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

    fn close(&mut self) {
        self.size = None;
    }
}
