//! Talking to `fermentool-core` over its local HTTP API, and starting it as a
//! process that outlives this window.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

const BASE: &str = "http://127.0.0.1:8730";

/// `true` iff the daemon answers `GET /api/status` with 200 inside 500 ms.
pub fn is_up() -> bool {
    ureq::get(&format!("{BASE}/api/status"))
        .timeout(Duration::from_millis(500))
        .call()
        .map(|r| r.status() == 200)
        .unwrap_or(false)
}

/// Poll [`is_up`] every 200 ms until it succeeds or `timeout` elapses.
pub fn wait_until_up(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if is_up() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Best-effort `POST /api/shutdown`. `true` if the daemon accepted it.
pub fn shutdown() -> bool {
    ureq::post(&format!("{BASE}/api/shutdown"))
        .timeout(Duration::from_secs(2))
        .call()
        .is_ok()
}

/// Start the daemon so it keeps running after this process exits.
///
/// `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`, no job object, and the
/// [`std::process::Child`] handle is dropped immediately — a ~100 h run must
/// survive the window closing (or the shell being killed).
#[cfg(windows)]
pub fn spawn_detached(exe: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    let child = Command::new(exe)
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .current_dir(exe.parent().unwrap_or_else(|| Path::new(".")))
        .spawn()?;
    drop(child); // never wait on it
    Ok(())
}

#[cfg(not(windows))]
pub fn spawn_detached(exe: &Path) -> io::Result<()> {
    let child = Command::new(exe).spawn()?;
    drop(child);
    Ok(())
}

/// Locate the bundled `fermentool-core` executable across the modes it can run
/// in: an installed NSIS bundle (sidecar sits next to the app exe, name without
/// the target triple), and `cargo tauri dev` (triple-suffixed, under the
/// crate's `binaries/`, put there by `scripts/copy-sidecar.ps1`).
pub fn sidecar_path(app: &AppHandle) -> Option<PathBuf> {
    let plain = if cfg!(windows) {
        "fermentool-core.exe"
    } else {
        "fermentool-core"
    };
    let suffixed = if cfg!(windows) {
        "fermentool-core-x86_64-pc-windows-msvc.exe"
    } else {
        "fermentool-core-x86_64-unknown-linux-gnu"
    };

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        candidates.push(dir.join(plain));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(plain));
            candidates.push(dir.join(suffixed));
        }
    }
    // `cargo tauri dev`: the sidecar is not copied anywhere near the target dir.
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(suffixed),
    );

    candidates.into_iter().find(|p| p.exists())
}
