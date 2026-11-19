[CmdletBinding()]
param([switch] $DebugBuild, [string] $Toolchain = 'nightly-2026-06-09')

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$workspace = Split-Path $PSScriptRoot -Parent
$profile = if ($DebugBuild) { 'debug' } else { 'release' }
$cargoOutput = Join-Path $workspace 'target/gpui-web-cargo'
$public = Join-Path $workspace "target/gpui-web/$profile/public"
$generation = [guid]::NewGuid().ToString('N')
$outputRoot = Split-Path $public -Parent
$staging = Join-Path $outputRoot "stage-$generation"
$previous = Join-Path $outputRoot "old-$generation"
function Assert-BundlePath([string] $bundlePath) {
    $path = [IO.Path]::GetFullPath($bundlePath)
    $relative = [IO.Path]::GetRelativePath($workspace, $path).Replace('\', '/')
    if ($relative -notmatch "^target/gpui-web/$profile/(public|(?:stage|old)-[0-9a-f]{32})$") {
        throw "Unexpected GPUI output path $path"
    }
    for ($ancestor = $path; $ancestor -ne $workspace; $ancestor = [IO.Path]::GetDirectoryName($ancestor)) {
        if ((Test-Path -LiteralPath $ancestor) -and ((Get-Item -LiteralPath $ancestor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Refusing to stage GPUI assets through a link at $ancestor"
        }
    }
    if ((Test-Path -LiteralPath $path) -and (Get-ChildItem -LiteralPath $path -Recurse -Force | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint } | Select-Object -First 1)) {
        throw "Refusing to replace a GPUI bundle containing a link at $path"
    }
}
$app = Join-Path $workspace 'apps/gpui-web'
$bindings = Join-Path $workspace '.nyatidraw/toolchains/wasm-bindgen-0.2.127/bin/wasm-bindgen'
if ($IsWindows) { $bindings += '.exe' }
if (-not (Test-Path -LiteralPath $bindings) -or (& $bindings --version) -ne 'wasm-bindgen 0.2.127') {
    throw 'Install the locked wasm-bindgen CLI with tools/setup-worker-ci.ps1.'
}
$compiler = $env:CC_wasm32_unknown_unknown
if (-not $compiler) {
    $clang = Get-Command clang -ErrorAction SilentlyContinue
    if ($clang) { $compiler = $clang.Source }
    elseif ($IsWindows -and (Test-Path "$env:ProgramFiles/LLVM/bin/clang.exe")) {
        $compiler = "$env:ProgramFiles/LLVM/bin/clang.exe"
    }
    else { throw 'Install LLVM Clang for the NTDR codec or set CC_wasm32_unknown_unknown.' }
}
$previousCompiler = $env:CC_wasm32_unknown_unknown
$previousDebug = $env:CARGO_PROFILE_DEV_DEBUG
$previousFlags = $env:CARGO_ENCODED_RUSTFLAGS
Push-Location $workspace
try {
    $env:CC_wasm32_unknown_unknown = $compiler
    $env:CARGO_ENCODED_RUSTFLAGS = & (Join-Path $PSScriptRoot 'web-rustflags.ps1') -RepositoryRoot $workspace
    if ($DebugBuild) { $env:CARGO_PROFILE_DEV_DEBUG = '0' }
    $arguments = @("+$Toolchain", 'rustc', '--locked', '--target', 'wasm32-unknown-unknown',
        '-p', 'nyatidraw-gpui-web', '--target-dir', $cargoOutput)
    if (-not $DebugBuild) { $arguments += '--release' }
    & cargo @arguments -- -C link-arg=-zstack-size=8388608
    if ($LASTEXITCODE -ne 0) { throw 'GPUI web compilation failed.' }
    Assert-BundlePath $staging
    $pkg = Join-Path $staging 'pkg'
    New-Item -ItemType Directory -Force $pkg | Out-Null
    & $bindings (Join-Path $cargoOutput "wasm32-unknown-unknown/$profile/nyatidraw_gpui_web.wasm") --target web --out-name editor --out-dir $pkg --no-typescript --remove-name-section
    if ($LASTEXITCODE -ne 0) { throw 'GPUI web bindings failed.' }
    & (Join-Path $PSScriptRoot 'build-web-worker.ps1') -PublicDirectory $staging -DebugBuild:$DebugBuild -CargoTargetDirectory (Join-Path $workspace 'target/gpui-web-worker-cargo')
    Copy-Item -LiteralPath (Join-Path $app 'index.html'), (Join-Path $app 'start.js'),
        (Join-Path $app 'assets/IBM-Plex-LICENSE.txt'), (Join-Path $workspace 'THIRD-PARTY-ASSETS.txt') -Destination $staging
    $integration = New-Item -ItemType Directory -Force (Join-Path $staging 'integration')
    Get-ChildItem -LiteralPath (Join-Path $workspace 'web-integration') -File |
        Where-Object Extension -in '.js', '.html', '.css' | Copy-Item -Destination $integration
    foreach ($asset in @('index.html', 'start.js', 'pkg/editor_bg.wasm', 'pkg/editor.js', 'worker/manifest.json', 'integration/nyatidraw.js', 'IBM-Plex-LICENSE.txt')) {
        if ((Get-Item -LiteralPath (Join-Path $staging $asset)).Length -eq 0) { throw "Empty GPUI bundle asset $asset" }
    }
    $guard = [IO.File]::Open((Join-Path $outputRoot '.publish.lock'), 'OpenOrCreate', 'Write', 'None')
    try {
        Assert-BundlePath $public
        Assert-BundlePath $previous
        Assert-BundlePath $staging
        $hadPrevious = Test-Path -LiteralPath $public
        if ($hadPrevious) { Move-Item -LiteralPath $public -Destination $previous }
        try { Move-Item -LiteralPath $staging -Destination $public }
        catch {
            if ($hadPrevious) { Move-Item -LiteralPath $previous -Destination $public }
            throw
        }
        if ($hadPrevious) {
            Assert-BundlePath $previous
            Remove-Item -LiteralPath $previous -Force -Recurse
        }
    } finally { $guard.Dispose() }
    Write-Output "GPUI browser editor ready in $public"
}
finally {
    $env:CC_wasm32_unknown_unknown = $previousCompiler
    $env:CARGO_PROFILE_DEV_DEBUG = $previousDebug
    $env:CARGO_ENCODED_RUSTFLAGS = $previousFlags
    Pop-Location
    if (Test-Path -LiteralPath $staging) {
        Assert-BundlePath $staging
        Remove-Item -LiteralPath $staging -Force -Recurse
    }
}
