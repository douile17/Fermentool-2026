# Running Fermentool as a background service

> Stub — filled in at milestone 9. The daemon already survives its own panics
> (internal `catch_unwind` + restart); an OS service is the outer safety net so
> it also comes back after a reboot or power cut.

## Linux — systemd

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

## macOS — launchd

`~/Library/LaunchAgents/com.fermentool.core.plist` with `KeepAlive = true` and
`RunAtLoad = true`, then `launchctl load` it.

## Windows

- **Task Scheduler**: "At log on" trigger, action = the binary, settings →
  "If the task fails, restart every 1 minute".
- or **NSSM** (`nssm install Fermentool ...`) to run it as a real service.

After a restart the daemon reads the journal and, if a run was active, shows the
resume prompt in the web UI.
