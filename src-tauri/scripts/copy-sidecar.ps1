# Build the UI and the fermentool-core daemon, then stage the daemon exe where
# Tauri expects the sidecar (name suffixed with the target triple). Run this
# before `cargo tauri dev` or `cargo tauri build`.
$ErrorActionPreference = "Stop"

$scriptDir = $PSScriptRoot
$srcTauri  = Split-Path -Parent $scriptDir
$root      = Split-Path -Parent $srcTauri
$triple    = "x86_64-pc-windows-msvc"

Write-Host "==> building ui/dist"
& npm --prefix "$root\ui" ci
& npm --prefix "$root\ui" run build

Write-Host "==> building fermentool-core ($triple, release)"
& cargo build --release -p fermentool-core --target $triple

$src = Join-Path $root "target\$triple\release\fermentool-core.exe"
$dstDir = Join-Path $srcTauri "binaries"
$dst = Join-Path $dstDir "fermentool-core-$triple.exe"

New-Item -ItemType Directory -Force $dstDir | Out-Null
Copy-Item -Force $src $dst
Write-Host "==> sidecar staged: $dst"
