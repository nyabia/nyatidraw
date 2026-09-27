[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$docsRoot = Join-Path $repositoryRoot 'site-docs'
$npm = if ($IsWindows) { 'npm.cmd' } else { 'npm' }
Push-Location $docsRoot
try {
    & $npm run build
    if ($LASTEXITCODE -ne 0) { throw 'Documentation build failed. Run npm ci in site-docs first.' }
    node (Join-Path $repositoryRoot 'web-integration/userscript/build.mjs')
    if ($LASTEXITCODE -ne 0) { throw 'Userscript bundle build failed.' }
}
finally { Pop-Location }
