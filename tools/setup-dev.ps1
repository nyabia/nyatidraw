[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$version = (Get-Content -LiteralPath (Join-Path $PSScriptRoot 'dioxus-cli.version') -Raw).Trim()
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Invalid pinned Dioxus CLI version.' }
$toolRoot = Join-Path $repositoryRoot ".nyatidraw\toolchains\dioxus-cli-$version"
$executable = Join-Path $toolRoot 'bin\dx.exe'
if (Test-Path -LiteralPath $executable -PathType Leaf) {
    $actual = (& $executable --version) -join ' '
    if ($LASTEXITCODE -eq 0 -and $actual -match "^dioxus $([regex]::Escape($version))(?:\s|$)") {
        Write-Host "Dioxus CLI $version is ready: $executable"
        return
    }
    throw "Unexpected tool at $executable ($actual). Resolve it before setup."
}

if (Get-Command cargo-binstall -ErrorAction SilentlyContinue) {
    # Use only the publisher's crate metadata/release assets, in a project-local root.
    & cargo binstall "dioxus-cli@$version" --root $toolRoot --strategies crate-meta-data --disable-telemetry --no-confirm
}
else {
    & cargo install dioxus-cli --version "=$version" --locked --root $toolRoot
}
if ($LASTEXITCODE -ne 0) { throw "Dioxus CLI setup failed with exit code $LASTEXITCODE" }
$actual = (& $executable --version) -join ' '
if ($LASTEXITCODE -ne 0 -or $actual -notmatch "^dioxus $([regex]::Escape($version))(?:\s|$)") {
    throw "Installed Dioxus CLI did not match $version`: $actual"
}
Write-Host "Dioxus CLI $version is ready: $executable"
