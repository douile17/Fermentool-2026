//! Fermentool control daemon.
//!
//! Milestone 7 (this file): load `config.toml`, start file logging, open the
//! journal, build the engine (real serial port or the simulator), run it on the
//! control thread, and serve the REST API on `127.0.0.1:<port>`.
//!
//! Still to come (see `docs/IMPLEMENTATION_PLAN.md` §8):
//!   8. embedded Svelte UI + WebSocket push
//!   9. per-OS single-binary packaging + service files

// Release builds run as a windowless background service: no console flashes up
// when the user double-clicks the exe. Debug builds keep the console so
// `cargo run` still shows live logs. Either way the file logger is the record.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use tokio::sync::{broadcast, Notify, RwLock};
use tracing_appender::non_blocking::WorkerGuard;

use fermentool_core::api::{self, AppState};
use fermentool_core::config::Config;
use fermentool_core::control;
use fermentool_core::engine::Engine;
use fermentool_core::store::Store;
use fermentool_modbus::{Pump, Transport};

const VERSION: &str = env!("CARGO_PKG_VERSION");

struct Paths {
    data_dir: PathBuf,
    config: PathBuf,
    db: PathBuf,
    log_dir: PathBuf,
}

fn default_data_dir() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);

    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));

    base.unwrap_or_else(|| PathBuf::from("."))
        .join("Fermentool")
}

fn resolve_paths(config_dir_override: Option<&str>) -> Paths {
    let data_dir = match config_dir_override {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => default_data_dir(),
    };
    Paths {
        config: data_dir.join("config.toml"),
        db: data_dir.join("fermentool.sqlite"),
        log_dir: data_dir.join("logs"),
        data_dir,
    }
}

/// Best-effort: open `url` in the machine's default browser. Fire-and-forget —
/// the daemon never waits on it and a failure only means the user opens the page
/// themselves. On Windows `explorer.exe <url>` launches the browser without a
/// console window flashing up.
#[cfg(not(debug_assertions))]
fn open_in_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let (bin, arg) = ("explorer.exe", url.to_string());
    #[cfg(target_os = "macos")]
    let (bin, arg) = ("open", url.to_string());
    #[cfg(all(unix, not(target_os = "macos")))]
    let (bin, arg) = ("xdg-open", url.to_string());

    match std::process::Command::new(bin).arg(arg).spawn() {
        Ok(_) => tracing::info!("opened {url} in the default browser"),
        Err(e) => tracing::warn!("could not open a browser ({e}); open {url} manually"),
    }
}

/// Delete rolled `fermentool.log.*` files older than `retain_days`. Best-effort:
/// `tracing_appender`'s daily roller creates files but never removes them, so
/// over months of 100 h runs the log dir would grow without bound. `0` disables.
fn prune_old_logs(log_dir: &Path, retain_days: u32) {
    if retain_days == 0 {
        return;
    }
    let cutoff = match std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(u64::from(retain_days) * 86_400))
    {
        Some(t) => t,
        None => return,
    };
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // Keep the live file (`fermentool.log`); only prune the dated rolls.
        if !name.starts_with("fermentool.log.") {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|m| m < cutoff)
            .unwrap_or(false);
        if old {
            match std::fs::remove_file(entry.path()) {
                Ok(()) => tracing::info!("pruned old log {name}"),
                Err(e) => tracing::warn!("could not prune {name}: {e}"),
            }
        }
    }
}

fn init_tracing(level: &str, log_dir: &Path) -> anyhow::Result<WorkerGuard> {
    use tracing_subscriber::prelude::*;

    std::fs::create_dir_all(log_dir).context("create log dir")?;
    let file = tracing_appender::rolling::daily(log_dir, "fermentool.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file);

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));

    // The console layer only makes sense when there's a console: debug builds.
    // Release builds are windowless (`windows_subsystem = "windows"`), so writing
    // to a detached stderr is pointless — the daily file is the record.
    let console = cfg!(debug_assertions)
        .then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stderr));

    tracing_subscriber::registry()
        .with(filter)
        .with(console)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file_writer),
        )
        .init();

    Ok(guard)
}

fn build_engine(cfg: &Config, db: &Path) -> anyhow::Result<Engine<Box<dyn Transport + Send>>> {
    let store = Store::open(db).context("open journal")?;
    let (transport, kind) = fermentool_core::transport::open(&cfg.serial, cfg.pump.address);
    let mut engine = Engine::new(Pump::new(transport, cfg.pump.address), store, VERSION);
    engine.set_transport_kind(kind);
    engine.set_serial(cfg.serial.clone(), cfg.pump.address);
    // No pump I/O on the startup path — a blocking MODBUS probe here would delay
    // the HTTP server (and the auto-opened browser). `set_serial` already flags
    // a port that won't open; the control loop's idle probe confirms an
    // open-but-silent port within a few seconds and latches the alarm then.
    Ok(engine)
}

async fn shutdown_signal(via_api: Arc<Notify>) {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => tracing::info!("ctrl-c received"),
        _ = via_api.notified() => {}
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // First pass with defaults so we can find config.toml; then re-resolve using
    // any storage.dir it sets.
    let bootstrap = resolve_paths(None);
    let config = Config::load_or_create(&bootstrap.config)
        .with_context(|| format!("load {}", bootstrap.config.display()))?;
    let paths = resolve_paths(Some(&config.storage.dir));
    let config = Config::load_or_create(&paths.config)?;

    let log_dir = if config.log.dir.is_empty() {
        paths.log_dir.clone()
    } else {
        PathBuf::from(&config.log.dir)
    };
    let _log_guard = init_tracing(&config.log.level, &log_dir)?;
    prune_old_logs(&log_dir, config.log.retain_days);

    tracing::info!("fermentool-core {VERSION} starting");
    tracing::info!(data_dir = %paths.data_dir.display(), "paths resolved");

    let engine = build_engine(&config, &paths.db)?;
    let (events, _) = broadcast::channel(64);
    let control = Arc::new(control::spawn(engine, config.grace(), events.clone()));
    let shutdown = Arc::new(Notify::new());

    let state = AppState {
        control: Arc::clone(&control),
        config: Arc::new(RwLock::new(config.clone())),
        config_path: Arc::new(paths.config.clone()),
        shutdown: Arc::clone(&shutdown),
        events,
    };

    let addr = SocketAddr::from(([127, 0, 0, 1], config.port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            // Log it too: a windowless release build has no console to print to,
            // and this is the usual "I double-clicked it twice" case.
            tracing::error!(
                "port {} already in use — Fermentool may already be running",
                config.port
            );
            anyhow::bail!(
                "port {} is already in use — another Fermentool instance may be running, \
                 or change `port` in {}",
                config.port,
                paths.config.display()
            );
        }
        Err(e) => return Err(e).context("bind listener"),
    };
    let url = format!("http://{addr}");
    tracing::info!("API listening on {url}");

    // Release builds have no console, so pop the UI in the default browser.
    #[cfg(not(debug_assertions))]
    open_in_browser(&url);

    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(shutdown_signal(shutdown))
        .await
        .context("http server")?;

    control.shutdown();
    tracing::info!("stopped cleanly");
    Ok(())
}
