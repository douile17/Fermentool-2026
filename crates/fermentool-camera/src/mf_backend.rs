//! The webcam, through Media Foundation directly (a source reader in
//! synchronous mode, RGB32 out, MJPEG and YUV converted by Media Foundation).
//!
//! It replaced nokhwa (2026-11-05): with nokhwa the "HUE HD Camera" crashed
//! this process 3 to 5 s after every open (access violation in
//! FrameServerClient.dll) while the Windows Camera app showed it fine, and
//! nokhwa could neither release a webcam nor drop one without crashing.

use std::ptr;

use jpeg_encoder::{ColorType, Encoder};
use windows::core::{Error, PWSTR};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

use crate::backend::{classify_error, Backend, BackendError, GLOBAL_SIZES, JPEG_QUALITY};
use crate::pixels::bgrx_to_rgb;
use crate::protocol::{DeviceInfo, ErrorCode};

const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const ALL_STREAMS: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
/// Empty reads (stream ticks) a grab tolerates before giving up.
const MAX_EMPTY_READS: u32 = 50;

/// The HRESULT in the text, as `classify_error` reads it (0xC00D3704: held
/// by another app).
fn failed(e: Error) -> BackendError {
    let message = format!("{} (0x{:08X})", e.message(), e.code().0 as u32);
    (classify_error(&message), message)
}

struct Open {
    activate: IMFActivate,
    source: IMFMediaSource,
    reader: IMFSourceReader,
    width: u32,
    height: u32,
    stride: i32,
}

#[derive(Default)]
pub struct MfBackend {
    started: bool,
    open: Option<Open>,
}

impl MfBackend {
    fn start(&mut self) -> Result<(), BackendError> {
        if !self.started {
            unsafe {
                // MTA: no message loop needed on this thread.
                CoInitializeEx(None, COINIT_MULTITHREADED).ok().map_err(failed)?;
                MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(failed)?;
            }
            self.started = true;
        }
        Ok(())
    }
}

/// The video capture devices, in Windows' order, with their names.
fn devices() -> Result<Vec<(String, IMFActivate)>, BackendError> {
    unsafe {
        let mut attr = None;
        MFCreateAttributes(&mut attr, 1).map_err(failed)?;
        let attr = attr.ok_or((ErrorCode::Failed, "no attribute store".to_string()))?;
        attr.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)
            .map_err(failed)?;
        let mut list: *mut Option<IMFActivate> = ptr::null_mut();
        let mut count = 0u32;
        MFEnumDeviceSources(&attr, &mut list, &mut count).map_err(failed)?;
        let mut out = Vec::new();
        if !list.is_null() {
            for slot in std::slice::from_raw_parts_mut(list, count as usize) {
                if let Some(activate) = slot.take() {
                    out.push((friendly_name(&activate), activate));
                }
            }
            CoTaskMemFree(Some(list as *const _));
        }
        Ok(out)
    }
}

fn friendly_name(activate: &IMFActivate) -> String {
    unsafe {
        let mut name = PWSTR::null();
        let mut len = 0u32;
        if activate.GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &mut name, &mut len).is_err() {
            return String::new();
        }
        let s = name.to_string().unwrap_or_default();
        CoTaskMemFree(Some(name.0 as *const _));
        s
    }
}

/// A source reader on the device, the video stream only selected.
fn reader_for(activate: &IMFActivate) -> Result<(IMFMediaSource, IMFSourceReader), BackendError> {
    unsafe {
        let source: IMFMediaSource = activate.ActivateObject().map_err(failed)?;
        let mut attr = None;
        MFCreateAttributes(&mut attr, 1).map_err(failed)?;
        let attr = attr.ok_or((ErrorCode::Failed, "no attribute store".to_string()))?;
        // lets the reader decode MJPEG and convert YUV to RGB32
        attr.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).map_err(failed)?;
        let reader = match MFCreateSourceReaderFromMediaSource(&source, &attr) {
            Ok(r) => r,
            Err(e) => {
                let _ = source.Shutdown();
                return Err(failed(e));
            }
        };
        let _ = reader.SetStreamSelection(ALL_STREAMS, false);
        reader.SetStreamSelection(VIDEO, true).map_err(failed)?;
        Ok((source, reader))
    }
}

/// The device's own formats: (width, height, frames per second, the type).
fn native_types(reader: &IMFSourceReader) -> Vec<(u32, u32, f64, IMFMediaType)> {
    let mut out = Vec::new();
    for i in 0.. {
        let Ok(t) = (unsafe { reader.GetNativeMediaType(VIDEO, i) }) else { break };
        let Ok(size) = (unsafe { t.GetUINT64(&MF_MT_FRAME_SIZE) }) else { continue };
        let fps = unsafe { t.GetUINT64(&MF_MT_FRAME_RATE) }
            .map(|r| (r >> 32) as f64 / ((r & 0xffff_ffff).max(1)) as f64)
            .unwrap_or(0.0);
        out.push(((size >> 32) as u32, size as u32, fps, t));
    }
    out
}

/// Lets go of the device: the reader, then the source, then the activation.
fn release(source: &IMFMediaSource, activate: &IMFActivate) {
    unsafe {
        let _ = source.Shutdown();
        let _ = activate.ShutdownObject();
    }
}

fn open_device(activate: IMFActivate, width: u32, height: u32) -> Result<Open, BackendError> {
    let (source, reader) = reader_for(&activate)?;
    let result = (|| unsafe {
        let native = native_types(&reader)
            .into_iter()
            .filter(|(w, h, _, _)| (*w, *h) == (width, height))
            .max_by(|a, b| a.2.total_cmp(&b.2))
            .ok_or((ErrorCode::Failed, format!("no {width} x {height} picture on this camera")))?;
        reader.SetCurrentMediaType(VIDEO, None, &native.3).map_err(failed)?;
        let rgb = MFCreateMediaType().map_err(failed)?;
        rgb.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(failed)?;
        rgb.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(failed)?;
        reader.SetCurrentMediaType(VIDEO, None, &rgb).map_err(failed)?;
        let now = reader.GetCurrentMediaType(VIDEO).map_err(failed)?;
        let size = now.GetUINT64(&MF_MT_FRAME_SIZE).map_err(failed)?;
        let (w, h) = ((size >> 32) as u32, size as u32);
        // RGB in Media Foundation is bottom-up unless the type says otherwise
        let stride = now.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(-((w * 4) as i32));
        Ok((w, h, stride))
    })();
    match result {
        Ok((width, height, stride)) => Ok(Open { activate, source, reader, width, height, stride }),
        Err(e) => {
            drop(reader);
            release(&source, &activate);
            Err(e)
        }
    }
}

fn grab_jpeg(open: &Open) -> Result<Vec<u8>, BackendError> {
    unsafe {
        let mut sample = None;
        for _ in 0..MAX_EMPTY_READS {
            let mut flags = 0u32;
            open.reader
                .ReadSample(VIDEO, 0, None, Some(&mut flags), None, Some(&mut sample))
                .map_err(failed)?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err((ErrorCode::Failed, "the camera stream failed".to_string()));
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return Err((ErrorCode::Failed, "the camera stream ended".to_string()));
            }
            if sample.is_some() {
                break;
            }
        }
        let sample: IMFSample = sample.ok_or((ErrorCode::Failed, "no picture from the camera".to_string()))?;
        let buffer = sample.ConvertToContiguousBuffer().map_err(failed)?;
        let mut data = ptr::null_mut();
        let mut len = 0u32;
        buffer.Lock(&mut data, None, Some(&mut len)).map_err(failed)?;
        let rgb = if data.is_null() {
            None
        } else {
            bgrx_to_rgb(std::slice::from_raw_parts(data, len as usize), open.width, open.height, open.stride)
        };
        let _ = buffer.Unlock();
        let rgb = rgb.ok_or((ErrorCode::Failed, format!("a picture of {len} bytes, too short")))?;
        let mut jpeg = Vec::new();
        Encoder::new(&mut jpeg, JPEG_QUALITY)
            .encode(&rgb, open.width as u16, open.height as u16, ColorType::Rgb)
            .map_err(|e| (ErrorCode::Failed, e.to_string()))?;
        Ok(jpeg)
    }
}

impl Backend for MfBackend {
    fn list(&mut self) -> Result<Vec<DeviceInfo>, BackendError> {
        self.start()?;
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        for (name, activate) in devices()? {
            let index = seen.iter().filter(|n| **n == name).count() as u32;
            seen.push(name.clone());
            // Only the sizes of the global list the camera reports; a camera
            // that will not even tell is listed with none.
            let sizes = match reader_for(&activate) {
                Ok((source, reader)) => {
                    let native = native_types(&reader);
                    drop(reader);
                    release(&source, &activate);
                    GLOBAL_SIZES.iter().copied().filter(|&(w, h)| native.iter().any(|n| (n.0, n.1) == (w, h))).collect()
                }
                Err((_, e)) => {
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
        self.start()?;
        let activate = devices()?
            .into_iter()
            .filter(|(n, _)| n == device)
            .nth(index as usize)
            .map(|(_, a)| a)
            .ok_or((ErrorCode::NotFound, format!("no camera named {device} (number {index})")))?;
        let open = open_device(activate, width, height)?;
        // A camera held by another app opens fine and fails at the first frame.
        if let Err(e) = grab_jpeg(&open) {
            close_device(open);
            return Err(e);
        }
        let size = (open.width, open.height);
        self.open = Some(open);
        Ok(size)
    }

    fn grab(&mut self) -> Result<Vec<u8>, BackendError> {
        let open = self.open.as_ref().ok_or((ErrorCode::Failed, "no camera is open".to_string()))?;
        grab_jpeg(open)
    }

    fn close(&mut self) {
        if let Some(open) = self.open.take() {
            close_device(open);
        }
    }
}

fn close_device(open: Open) {
    let Open { activate, source, reader, .. } = open;
    drop(reader);
    release(&source, &activate);
}
