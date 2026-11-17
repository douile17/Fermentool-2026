# Build the UI, the fermentool-core daemon and the fermentool-camera helper, then
# stage both exes where Tauri expects the sidecars (name suffixed with the target
# triple). Run this before `cargo tauri dev` or `cargo tauri build`.
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
Invoke-Step "building fermentool-camera ($triple, release, capture)" {
    cargo build --release -p fermentool-camera --features capture --target $triple
}

$dstDir = Join-Path $srcTauri "binaries"
New-Item -ItemType Directory -Force $dstDir | Out-Null

# Both exes are staged with the target triple in their name; Tauri installs
# them next to fermentool.exe without it. A fresh PC has no Visual C++
# Redistributable: neither may need it (see .cargo/config.toml, +crt-static).
foreach ($name in @("fermentool-core", "fermentool-camera")) {
    $src = Join-Path $root "target\$triple\release\$name.exe"
    $dst = Join-Path $dstDir "$name-$triple.exe"
    if (-not (Test-Path $src)) { throw "$name.exe was not built: $src" }
    if (Select-String -Path $src -Pattern "VCRUNTIME140" -SimpleMatch -Quiet) {
        throw "$name.exe still depends on VCRUNTIME140.dll: check .cargo/config.toml"
    }
    Copy-Item -Force $src $dst
    Write-Host "==> sidecar staged: $dst"
}
