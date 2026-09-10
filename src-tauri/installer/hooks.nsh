; Tauri v2 NSIS installer hooks. Wired in via
; tauri.conf.json -> bundle.windows.nsis.installerHooks.

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Registering the Fermentool log-on task..."
  nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\setup-task.ps1" -InstallDir "$INSTDIR"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Removing the Fermentool log-on task and stopping the daemon..."
  nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\remove-task.ps1"'
!macroend
