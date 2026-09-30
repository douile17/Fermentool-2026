# Build the UI and the fermentool-core daemon, then stage the daemon exe where
# Tauri expects the sidecar (name suffixed with the target triple). Run this
# before `cargo tauri dev` or `cargo tauri build`.
#
# Native tools (npm, cargo) write their normal progress to stderr. Windows
# PowerShell 5.1 turns that into errors under "Stop", so failures are checked
# through $LASTEXITCODE instead.
$ErrorActionPreference = "Continue"

$scriptDir = $PSScriptRoot
$srcTauri  = Split-Path -Parent $scriptDir
$root      = Split-Path -Parent $srcTauri
$triple    = "x86_64-pc-windows-msvc"

function Invoke-Step($what, [scriptblock]$cmd) {
    Write-Host "==> $what"
    & $cmd 2>&1 | ForEach-Object { "$_" }
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}

Invoke-Step "npm ci (ui)" { npm --prefix "$root\ui" ci }
Invoke-Step "building ui/dist" { npm --prefix "$root\ui" run build }
Invoke-Step "building fermentool-core ($triple, release, static CRT)" {
    cargo build --release -p fermentool-core --target $triple
}

$src = Join-Path $root "target\$triple\release\fermentool-core.exe"
$dstDir = Join-Path $srcTauri "binaries"
$dst = Join-Path $dstDir "fermentool-core-$triple.exe"

# A fresh PC has no Visual C++ Redistributable: the sidecar must not need it
# (see .cargo/config.toml, +crt-static).
if (Select-String -Path $src -Pattern "VCRUNTIME140" -SimpleMatch -Quiet) {
    throw "fermentool-core.exe still depends on VCRUNTIME140.dll: check .cargo/config.toml"
}

New-Item -ItemType Directory -Force $dstDir | Out-Null
Copy-Item -Force $src $dst
Write-Host "==> sidecar staged: $dst"
