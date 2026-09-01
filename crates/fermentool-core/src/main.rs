//! Fermentool control daemon.
//!
//! Milestone 1 (this file): a bootstrap shell that resolves the runtime data
//! directory and prints build info. It links the sibling crates so the
//! workspace wiring is exercised at build time.
//!
//! Still to come (see `docs/IMPLEMENTATION_PLAN.md` §8):
//!   4. SQLite store (`store/`)          7. HTTP API + config (`api/`, `config.rs`)
//!   5. tick engine (`engine/`)          8. embedded Svelte UI
//!   6. crash recovery / resume          9. per-OS single-binary packaging

use std::path::PathBuf;
use std::time::Duration;

use fermentool_curves::{CurveKind, CurveSpec, ParamMode};

const NAME: &str = env!("CARGO_PKG_NAME");
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default runtime data directory (DB, logs, config). Milestone 7 replaces this
/// with a config-driven path.
fn data_dir() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);

    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));

    base.unwrap_or_else(|| PathBuf::from(".")).join("Fermentool")
}

fn main() {
    println!("{NAME} {VERSION}");
    println!("data dir : {}", data_dir().display());
    println!("status   : scaffold (milestone 1) — control engine not yet wired");

    // Smoke-check the workspace wiring.
    let demo = CurveSpec {
        kind: CurveKind::Linear,
        mode: ParamMode::Endpoints,
        start: 5.0,
        end: 50.0,
        duration: Duration::from_secs(100 * 3600),
        clamp_min: 0.1,
        clamp_max: 350.0,
    };
    let at_50h = demo.value_at(Duration::from_secs(50 * 3600));
    let start_frame = fermentool_modbus::with_crc(vec![0x01, 0x06, 0x03, 0xEE, 0x00, 0x01]);
    println!("self-test: linear midpoint = {at_50h:.1} rpm, start frame = {start_frame:02X?}");
}
