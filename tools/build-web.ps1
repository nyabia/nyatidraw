[CmdletBinding()]
param(
    [switch] $DebugBuild,
    [string] $BasePath = '/nyatidraw/draw/'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$version = (Get-Content -LiteralPath (Join-Path $PSScriptRoot 'dioxus-cli.version') -Raw).Trim()
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Invalid pinned Dioxus CLI version.' }
if ($BasePath -notmatch '^/(?:[A-Za-z0-9_-]+/)*$') { throw 'BasePath must be an absolute URL directory path ending with a slash.' }
$manifest = Get-Content -LiteralPath (Join-Path $repositoryRoot 'apps/web/Cargo.toml') -Raw
if ($manifest -notmatch "(?m)^dioxus\s*=\s*\{\s*version\s*=\s*`"=$([regex]::Escape($version))`"") {
    throw 'Update the web Dioxus dependency and CLI pin together.'
}
$executableName = if ($IsWindows) { 'dx.exe' } else { 'dx' }
$executable = Join-Path $repositoryRoot ".nyatidraw/toolchains/dioxus-cli-$version/bin/$executableName"
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw 'Run tools/setup-web-ci.ps1 to install the verified Dioxus CLI.'
}
$actual = (& $executable --version) -join ' '
if ($LASTEXITCODE -ne 0 -or $actual -notmatch "^dioxus $([regex]::Escape($version))(?:\s|$)") {
    throw "Expected Dioxus CLI $version, found '$actual'."
}
$profile = if ($DebugBuild) { 'debug' } else { 'release' }
$publicDirectory = [IO.Path]::GetFullPath((Join-Path $repositoryRoot "target/dx/nyatidraw-web/$profile/web/public"))
$comparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
$repositoryPrefix = $repositoryRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
$expectedRelative = "target/dx/nyatidraw-web/$profile/web/public"
if (-not $publicDirectory.StartsWith($repositoryPrefix, $comparison) -or
    [IO.Path]::GetRelativePath($repositoryRoot, $publicDirectory).Replace('\', '/') -ne $expectedRelative) {
    throw 'Refusing to clean an unexpected web output directory.'
}
for ($ancestor = $publicDirectory; ; $ancestor = [IO.Path]::GetDirectoryName($ancestor)) {
    if ((Test-Path -LiteralPath $ancestor) -and ((Get-Item -LiteralPath $ancestor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw 'Refusing to clean web output through a linked directory.'
    }
    if ($ancestor.Equals($repositoryRoot, $comparison)) { break }
}
if (Test-Path -LiteralPath $publicDirectory) {
    if (Get-ChildItem -LiteralPath $publicDirectory -Recurse -Force | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint } | Select-Object -First 1) {
        throw 'Refusing to clean web output containing links.'
    }
    Remove-Item -LiteralPath $publicDirectory -Recurse -Force
}
New-Item -ItemType Directory -Path $publicDirectory -Force | Out-Null

$hadEncodedFlags = Test-Path Env:CARGO_ENCODED_RUSTFLAGS
$previousWasmCompiler = [Environment]::GetEnvironmentVariable('CC_wasm32_unknown_unknown', 'Process')
$wasmCompiler = $previousWasmCompiler
if ([string]::IsNullOrWhiteSpace($wasmCompiler)) {
    $clangCommand = Get-Command clang -ErrorAction SilentlyContinue
    if ($clangCommand) { $wasmCompiler = $clangCommand.Source }
    elseif ($IsWindows -and (Test-Path -LiteralPath "$env:ProgramFiles/LLVM/bin/clang.exe" -PathType Leaf)) {
        $wasmCompiler = "$env:ProgramFiles/LLVM/bin/clang.exe"
    }
    else { throw 'Install LLVM Clang, or set CC_wasm32_unknown_unknown to its executable, for the shared NTDR zstd codec.' }
}
$previousEncodedFlags = [Environment]::GetEnvironmentVariable('CARGO_ENCODED_RUSTFLAGS', 'Process')
$rustFlags = @()
if ($hadEncodedFlags) {
    if (-not [string]::IsNullOrEmpty($previousEncodedFlags)) { $rustFlags = @($previousEncodedFlags.Split([char]0x1f)) }
}
elseif (-not [string]::IsNullOrWhiteSpace($env:RUSTFLAGS)) {
    $rustFlags = @([regex]::Split($env:RUSTFLAGS.Trim(), '\s+'))
}
$taskCargoDirectory = if ($env:CARGO_HOME) { [IO.Path]::GetFullPath($env:CARGO_HOME) } else { Join-Path $HOME '.cargo' }
foreach ($mapping in @(@($repositoryRoot, '/workspace'), @($taskCargoDirectory, '/cargo'))) {
    $source = [IO.Path]::GetFullPath($mapping[0])
    $variants = @($source, $source.Replace('\', '/'))
    if ($IsWindows -and $source -match '^[A-Za-z]:\\') {
        $variants += '\\?\' + $source
        $variants += '//?/' + $source.Replace('\', '/')
    }
    foreach ($variant in ($variants | Select-Object -Unique)) {
        $rustFlags += "--remap-path-prefix=$variant=$($mapping[1])"
    }
}
$rustFlags += '-Cdebuginfo=0'
$arguments = @('build', '--web', '--package', 'nyatidraw-web', '--locked', '--base-path', $BasePath, '--debug-symbols', 'false')
if (-not $DebugBuild) { $arguments += '--release' }
Push-Location $repositoryRoot
try {
    $env:CC_wasm32_unknown_unknown = $wasmCompiler
    $env:CARGO_ENCODED_RUSTFLAGS = $rustFlags -join [char]0x1f
    Write-Host "Building web editor with $actual ($BasePath)"
    & $executable @arguments
    if ($LASTEXITCODE -ne 0) { throw "Dioxus web build failed with exit code $LASTEXITCODE" }
    & (Join-Path $PSScriptRoot 'build-web-worker.ps1') -PublicDirectory $publicDirectory -DebugBuild:$DebugBuild
}
finally {
    if ($null -ne $previousWasmCompiler) { $env:CC_wasm32_unknown_unknown = $previousWasmCompiler }
    else { Remove-Item Env:CC_wasm32_unknown_unknown -ErrorAction SilentlyContinue }
    if ($hadEncodedFlags) { $env:CARGO_ENCODED_RUSTFLAGS = $previousEncodedFlags }
    else { Remove-Item Env:CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue }
    Pop-Location
}
if (-not (Test-Path -LiteralPath (Join-Path $publicDirectory 'index.html') -PathType Leaf)) {
    throw "Web build output not found: $publicDirectory"
}
Write-Output "Web editor ready: $publicDirectory"
