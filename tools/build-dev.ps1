[CmdletBinding()]
param([switch] $DebugBuild)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$version = (Get-Content -LiteralPath (Join-Path $PSScriptRoot 'dioxus-cli.version') -Raw).Trim()
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Invalid pinned Dioxus CLI version.' }
$manifest = Get-Content -LiteralPath (Join-Path $repositoryRoot 'Cargo.toml') -Raw
foreach ($dependency in @('dioxus', 'dioxus-desktop')) {
    if ($manifest -notmatch "(?m)^$dependency\s*=\s*\{\s*version\s*=\s*`"=$([regex]::Escape($version))`"") {
        throw "Update the CLI pin and build scripts alongside the $dependency dependency."
    }
}
$executable = Join-Path $repositoryRoot ".nyatidraw\toolchains\dioxus-cli-$version\bin\dx.exe"
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    $systemDx = Get-Command dx -ErrorAction SilentlyContinue
    if (-not $systemDx) { throw 'Run tools/setup-dev.ps1 to install the pinned Dioxus CLI.' }
    $executable = $systemDx.Source
}
$actual = (& $executable --version) -join ' '
if ($LASTEXITCODE -ne 0 -or $actual -notmatch "^dioxus $([regex]::Escape($version))(?:\s|$)") {
    throw "Expected Dioxus CLI $version, found '$actual'. Run tools/setup-dev.ps1."
}
$arguments = @('build', '--windows', '--renderer', 'webview', '--package', 'nyatidraw-desktop', '--locked')
if (-not $DebugBuild) { $arguments += '--release' }
Push-Location $repositoryRoot
try {
    Write-Host "Building with $actual ($executable)"
    & $executable @arguments
    if ($LASTEXITCODE -ne 0) { throw "Dioxus build failed with exit code $LASTEXITCODE" }
}
finally { Pop-Location }
