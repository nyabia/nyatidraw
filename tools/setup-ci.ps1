[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$version = (Get-Content (Join-Path $PSScriptRoot 'dioxus-cli.version') -Raw).Trim()
# Keep the official release archive digest alongside the CLI pin.
if ($version -ne '0.7.9') { throw 'Update the CI archive digest for the new Dioxus CLI.' }
$expected = '0423b94dd36372d09936a9a288c4a6e7903a9f1bddd193b60bba9659890c87c4'
$toolRoot = Join-Path $repositoryRoot ".nyatidraw/toolchains/dioxus-cli-$version"
$archive = Join-Path $repositoryRoot 'target/dioxus-cli-ci.zip'
New-Item -ItemType Directory -Force (Split-Path $archive), (Join-Path $toolRoot 'bin') | Out-Null
Invoke-WebRequest "https://github.com/DioxusLabs/dioxus/releases/download/v$version/dx-x86_64-pc-windows-msvc.zip" -OutFile $archive
if ((Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) {
    throw 'Dioxus CLI archive checksum mismatch.'
}
Expand-Archive -LiteralPath $archive -DestinationPath (Join-Path $toolRoot 'bin') -Force
$executable = Join-Path $toolRoot 'bin/dx.exe'
$actual = (& $executable --version) -join ' '
if ($LASTEXITCODE -ne 0 -or $actual -notmatch '^dioxus 0\.7\.9(?:\s|$)') {
    throw 'Dioxus CLI version verification failed.'
}
Write-Output $actual
