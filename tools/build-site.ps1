[CmdletBinding()]
param([switch] $Offline)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$destination = Join-Path $repositoryRoot 'target/site'
New-Item -ItemType Directory -Force $destination | Out-Null
Copy-Item (Join-Path $repositoryRoot 'site/*') $destination -Recurse -Force
# Never retain a stale download when the published release has been withdrawn.
$releaseFile = Join-Path $destination 'release.json'
'{}' | Set-Content $releaseFile -Encoding utf8NoBOM
if (-not $Offline) {
    $headers = @{ Accept = 'application/vnd.github+json' }
    if ($env:GITHUB_TOKEN) { $headers.Authorization = 'Bearer ' + $env:GITHUB_TOKEN }
    $releases = Invoke-RestMethod 'https://api.github.com/repos/nyabia/nyatidraw/releases?per_page=30' -Headers $headers
    $release = $releases | Where-Object { -not $_.draft -and $_.tag_name -match '^v\d+\.\d+\.\d+-alpha\.\d+$' } | Select-Object -First 1
    if ($release) {
        $asset = $release.assets | Where-Object name -EQ 'NyatiDraw-Alpha-win-Setup.exe' | Select-Object -First 1
        if ($asset) { @{ version=$release.tag_name; url=$asset.browser_download_url } | ConvertTo-Json | Set-Content $releaseFile -Encoding utf8NoBOM }
    }
}
Write-Output "Static website ready: $destination"
