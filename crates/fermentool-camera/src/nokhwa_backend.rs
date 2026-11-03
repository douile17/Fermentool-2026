//! The webcam, through Media Foundation (nokhwa).

use jpeg_encoder::{ColorType, Encoder};
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType, Resolution};
use nokhwa::{query, Camera};

use crate::backend::{classify_error, Backend, BackendError, GLOBAL_SIZES, JPEG_QUALITY};
use crate::protocol::{DeviceInfo, ErrorCode};

#[derive(Default)]
pub struct NokhwaBackend {
    camera: Option<Camera>,
}

/// Dropping a nokhwa `Camera` crashes this process (access violation, seen in
/// the spike as exit 139): stop its stream and forget it. One small leak per
/// open, and opens are rare.
fn leak(camera: Camera) {
    std::mem::forget(camera);
}

fn failed(e: impl std::fmt::Display) -> BackendError {
    let message = e.to_string();
    (classify_error(&message), message)
}

/// Opens the camera at the size asked, in a pixel format it really offers there
/// (the bench webcam has NV12 at 1280 x 720 but YUYV at 640 x 480). The format
/// is chosen from the camera's own list: asking for one it lacks makes
/// nokhwa fail inside its constructor, and that failure crashed the process.
fn new_camera(id: &CameraIndex, width: u32, height: u32) -> Result<Camera, BackendError> {
    let mut camera = Camera::new(id.clone(), RequestedFormat::new::<RgbFormat>(RequestedFormatType::None)).map_err(failed)?;
    let formats = camera.compatible_camera_formats().map_err(failed)?;
    let size = Resolution::new(width, height);
    let best = formats.into_iter().filter(|f| f.resolution() == size).max_by_key(|f| f.frame_rate());
    let Some(format) = best else {
        leak(camera);
        return Err((ErrorCode::Failed, format!("no {width} x {height} picture on this camera")));
    };
    if let Err(e) = camera.set_camera_requset(RequestedFormat::new::<RgbFormat>(RequestedFormatType::Exact(format))) {
        leak(camera);
        return Err(failed(e));
    }
    Ok(camera)
}

/// The nokhwa index of the `index`-th device named `name`.
fn find_device(name: &str, index: u32) -> Result<CameraIndex, BackendError> {
    let devices = query(ApiBackend::MediaFoundation).map_err(failed)?;
    devices
        .iter()
        .filter(|d| d.human_name() == name)
        .nth(index as usize)
        .map(|d| d.index().clone())
        .ok_or((ErrorCode::NotFound, format!("no camera named {name} (number {index})")))
}

fn grab_rgb(camera: &mut Camera) -> Result<Vec<u8>, BackendError> {
    let frame = camera.frame().map_err(failed)?;
    let image = frame.decode_image::<RgbFormat>().map_err(failed)?;
    let mut jpeg = Vec::new();
    Encoder::new(&mut jpeg, JPEG_QUALITY)
        .encode(image.as_raw(), image.width() as u16, image.height() as u16, ColorType::Rgb)
        .map_err(failed)?;
    Ok(jpeg)
}

impl Backend for NokhwaBackend {
    fn list(&mut self) -> Result<Vec<DeviceInfo>, BackendError> {
        let devices = query(ApiBackend::MediaFoundation).map_err(failed)?;
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        for d in &devices {
            let name = d.human_name();
            let index = seen.iter().filter(|n| **n == name).count() as u32;
            seen.push(name.clone());
            // Only the sizes of the global list the camera reports; a camera
            // that will not even tell is listed with none.
            let sizes = match Camera::new(d.index().clone(), RequestedFormat::new::<RgbFormat>(RequestedFormatType::None)) {
                Ok(mut c) => {
                    let formats = c.compatible_camera_formats().unwrap_or_default();
                    leak(c);
                    GLOBAL_SIZES
                        .iter()
                        .copied()
                        .filter(|&(w, h)| formats.iter().any(|f| f.resolution() == Resolution::new(w, h)))
                        .collect()
                }
                Err(e) => {
                    eprintln!("camera {name}: {e}");
                    Vec::new()
                }
            };
            out.push(DeviceInfo { name, index, sizes });
        }
        Ok(out)
    }

    fn open(&mut self, device: &str, index: u32, width: u32, height: u32) -> Result<(u32, u32), BackendError> {
        self.close();
        let id = find_device(device, index)?;
        let mut camera = new_camera(&id, width, height)?;
        camera.open_stream().map_err(failed)?;
        // A camera held by another app opens fine and fails at the first frame.
        if let Err(e) = grab_rgb(&mut camera) {
            let _ = camera.stop_stream();
            leak(camera);
            return Err(e);
        }
        let size = camera.resolution();
        self.camera = Some(camera);
        Ok((size.width(), size.height()))
    }

    fn grab(&mut self) -> Result<Vec<u8>, BackendError> {
        let camera = self.camera.as_mut().ok_or((ErrorCode::Failed, "no camera is open".to_string()))?;
        grab_rgb(camera)
    }

    fn close(&mut self) {
        if let Some(mut camera) = self.camera.take() {
            let _ = camera.stop_stream();
            leak(camera);
        }
    }
}
