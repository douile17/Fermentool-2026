# Run by the NSIS preUninstall hook, once the daemon is stopped (the hook
# refuses to uninstall during a run, see hooks.nsh). Removes the log-on task.
schtasks /delete /tn "Fermentool" /f 2>$null
exit 0
