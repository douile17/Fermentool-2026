//! Camera helper: the pipe protocol the daemon speaks to `fermentool-camera.exe`,
//! and the loop that answers it over any `Backend`.

pub mod backend;
#[cfg(feature = "capture")]
pub mod mf_backend;
pub mod pixels;
pub mod protocol;
pub mod serve;
pub mod test_pattern;
