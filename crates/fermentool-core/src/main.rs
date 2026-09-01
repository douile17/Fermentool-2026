//! Fermentool control daemon (binary shell over the `fermentool_core` library).
//!
//! Milestone 5 (this file): resolve the data dir, then run a self-test that
//! drives the whole engine — curve, pump simulator, SQLite journal — across a
//! synthetic 100 h run.
//!
//! Still to come (see `docs/IMPLEMENTATION_PLAN.md` §8):
//!   6. crash recovery / resume          7. HTTP API + config (`api/`, `config.rs`)
//!   8. embedded Svelte UI               9. per-OS single-binary packaging

use std::path::PathBuf;
use std::time::Duration;

use fermentool_core::engine::{Engine, RunConfig, TickOutcome};
use fermentool_core::store::{ControlVar, Direction, Store};
use fermentool_curves::CurveSpec;
use fermentool_modbus::{Pump, SimPump};
use jiff::{SignedDuration, Timestamp};

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
    println!(
        "status   : scaffold (milestone 6) — engine + crash recovery ready, daemon wiring next"
    );

    let mut engine = Engine::new(
        Pump::new(SimPump::new(1), 1),
        Store::open_in_memory().expect("store"),
        VERSION,
    );

    let start = Timestamp::now();
    let run_id = engine
        .start_run(
            RunConfig {
                name: "self-test".into(),
                control_var: ControlVar::Rpm,
                direction: Direction::Cw,
                tick_interval_s: 10,
                pump_addr: 1,
                pump_head: None,
                tubing: None,
                curve: CurveSpec::linear(5.0, 50.0, Duration::from_secs(100 * 3600))
                    .with_clamp(0.1, 350.0),
            },
            start,
        )
        .expect("start run");

    // Synthetic ticks across the whole 100 h run.
    let mut last = 0.0;
    for h in 0i64..=100 {
        if let Ok(TickOutcome::Applied { target, .. } | TickOutcome::Finished { target, .. }) =
            engine.tick(start + SignedDuration::from_secs(h * 3600))
        {
            last = target;
        }
    }

    let ticks = engine.store().tick_count(run_id).unwrap_or(0);
    println!("self-test: run #{run_id}, {ticks} ticks journalled, final setpoint {last:.1} rpm");
}
