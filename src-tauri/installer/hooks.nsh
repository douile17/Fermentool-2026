; Tauri v2 NSIS installer hooks. Wired in via
; tauri.conf.json -> bundle.windows.nsis.installerHooks.

; Updating over a running daemon would leave its exe locked and not replaced
; (the old version keeps running). Stop it cleanly first, and never during a
; run: refuse instead. Exit codes of stop-daemon.ps1: 0 = go on (stopped, or
; none running), 2 = a run is active, 3 = the daemon did not stop, 4 = a daemon
; is running but does not answer.
!macro FERMENTOOL_STOP_DAEMON WHAT
  nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -File "$PLUGINSDIR\stop-daemon.ps1"'
  Pop $0
  StrCmp $0 "2" 0 +3
    MessageBox MB_ICONSTOP "A run is in progress in Fermentool. Stop it, then run the ${WHAT} again."
    Abort
  StrCmp $0 "3" 0 +3
    MessageBox MB_ICONSTOP "The Fermentool daemon did not stop. Use Settings, Daemon, Shut down daemon (or the tray menu), then run the ${WHAT} again."
    Abort
  StrCmp $0 "4" 0 +3
    MessageBox MB_ICONSTOP "The Fermentool daemon is running but does not answer, so the ${WHAT} cannot tell whether a run is in progress. Check Fermentool, stop the daemon, then run the ${WHAT} again."
    Abort
!macroend

!macro FERMENTOOL_WRITE_STOP_SCRIPT
  InitPluginsDir
  FileOpen $1 "$PLUGINSDIR\stop-daemon.ps1" w
  FileWrite $1 "$$base = 'http://127.0.0.1:8730'$\r$\n"
  FileWrite $1 "try { $$s = Invoke-RestMethod -Uri ($$base + '/api/status') -TimeoutSec 5 } catch {$\r$\n"
  FileWrite $1 "  if (Get-Process fermentool-core -ErrorAction SilentlyContinue) { exit 4 } else { exit 0 }$\r$\n"
  FileWrite $1 "}$\r$\n"
  FileWrite $1 "if ($$s.active) { exit 2 }$\r$\n"
  FileWrite $1 "try { Invoke-WebRequest -UseBasicParsing -Method POST -Uri ($$base + '/api/shutdown') -TimeoutSec 5 | Out-Null } catch {}$\r$\n"
  FileWrite $1 "for ($$i = 0; $$i -lt 60; $$i++) {$\r$\n"
  FileWrite $1 "  if (-not (Get-Process fermentool-core -ErrorAction SilentlyContinue)) { exit 0 }$\r$\n"
  FileWrite $1 "  Start-Sleep -Milliseconds 250$\r$\n"
  FileWrite $1 "}$\r$\n"
  FileWrite $1 "exit 3$\r$\n"
  FileClose $1
!macroend

!macro NSIS_HOOK_PREINSTALL
  DetailPrint "Stopping a running Fermentool daemon before installing..."
  !insertmacro FERMENTOOL_WRITE_STOP_SCRIPT
  !insertmacro FERMENTOOL_STOP_DAEMON "installer"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Registering the Fermentool log-on task..."
  nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\setup-task.ps1" -InstallDir "$INSTDIR"'
!macroend

; Same guard when uninstalling: never in the middle of a run, and only once
; the daemon has really let go of its exe.
!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Stopping the Fermentool daemon and removing its log-on task..."
  !insertmacro FERMENTOOL_WRITE_STOP_SCRIPT
  !insertmacro FERMENTOOL_STOP_DAEMON "uninstaller"
  nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\remove-task.ps1"'
!macroend
