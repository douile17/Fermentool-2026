; Tauri v2 NSIS installer hooks. Wired in via
; tauri.conf.json -> bundle.windows.nsis.installerHooks.

; Updating over a running daemon would leave its exe locked and not replaced
; (the old version keeps running). Stop it cleanly first, and never during a
; run: refuse instead. Exit codes: 0 = go on, 2 = a run is active.
!macro NSIS_HOOK_PREINSTALL
  DetailPrint "Stopping a running Fermentool daemon before installing..."
  nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -Command "try { $$s = Invoke-RestMethod -Uri http://127.0.0.1:8730/api/status -TimeoutSec 2 } catch { exit 0 }; if ($$s.active) { exit 2 }; try { Invoke-WebRequest -UseBasicParsing -Method POST -Uri http://127.0.0.1:8730/api/shutdown -TimeoutSec 2 | Out-Null } catch {}; for ($$i = 0; $$i -lt 40; $$i++) { if (-not (Get-Process fermentool-core -ErrorAction SilentlyContinue)) { exit 0 }; Start-Sleep -Milliseconds 250 }; exit 3"'
  Pop $0
  StrCmp $0 "2" 0 +3
    MessageBox MB_ICONSTOP "A run is in progress in Fermentool. Stop it, then run this installer again."
    Abort
!macroend

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Registering the Fermentool log-on task..."
  nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\setup-task.ps1" -InstallDir "$INSTDIR"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Removing the Fermentool log-on task and stopping the daemon..."
  nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\remove-task.ps1"'
!macroend
