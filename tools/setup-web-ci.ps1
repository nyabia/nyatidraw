[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($IsWindows) {
    & (Join-Path $PSScriptRoot 'setup-ci.ps1')
    return
}
if (-not $IsLinux -or [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne 'X64') {
    throw 'The verified web CI toolchain currently supports Windows x64 and Linux x64.'
}
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$version = (Get-Content -LiteralPath (Join-Path $PSScriptRoot 'dioxus-cli.version') -Raw).Trim()
if ($version -ne '0.7.9') { throw 'Update the verified Linux Dioxus CLI digest alongside the CLI pin.' }
$expected = '3b132551b480bc96f938f9f0d37936ee1190f994977539dcc347eaf38540d005'
$toolBin = Join-Path $repositoryRoot ".nyatidraw/toolchains/dioxus-cli-$version/bin"
$archive = Join-Path $repositoryRoot 'target/dioxus-cli-web-ci.tar.gz'
New-Item -ItemType Directory -Force (Split-Path $archive), $toolBin | Out-Null
Invoke-WebRequest "https://github.com/DioxusLabs/dioxus/releases/download/v$version/dx-x86_64-unknown-linux-gnu.tar.gz" -OutFile $archive
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) {
    throw 'Dioxus CLI archive checksum mismatch.'
}
& tar -xzf $archive -C $toolBin
if ($LASTEXITCODE -ne 0) { throw "Dioxus CLI extraction failed with exit code $LASTEXITCODE" }
$executable = Join-Path $toolBin 'dx'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) { throw 'Verified CLI archive did not contain dx.' }
& chmod +x $executable
if ($LASTEXITCODE -ne 0) { throw 'Dioxus CLI executable permissions could not be set.' }
$actual = (& $executable --version) -join ' '
if ($LASTEXITCODE -ne 0 -or $actual -notmatch '^dioxus 0\.7\.9(?:\s|$)') {
    throw 'Dioxus CLI version verification failed.'
}
Write-Output $actual
