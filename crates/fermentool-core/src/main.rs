//! Fermentool control daemon.
//!
//! Milestone 7 (this file): load `config.toml`, start file logging, open the
//! journal, build the engine (real serial port or the simulator), run it on the
//! control thread, and serve the REST API on `127.0.0.1:<port>`.
//!
//! Still to come (see `docs/IMPLEMENTATION_PLAN.md` §8):
//!   8. embedded Svelte UI + WebSocket push
//!   9. per-OS single-binary packaging + service files

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

fn init_tracing(level: &str, log_dir: &Path) -> anyhow::Result<WorkerGuard> {
    use tracing_subscriber::prelude::*;

    std::fs::create_dir_all(log_dir).context("create log dir")?;
    let file = tracing_appender::rolling::daily(log_dir, "fermentool.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file);

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
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
            anyhow::bail!(
                "port {} is already in use — another Fermentool instance may be running, \
                 or change `port` in {}",
                config.port,
                paths.config.display()
            );
        }
        Err(e) => return Err(e).context("bind listener"),
    };
    tracing::info!("API listening on http://{addr}");

    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(shutdown_signal(shutdown))
        .await
        .context("http server")?;

    control.shutdown();
    tracing::info!("stopped cleanly");
    Ok(())
}
