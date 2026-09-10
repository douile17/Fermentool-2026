// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod daemon;

use std::time::Duration;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let handle = app.handle().clone();
            // Bring the daemon up (or attach to a running one), then reveal the
            // window. Off the main thread so the UI event loop starts now and
            // the window can paint its "reconnecting…" state meanwhile.
            std::thread::spawn(move || {
                if !daemon::is_up() {
                    match daemon::sidecar_path(&handle) {
                        Some(exe) => {
                            if let Err(e) = daemon::spawn_detached(&exe) {
                                eprintln!("could not start fermentool-core: {e}");
                            }
                        }
                        None => eprintln!("fermentool-core sidecar not found"),
                    }
                    daemon::wait_until_up(Duration::from_secs(15));
                }
                if let Some(w) = handle.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Fermentool");
}
