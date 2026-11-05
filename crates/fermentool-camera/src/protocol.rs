//! Messages between the daemon and the camera helper over its stdin/stdout.
//!
//! Framing: 4-byte little-endian length (kind byte + payload), 1 kind byte
//! (`J` JSON, `F` raw JPEG), then the payload. A frame travels as raw bytes,
//! never JSON, so a 100 kB picture costs 5 bytes of overhead.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// Largest accepted message (kind byte + payload). Checked before allocating.
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;

const KIND_JSON: u8 = b'J';
const KIND_FRAME: u8 = b'F';

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    List,
    /// `controls` are set right after the open, each on its own: one the
    /// camera refuses leaves the others applied.
    Open {
        device: String,
        index: u32,
        width: u32,
        height: u32,
        #[serde(default)]
        controls: Vec<ControlValue>,
    },
    Grab,
    /// The open camera's picture controls, as it has them now.
    Controls,
    /// Sets one control, answered by `Controls` as they are after it.
    SetControl(ControlValue),
    Close,
    Quit,
}

/// A picture control the app offers, where the camera has it. Windows
/// numbers them in two families (`IAMCameraControl`: exposure, focus;
/// `IAMVideoProcAmp`: the rest), in each camera's own units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Control {
    /// log2 of seconds: -6 is 1/64 s.
    Exposure,
    /// Kelvin.
    WhiteBalance,
    Focus,
    Brightness,
    Contrast,
    Saturation,
    Sharpness,
    Gain,
    /// 0 off, 1 50 Hz, 2 60 Hz, 3 auto (where the camera has it).
    PowerLine,
}

impl Control {
    /// In the order Settings shows them.
    pub const ALL: [Control; 9] = [
        Control::Exposure,
        Control::WhiteBalance,
        Control::Focus,
        Control::Brightness,
        Control::Contrast,
        Control::Saturation,
        Control::Sharpness,
        Control::Gain,
        Control::PowerLine,
    ];

    /// Its name in JSON and in `config.toml`.
    pub fn name(self) -> &'static str {
        match self {
            Control::Exposure => "exposure",
            Control::WhiteBalance => "white_balance",
            Control::Focus => "focus",
            Control::Brightness => "brightness",
            Control::Contrast => "contrast",
            Control::Saturation => "saturation",
            Control::Sharpness => "sharpness",
            Control::Gain => "gain",
            Control::PowerLine => "power_line",
        }
    }

    pub fn from_name(name: &str) -> Option<Control> {
        Control::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// A control to set: `auto` hands it to the camera (`value` is then kept
/// for when auto goes off).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlValue {
    pub control: Control,
    pub value: i32,
    #[serde(default)]
    pub auto: bool,
}

/// A control as the camera has it: its range and default, and its value now
/// (in auto, the value the camera has chosen).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlInfo {
    pub control: Control,
    pub min: i32,
    pub max: i32,
    pub step: i32,
    pub default: i32,
    pub value: i32,
    pub can_auto: bool,
    pub auto: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub index: u32,
    pub sizes: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ErrorCode {
    NotFound,
    InUse,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Reply {
    Devices(Vec<DeviceInfo>),
    Opened { width: u32, height: u32 },
    /// A JPEG picture, sent as kind `F` with the bytes as payload.
    Frame(Vec<u8>),
    Controls(Vec<ControlInfo>),
    Closed,
    Error { code: ErrorCode, message: String },
}

fn write_message(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let len = payload.len() + 1;
    if len > MAX_MESSAGE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let mut buf = Vec::with_capacity(4 + len);
    buf.extend_from_slice(&(len as u32).to_le_bytes());
    buf.push(kind);
    buf.extend_from_slice(payload);
    w.write_all(&buf)?;
    w.flush()
}

/// One message, `None` on a clean EOF before its first byte.
fn read_message(r: &mut impl Read) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut head = [0u8; 4];
    let mut got = 0;
    while got < head.len() {
        match r.read(&mut head[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let len = u32::from_le_bytes(head) as usize;
    if len == 0 || len > MAX_MESSAGE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad message length"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let kind = body[0];
    body.remove(0);
    Ok(Some((kind, body)))
}

fn json_err(e: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

pub fn write_request(w: &mut impl Write, r: &Request) -> io::Result<()> {
    write_message(w, KIND_JSON, &serde_json::to_vec(r).map_err(json_err)?)
}

/// `None` on a clean EOF (the daemon closed the pipe).
pub fn read_request(r: &mut impl Read) -> io::Result<Option<Request>> {
    match read_message(r)? {
        None => Ok(None),
        Some((KIND_JSON, p)) => serde_json::from_slice(&p).map(Some).map_err(json_err),
        Some(_) => Err(io::Error::new(io::ErrorKind::InvalidData, "unknown message kind")),
    }
}

pub fn write_reply(w: &mut impl Write, r: &Reply) -> io::Result<()> {
    match r {
        Reply::Frame(jpeg) => write_message(w, KIND_FRAME, jpeg),
        other => write_message(w, KIND_JSON, &serde_json::to_vec(other).map_err(json_err)?),
    }
}

pub fn read_reply(r: &mut impl Read) -> io::Result<Reply> {
    match read_message(r)? {
        None => Err(io::ErrorKind::UnexpectedEof.into()),
        Some((KIND_FRAME, p)) => Ok(Reply::Frame(p)),
        Some((KIND_JSON, p)) => serde_json::from_slice(&p).map_err(json_err),
        Some(_) => Err(io::Error::new(io::ErrorKind::InvalidData, "unknown message kind")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, ErrorKind};

    #[test]
    fn request_round_trips() {
        let all = [
            Request::List,
            Request::Open { device: "Cam".into(), index: 2, width: 1280, height: 720, controls: Vec::new() },
            Request::Open {
                device: "Cam".into(),
                index: 0,
                width: 640,
                height: 480,
                controls: vec![ControlValue { control: Control::Exposure, value: -6, auto: false }],
            },
            Request::Grab,
            Request::Controls,
            Request::SetControl(ControlValue { control: Control::WhiteBalance, value: 4600, auto: true }),
            Request::Close,
            Request::Quit,
        ];
        for r in all {
            let mut buf = Vec::new();
            write_request(&mut buf, &r).unwrap();
            assert_eq!(read_request(&mut Cursor::new(buf)).unwrap(), Some(r));
        }
    }

    #[test]
    fn json_replies_round_trip() {
        let all = [
            Reply::Devices(vec![DeviceInfo { name: "Cam".into(), index: 0, sizes: vec![(640, 480)] }]),
            Reply::Opened { width: 640, height: 480 },
            Reply::Controls(vec![ControlInfo {
                control: Control::PowerLine,
                min: 0,
                max: 2,
                step: 1,
                default: 1,
                value: 1,
                can_auto: false,
                auto: false,
            }]),
            Reply::Closed,
            Reply::Error { code: ErrorCode::InUse, message: "busy".into() },
        ];
        for r in all {
            let mut buf = Vec::new();
            write_reply(&mut buf, &r).unwrap();
            assert_eq!(read_reply(&mut Cursor::new(buf)).unwrap(), r);
        }
    }

    #[test]
    fn open_without_controls_still_parses() {
        let r: Request =
            serde_json::from_str(r#"{"Open":{"device":"Cam","index":0,"width":640,"height":480}}"#).unwrap();
        assert_eq!(r, Request::Open { device: "Cam".into(), index: 0, width: 640, height: 480, controls: Vec::new() });
    }

    #[test]
    fn control_names_match_their_json() {
        for c in Control::ALL {
            assert_eq!(serde_json::to_value(c).unwrap(), serde_json::json!(c.name()));
            assert_eq!(Control::from_name(c.name()), Some(c));
        }
        assert_eq!(Control::from_name("zoom"), None);
    }

    #[test]
    fn frame_reply_round_trips_without_json() {
        let jpeg: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let mut buf = Vec::new();
        write_reply(&mut buf, &Reply::Frame(jpeg.clone())).unwrap();
        assert_eq!(buf.len(), 100_000 + 5);
        assert_eq!(read_reply(&mut Cursor::new(buf)).unwrap(), Reply::Frame(jpeg));
    }

    #[test]
    fn clean_eof_is_none() {
        assert_eq!(read_request(&mut Cursor::new(Vec::new())).unwrap(), None);
    }

    #[test]
    fn truncated_message_is_an_error() {
        let mut buf = 100u32.to_le_bytes().to_vec();
        buf.extend_from_slice(&[b'J'; 10]);
        let e = read_request(&mut Cursor::new(buf)).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::UnexpectedEof);
    }

    #[test]
    fn oversized_length_is_refused() {
        let buf = (MAX_MESSAGE as u32 + 1).to_le_bytes().to_vec();
        let e = read_request(&mut Cursor::new(buf)).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn unknown_kind_is_invalid_data() {
        let mut buf = 2u32.to_le_bytes().to_vec();
        buf.extend_from_slice(b"Xa");
        let e = read_reply(&mut Cursor::new(buf)).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::InvalidData);
    }
}
