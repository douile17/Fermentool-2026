//! The helper's request loop: one request, one reply, in order.

use std::io::{self, Read, Write};

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
            Request::Open { device, index, width, height } => {
                backend.close();
                backend.open(&device, index, width, height).map(|(width, height)| Reply::Opened { width, height })
            }
            Request::Grab => backend.grab().map(Reply::Frame),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{read_reply, write_request, ErrorCode};
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
        Request::Open { device: device.into(), index: 0, width: 640, height: 480 }
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

    #[test]
    fn the_bar_moves_between_frames() {
        let (_, replies) = run(&[open("Test pattern"), Request::Grab, Request::Grab]);
        assert_ne!(replies[1], replies[2]);
    }
}
