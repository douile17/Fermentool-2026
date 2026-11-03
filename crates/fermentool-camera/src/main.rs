//! `fermentool-camera.exe`: the daemon's camera helper. Requests on stdin,
//! replies on stdout (the protocol and nothing else), diagnostics on stderr.

use std::io::{BufReader, BufWriter, Write};

use fermentool_camera::backend::{Backend, Combined};
use fermentool_camera::nokhwa_backend::NokhwaBackend;
use fermentool_camera::serve::serve;

fn main() {
    let test_pattern = std::env::args().any(|a| a == "--test-pattern");
    let mut backend: Box<dyn Backend> = if test_pattern {
        Box::new(Combined::new(Box::new(NokhwaBackend::default())))
    } else {
        Box::new(NokhwaBackend::default())
    };
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    let mut output = BufWriter::new(stdout);
    // serve flushes after every reply (write_reply) and has closed the device.
    let result = serve(BufReader::new(stdin), &mut output, backend.as_mut());
    let _ = output.flush();
    if let Err(e) = &result {
        eprintln!("fermentool-camera: {e}");
    }
    // Dropping the Media Foundation objects at a normal exit segfaulted in the
    // spike (exit 139), so leave without running destructors.
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}
