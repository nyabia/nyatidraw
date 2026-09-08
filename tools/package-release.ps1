[CmdletBinding()]
param(
    [Parameter(Mandatory)] [ValidatePattern('^\d+\.\d+\.\d+-alpha\.\d+$')] [string] $Version,
    [string] $Dotnet = 'dotnet'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$source = Join-Path $repositoryRoot 'target/dx/nyatidraw-desktop/release/windows/app'
$toolRoot = Join-Path $repositoryRoot '.nyatidraw/toolchains/vpk-1.2.0'
$archive = Join-Path $repositoryRoot 'target/vpk-1.2.0.zip'
$tool = Join-Path $toolRoot 'tools/net8.0/any/vpk.dll'
if (-not (Test-Path $tool)) {
    New-Item -ItemType Directory -Force $toolRoot | Out-Null
    Invoke-WebRequest 'https://github.com/velopack/velopack/releases/download/1.2.0/vpk.1.2.0.nupkg' -OutFile $archive
    if ((Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne '3e458a676be46d1122e522312db18411f36ea8c70e586f81a676695d43f89dbc') {
        throw 'Velopack archive checksum mismatch.'
    }
    Expand-Archive -LiteralPath $archive -DestinationPath $toolRoot -Force
}
if (-not (Test-Path (Join-Path $source 'nyatidraw-desktop.exe'))) { throw 'Build the release app first.' }
$output = Join-Path $repositoryRoot "target/releases/$Version"
if (Test-Path $output) { throw "Release output already exists; preserve it and choose a new version: $output" }
New-Item -ItemType Directory -Force $output | Out-Null
# A dedicated stage avoids mutating the Dioxus output or packaging developer files.
$stage = Join-Path $output 'stage'
New-Item -ItemType Directory $stage | Out-Null
Copy-Item (Join-Path $source '*') $stage -Recurse
Rename-Item -LiteralPath (Join-Path $stage 'nyatidraw-desktop.exe') -NewName 'nyatidraw.exe'
$notices = [Text.StringBuilder]::new()
[void]$notices.AppendLine('NyatiDraw — third-party notices')
Push-Location $repositoryRoot
try {
    $metadata = (& cargo metadata --format-version 1 --locked --filter-platform x86_64-pc-windows-msvc) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Dependency metadata failed.' }
    foreach ($package in ($metadata.packages | Sort-Object name,version)) {
        if ($package.name -like 'nyatidraw-*') { continue }
        [void]$notices.AppendLine("`n--- $($package.name) $($package.version) [$($package.license)] ---")
        $directory = Split-Path $package.manifest_path
        $licenses = Get-ChildItem -LiteralPath $directory -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)([.-]|$)' }
        foreach ($license in $licenses) {
            [void]$notices.AppendLine($license.Name)
            [void]$notices.AppendLine([IO.File]::ReadAllText($license.FullName))
        }
    }
} finally { Pop-Location }
[IO.File]::WriteAllText((Join-Path $stage 'THIRD-PARTY-NOTICES.txt'), $notices.ToString())
foreach ($name in @('LICENSE-MIT', 'LICENSE-APACHE')) {
    Copy-Item -LiteralPath (Join-Path $repositoryRoot $name) -Destination $stage
}
$versionText = (Get-Item -LiteralPath $tool).VersionInfo.ProductVersion
if ($versionText -notmatch '^1\.2\.0(?:\+|$)') { throw 'Expected Velopack packager 1.2.0.' }
# Keep the new identity off the legacy Alpha feed: its shipped updater does not
# filter package IDs. Alpha is a release maturity label, not an install identity.
& $Dotnet $tool pack --packId NyatiDraw --packVersion $Version --packDir $stage --mainExe nyatidraw.exe --packTitle NyatiDraw --packAuthors nyabia --channel nyatidraw-alpha --runtime win-x64 --framework webview2 --delta None --shortcuts StartMenuRoot --icon (Join-Path $repositoryRoot 'apps/desktop/assets/nyatidraw.ico') --outputDir $output
if ($LASTEXITCODE -ne 0) { throw 'Installer packaging failed.' }
$setup = Get-ChildItem -LiteralPath $output -Filter '*Setup.exe' -File | Select-Object -First 1
if (-not $setup) { throw 'Installer was not produced.' }
# Stable website-facing filename; retain the tool-generated update feeds/packages.
if ($setup.Name -ne 'NyatiDraw-win-Setup.exe') {
    Move-Item -LiteralPath $setup.FullName -Destination (Join-Path $output 'NyatiDraw-win-Setup.exe')
}
$assetManifest = Join-Path $output 'assets.nyatidraw-alpha.json'
$assets = Get-Content -LiteralPath $assetManifest -Raw | ConvertFrom-Json
foreach ($asset in $assets) {
    if ($asset.Type -eq 'Installer') { $asset.RelativeFileName = 'NyatiDraw-win-Setup.exe' }
}
ConvertTo-Json -InputObject @($assets) | Set-Content -LiteralPath $assetManifest -Encoding utf8NoBOM
$sums = Get-ChildItem -LiteralPath $output -File | Sort-Object Name | ForEach-Object { '{0}  {1}' -f (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $_.Name }
$sums | Set-Content (Join-Path $output 'SHA256SUMS.txt') -Encoding utf8NoBOM
Write-Output "Release files ready: $output"
