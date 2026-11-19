[CmdletBinding()]
param([switch] $DebugBuild)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$workspace = Split-Path $PSScriptRoot -Parent
Push-Location $workspace
try {
    $arguments = @('build', '--locked', '--package', 'nyatidraw-desktop', '--bin', 'nyatidraw-gpui-desktop', '--no-default-features', '--features', 'gpui', '--target-dir', 'target')
    if (-not $DebugBuild) { $arguments += '--release' }
    & cargo @arguments
    if ($LASTEXITCODE -ne 0) { throw 'NyatiDraw Windows build failed.' }
} finally { Pop-Location }
