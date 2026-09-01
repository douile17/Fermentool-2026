//! Fermentool control daemon (binary shell over the `fermentool_core` library).
//!
//! Milestone 4 (this file): resolve the data dir, run a self-test that exercises
//! the curve engine, the pump simulator, and the SQLite store.
//!
//! Still to come (see `docs/IMPLEMENTATION_PLAN.md` §8):
//!   5. tick engine (`engine/`)          7. HTTP API + config (`api/`, `config.rs`)
//!   6. crash recovery / resume          8. embedded Svelte UI

use std::path::PathBuf;
use std::time::Duration;

use fermentool_core::store::{ControlVar, Direction, NewRun, Store};
use fermentool_curves::CurveSpec;
use fermentool_modbus::{Pump, PumpTransport, SimPump};
use jiff::Timestamp;

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

    base.unwrap_or_else(|| PathBuf::from("."))
        .join("Fermentool")
}

fn main() {
    println!("{NAME} {VERSION}");
    println!("data dir : {}", data_dir().display());
    println!("status   : scaffold (milestone 4) — control engine not yet wired");

    // Curve -> pump self-test.
    let curve =
        CurveSpec::linear(5.0, 50.0, Duration::from_secs(100 * 3600)).with_clamp(0.1, 350.0);
    let at_50h = curve.value_at(Duration::from_secs(50 * 3600));
    let mut pump = Pump::new(SimPump::new(1), 1);
    pump.set_direction(true).ok();
    pump.set_speed_rpm(at_50h as f32).ok();
    pump.start().ok();
    let readback = pump.read_speed_rpm().unwrap_or(f32::NAN);

    // Store self-test (in-memory).
    let store = Store::open_in_memory().expect("open store");
    let run_id = store
        .insert_run(&NewRun {
            name: "self-test".into(),
            started_at: Timestamp::now(),
            control_var: ControlVar::Rpm,
            direction: Direction::Cw,
            tick_interval_s: 10,
            pump_addr: 1,
            pump_head: None,
            tubing: None,
            app_version: VERSION.into(),
            curve,
        })
        .expect("insert run");
    store.integrity_check().expect("integrity");

    println!(
        "self-test: midpoint {at_50h:.1} rpm, sim running={} readback={readback:.1}, run #{run_id} stored ok",
        pump.transport().running()
    );
}
