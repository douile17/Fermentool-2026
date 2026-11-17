//! `fermentool-camera.exe`: the daemon's camera helper. Requests on stdin,
//! replies on stdout (the protocol and nothing else), diagnostics on stderr.

use std::io::{BufWriter, Write};
use std::time::Duration;

use fermentool_camera::backend::{Backend, Combined};
use fermentool_camera::mf_backend::MfBackend;
use fermentool_camera::serve::{forward_input, serve};

/// After stdin closes, how long a `Quit` sent just before has to end the
/// helper the normal way before it exits regardless.
const EOF_GRACE: Duration = Duration::from_millis(300);

fn main() {
    let test_pattern = std::env::args().any(|a| a == "--test-pattern");
    let mut backend: Box<dyn Backend> = if test_pattern {
        Box::new(Combined::new(Box::new(MfBackend::default())))
    } else {
        Box::new(MfBackend::default())
    };
    // The daemon closing our stdin (it quit, crashed or was killed) ends
    // this process within EOF_GRACE, even with a grab stuck in the driver.
    let input = forward_input(std::io::stdin(), EOF_GRACE, || std::process::exit(0));
    let stdout = std::io::stdout().lock();
    let mut output = BufWriter::new(stdout);
    // serve flushes after every reply (write_reply) and has closed the device.
    let result = serve(input, &mut output, backend.as_mut());
    let _ = output.flush();
    if let Err(e) = &result {
        eprintln!("fermentool-camera: {e}");
    }
    // Dropping the Media Foundation objects at a normal exit segfaulted in the
    // spike (exit 139), so leave without running destructors.
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}
