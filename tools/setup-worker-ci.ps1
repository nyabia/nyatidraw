[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$version = '0.2.127'
$toolRoot = Join-Path $repositoryRoot ".nyatidraw/toolchains/wasm-bindgen-$version"
$executable = Join-Path $toolRoot $(if ($IsWindows) { 'bin/wasm-bindgen.exe' } else { 'bin/wasm-bindgen' })
if (-not (Test-Path -LiteralPath $executable)) {
    cargo install wasm-bindgen-cli --version $version --locked --root $toolRoot
    if ($LASTEXITCODE -ne 0) { throw 'Worker build tool installation failed.' }
}
$actual = (& $executable --version) -join ' '
if ($LASTEXITCODE -ne 0 -or $actual.Trim() -ne "wasm-bindgen $version") {
    throw "Expected wasm-bindgen $version, found '$actual'."
}
Write-Output $actual
