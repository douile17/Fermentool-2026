# Running Fermentool as a background service

> Stub - filled in at milestone 9. The daemon already survives its own panics
> (internal `catch_unwind` + restart); an OS service is the outer safety net so
> it also comes back after a reboot or power cut.

## Linux - systemd

`/etc/systemd/system/fermentool.service`:

```ini
[Unit]
Description=Fermentool pump controller
After=network.target

[Service]
ExecStart=/opt/fermentool/fermentool-core
Restart=always
RestartSec=3
WorkingDirectory=/opt/fermentool

[Install]
WantedBy=multi-user.target
```

`sudo systemctl enable --now fermentool`

## macOS - launchd

`~/Library/LaunchAgents/com.fermentool.core.plist` with `KeepAlive = true` and
`RunAtLoad = true`, then `launchctl load` it.

## Windows - the Fermentool app (recommended)

The installer (`Fermentool_<version>_x64-setup.exe`, built with `cargo tauri
build`) does the service side for you:

- installs `fermentool.exe` (the window + tray) and `fermentool-core.exe`
  (the daemon);
- registers a scheduled task **Fermentool** - trigger "at log on", run level
  highest available, `RestartOnFailure` every 1 min up to 3 times.

So after a reboot the daemon is back within seconds and - if a run was
interrupted - the app shows the resume prompt when you open it. Closing the
window leaves the daemon running; use the tray menu's *Shut down daemon &
quit* to stop it. Uninstalling removes the task and stops the daemon.

Manual check: `schtasks /query /tn Fermentool`.

The notes below are only for a **headless** machine with no interactive
login (the app's scheduled task is per-user, `onlogon`).

## Windows - headless (Task Scheduler / NSSM)

- **Task Scheduler**: "At startup" trigger, action = `fermentool-core.exe`,
  settings → "If the task fails, restart every 1 minute".
- or **NSSM** (`nssm install Fermentool <path>\fermentool-core.exe`) to run it
  as a real service under a service account.

After a restart the daemon reads the journal and, if a run was active, shows the
resume prompt in the web UI.
