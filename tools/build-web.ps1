[CmdletBinding()]
param(
    [switch] $DebugBuild,
    [string] $Toolchain = 'nightly-2026-06-09',
    [ValidatePattern('^/(?:[A-Za-z0-9_-]+/)*$')][string] $BasePath = '/nyatidraw/draw/'
)

$ErrorActionPreference = 'Stop'
# The GPUI loader and worker resolve assets relative to the document or module URL.
& (Join-Path $PSScriptRoot 'build-gpui-web.ps1') -DebugBuild:$DebugBuild -Toolchain $Toolchain
