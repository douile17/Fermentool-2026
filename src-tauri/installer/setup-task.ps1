# Run by the NSIS postInstall hook. Substitutes the installed daemon path into
# the task template and registers the "at log on" scheduled task.
param([Parameter(Mandatory = $true)][string]$InstallDir)

$ErrorActionPreference = "Stop"
$xml = Join-Path $InstallDir "fermentool-task.xml"
$exe = Join-Path $InstallDir "fermentool-core.exe"

(Get-Content -Raw $xml).Replace("{{EXE}}", $exe) |
    Set-Content -Encoding Unicode $xml

schtasks /create /tn "Fermentool" /xml "$xml" /f
