# Run by the NSIS preUninstall hook. Removes the scheduled task and asks a
# running daemon to stop so the uninstaller doesn't leave an orphan on :8730.
schtasks /delete /tn "Fermentool" /f 2>$null

try {
    Invoke-WebRequest -UseBasicParsing -Method POST `
        -Uri "http://127.0.0.1:8730/api/shutdown" -TimeoutSec 2 | Out-Null
} catch {}
