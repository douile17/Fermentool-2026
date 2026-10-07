//! System-tray icon. The window closes to the tray so the daemon keeps running
//! (a ~100 h run must survive the operator closing the UI). Quitting the daemon
//! is a deliberate menu choice.

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

use crate::daemon;

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Fermentool", true, None::<&str>)?;
    let quit_daemon = MenuItem::with_id(
        app,
        "quit_daemon",
        "Shut down daemon && quit",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        "Quit (leave daemon running)",
        true,
        None::<&str>,
    )?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &sep, &quit_daemon, &quit])?;

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("Fermentool")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            "quit_daemon" => {
                // Off the event loop: the question blocks until answered.
                let app = app.clone();
                std::thread::spawn(move || {
                    if confirm_shutdown() {
                        let _ = daemon::shutdown();
                        app.exit(0);
                    }
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Shutting the daemon down during a run ends its regulation: the pump keeps
/// its last speed, uncorrected and unrecorded, and nothing restarts the daemon
/// by itself (the log-on task only does after a crash). Asked first, then.
fn confirm_shutdown() -> bool {
    let Some(name) = daemon::active_run() else {
        return true;
    };
    ask_ok_cancel(
        "Fermentool",
        &format!(
            "Run \"{name}\" is in progress. Shutting the daemon down stops its regulation: the pump \
             stays at its last speed, uncorrected and unrecorded, until Fermentool is started again.\n\n\
             Shut the daemon down anyway?"
        ),
    )
}

/// A plain Windows OK / Cancel warning box, Cancel by default. Win32
/// directly: a dialog plugin would have pulled a newer Tauri into the build.
#[cfg(windows)]
fn ask_ok_cancel(title: &str, text: &str) -> bool {
    use std::ffi::c_void;
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: *mut c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }
    const MB_OKCANCEL: u32 = 0x1;
    const MB_ICONWARNING: u32 = 0x30;
    const MB_DEFBUTTON2: u32 = 0x100;
    const MB_SETFOREGROUND: u32 = 0x1_0000;
    const IDOK: i32 = 1;
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (text, title) = (wide(text), wide(title));
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call;
    // a null owner window is allowed.
    let answer = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2 | MB_SETFOREGROUND,
        )
    };
    answer == IDOK
}

#[cfg(not(windows))]
fn ask_ok_cancel(_title: &str, _text: &str) -> bool {
    true
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}
