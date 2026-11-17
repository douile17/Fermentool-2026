//! The helper's request loop: one request, one reply, in order.

use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::time::Duration;

use crate::backend::Backend;
use crate::protocol::{read_request, write_reply, Reply, Request};

/// Answers requests until `Quit` or the daemon closes the pipe, then closes
/// the device.
pub fn serve(mut input: impl Read, mut output: impl Write, backend: &mut dyn Backend) -> io::Result<()> {
    let result = loop {
        let request = match read_request(&mut input) {
            Ok(Some(Request::Quit)) | Ok(None) => break Ok(()),
            Ok(Some(r)) => r,
            Err(e) => break Err(e),
        };
        let reply = match request {
            Request::List => backend.list().map(Reply::Devices),
            Request::Open { device, index, width, height, controls } => {
                backend.close();
                backend.open(&device, index, width, height).map(|(width, height)| {
                    for v in &controls {
                        if let Err((_, e)) = backend.set_control(v) {
                            eprintln!("camera {device}: {} not set: {e}", v.control.name());
                        }
                    }
                    Reply::Opened { width, height }
                })
            }
            Request::Grab => backend.grab().map(Reply::Frame),
            Request::Still { quality } => backend.still(quality).map(|(jpeg, thumb)| Reply::Still { jpeg, thumb }),
            Request::Controls => backend.controls().map(Reply::Controls),
            Request::SetControl(v) => backend.set_control(&v).and_then(|()| backend.controls()).map(Reply::Controls),
            Request::Close => {
                backend.close();
                Ok(Reply::Closed)
            }
            Request::Quit => unreachable!(),
        };
        let reply = reply.unwrap_or_else(|(code, message)| Reply::Error { code, message });
        if let Err(e) = write_reply(&mut output, &reply) {
            break Err(e);
        }
    };
    backend.close();
    result
}

/// Bytes read on another thread, handed over by a channel. EOF once that
/// thread stops sending.
pub struct ChannelReader {
    rx: mpsc::Receiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos >= self.buf.len() {
            match self.rx.recv() {
                Ok(b) => {
                    self.buf = b;
                    self.pos = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Reads `input` (the helper's stdin) on a thread of its own and returns a
/// reader of its bytes for [`serve`]. At EOF or a read error the reader sees
/// EOF too, then `on_eof` runs after `grace`, whatever `serve` is doing: the
/// helper exits even when a grab never returns (nokhwa after an unplug), so
/// a daemon that died never leaves the webcam held. The grace lets a `Quit`
/// just before the EOF end the helper the normal way.
pub fn forward_input(
    mut input: impl Read + Send + 'static,
    grace: Duration,
    on_eof: impl FnOnce() + Send + 'static,
) -> ChannelReader {
    let (tx, rx) = mpsc::channel();
    // Should the thread not start, its sender is dropped with it: serve
    // reads EOF at once and the helper ends.
    let _ = std::thread::Builder::new().name("stdin".into()).spawn(move || {
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            match input.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(chunk[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        drop(tx);
        std::thread::sleep(grace);
        on_eof();
    });
    ChannelReader { rx, buf: Vec::new(), pos: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{read_reply, write_request, Control, ControlInfo, ControlValue, ErrorCode};
    use crate::test_pattern::TestPattern;
    use std::io::Cursor;

    fn run(requests: &[Request]) -> (io::Result<()>, Vec<Reply>) {
        let mut input = Vec::new();
        for r in requests {
            write_request(&mut input, r).unwrap();
        }
        let mut output = Vec::new();
        let result = serve(Cursor::new(input), &mut output, &mut TestPattern::default());
        let mut replies = Vec::new();
        let mut cur = Cursor::new(output);
        while (cur.position() as usize) < cur.get_ref().len() {
            replies.push(read_reply(&mut cur).unwrap());
        }
        (result, replies)
    }

    fn open(device: &str) -> Request {
        Request::Open { device: device.into(), index: 0, width: 640, height: 480, controls: Vec::new() }
    }

    fn brightness(r: &Reply) -> Option<ControlInfo> {
        match r {
            Reply::Controls(c) => c.iter().find(|c| c.control == Control::Brightness).copied(),
            _ => None,
        }
    }

    #[test]
    fn open_applies_its_controls_and_set_control_answers_them() {
        let set = |value| ControlValue { control: Control::Brightness, value, auto: false };
        let (_, replies) = run(&[
            Request::Open {
                device: "Test pattern".into(),
                index: 0,
                width: 640,
                height: 480,
                // the refused one (focus) leaves brightness applied
                controls: vec![ControlValue { control: Control::Focus, value: 1, auto: false }, set(20)],
            },
            Request::Controls,
            Request::SetControl(set(-10)),
            Request::SetControl(set(999)),
        ]);
        assert_eq!(replies[0], Reply::Opened { width: 640, height: 480 });
        assert_eq!(brightness(&replies[1]).unwrap().value, 20);
        assert_eq!(brightness(&replies[2]).unwrap().value, -10);
        assert!(matches!(&replies[3], Reply::Error { code: ErrorCode::Failed, .. }));
    }

    #[test]
    fn serve_lists_opens_grabs_and_quits() {
        let (result, replies) = run(&[Request::List, open("Test pattern"), Request::Grab, Request::Quit]);
        assert!(result.is_ok());
        assert_eq!(replies.len(), 3);
        match &replies[0] {
            Reply::Devices(d) => assert_eq!(d.len(), 1),
            r => panic!("{r:?}"),
        }
        assert_eq!(replies[1], Reply::Opened { width: 640, height: 480 });
        match &replies[2] {
            Reply::Frame(jpeg) => assert_eq!(&jpeg[..2], &[0xFF, 0xD8]),
            r => panic!("{r:?}"),
        }
    }

    #[test]
    fn serve_returns_on_eof() {
        let (result, replies) = run(&[Request::List]);
        assert!(result.is_ok());
        assert_eq!(replies.len(), 1);
    }

    #[test]
    fn grab_before_open_is_an_error() {
        let (_, replies) = run(&[Request::Grab, Request::List]);
        assert!(matches!(&replies[0], Reply::Error { code: ErrorCode::Failed, .. }));
        assert!(matches!(&replies[1], Reply::Devices(_)));
    }

    #[test]
    fn unknown_device_is_not_found() {
        let (_, replies) = run(&[open("Nope")]);
        assert!(matches!(&replies[0], Reply::Error { code: ErrorCode::NotFound, .. }));
    }

    /// A device whose grab never returns, like nokhwa after an unplug.
    struct Stuck;

    impl Backend for Stuck {
        fn list(&mut self) -> Result<Vec<crate::protocol::DeviceInfo>, crate::backend::BackendError> {
            Ok(Vec::new())
        }
        fn open(&mut self, _: &str, _: u32, w: u32, h: u32) -> Result<(u32, u32), crate::backend::BackendError> {
            Ok((w, h))
        }
        fn grab(&mut self) -> Result<Vec<u8>, crate::backend::BackendError> {
            loop {
                std::thread::park();
            }
        }
        fn close(&mut self) {}
    }

    fn requests(all: &[Request]) -> Vec<u8> {
        let mut input = Vec::new();
        for r in all {
            write_request(&mut input, r).unwrap();
        }
        input
    }

    #[test]
    fn forwarded_input_serves_like_the_pipe_and_reports_eof() {
        let (eof_tx, eof_rx) = std::sync::mpsc::channel();
        let input = forward_input(Cursor::new(requests(&[open("Test pattern"), Request::Grab])), GRACE, move || {
            let _ = eof_tx.send(());
        });
        let mut output = Vec::new();
        assert!(serve(input, &mut output, &mut TestPattern::default()).is_ok());
        let mut cur = Cursor::new(output);
        assert_eq!(read_reply(&mut cur).unwrap(), Reply::Opened { width: 640, height: 480 });
        assert!(matches!(read_reply(&mut cur).unwrap(), Reply::Frame(_)));
        assert!(eof_rx.recv_timeout(GRACE + Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn eof_ends_even_a_stuck_grab() {
        let (eof_tx, eof_rx) = std::sync::mpsc::channel();
        let started = std::time::Instant::now();
        let input = forward_input(Cursor::new(requests(&[open("Cam"), Request::Grab])), GRACE, move || {
            let _ = eof_tx.send(());
        });
        // serve stays stuck in the grab forever; the EOF action still runs.
        std::thread::spawn(move || serve(input, io::sink(), &mut Stuck));
        assert!(eof_rx.recv_timeout(GRACE + Duration::from_secs(2)).is_ok());
        assert!(started.elapsed() >= GRACE);
    }

    const GRACE: Duration = Duration::from_millis(100);

    #[test]
    fn still_returns_a_big_and_a_small_jpeg() {
        let (_, replies) = run(&[open("Test pattern"), Request::Still { quality: 95 }]);
        match &replies[1] {
            Reply::Still { jpeg, thumb } => {
                assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
                assert_eq!(&thumb[..2], &[0xFF, 0xD8]);
                assert!(thumb.len() < jpeg.len());
            }
            r => panic!("{r:?}"),
        }
    }

    #[test]
    fn still_before_open_is_an_error() {
        let (_, replies) = run(&[Request::Still { quality: 95 }]);
        assert!(matches!(&replies[0], Reply::Error { code: ErrorCode::Failed, .. }));
    }

    #[test]
    fn the_bar_moves_between_frames() {
        let (_, replies) = run(&[open("Test pattern"), Request::Grab, Request::Grab]);
        assert_ne!(replies[1], replies[2]);
    }
}
