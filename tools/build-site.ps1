[CmdletBinding()]
param(
    [switch] $Offline,
    [string] $WebPublicDirectory,
    [switch] $RequireWeb
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$destination = Join-Path $repositoryRoot 'target/site'
if (-not $WebPublicDirectory) {
    $candidate = Join-Path $repositoryRoot 'target/dx/nyatidraw-web/release/web/public'
    if (Test-Path -LiteralPath $candidate -PathType Container) { $WebPublicDirectory = $candidate }
}
if ($WebPublicDirectory) {
    $WebPublicDirectory = [IO.Path]::GetFullPath($WebPublicDirectory)
    if (-not (Test-Path -LiteralPath (Join-Path $WebPublicDirectory 'index.html') -PathType Leaf)) {
        throw 'The web editor public directory must contain index.html.'
    }
    $wasm = Get-ChildItem -LiteralPath $WebPublicDirectory -Filter '*.wasm' -Recurse -File | Select-Object -First 1
    $javascript = Get-ChildItem -LiteralPath $WebPublicDirectory -Filter '*.js' -Recurse -File | Select-Object -First 1
    if (-not $wasm -or -not $javascript) { throw 'Web editor output is incomplete: wasm and JavaScript are required.' }
    $workerRoot = Join-Path $WebPublicDirectory 'worker'
    $workerManifest = Get-Content -LiteralPath (Join-Path $workerRoot 'manifest.json') -Raw | ConvertFrom-Json
    if ($workerManifest.protocol -ne 1) { throw 'Unsupported recovery Worker protocol.' }
    foreach ($entry in @($workerManifest.entry, $workerManifest.storage)) {
        if ($entry -notmatch '^(recovery|storage)-[a-f0-9]+\.js$' -or
            -not (Test-Path -LiteralPath (Join-Path $workerRoot $entry) -PathType Leaf)) {
            throw 'Recovery Worker output is incomplete.'
        }
    }
    if (-not (Get-ChildItem -LiteralPath $workerRoot -Filter 'ntdr-*.wasm' -File) -or
        -not (Get-ChildItem -LiteralPath $workerRoot -Filter 'ntdr-*.js' -File)) {
        throw 'Recovery Worker WASM bindings are missing.'
    }
    $header = @(Get-Content -LiteralPath $wasm.FullName -AsByteStream -TotalCount 4)
    if (($header -join ',') -ne '0,97,115,109') { throw 'Web editor output does not contain a valid WebAssembly header.' }
}
elseif ($RequireWeb) {
    throw 'Build the web editor before publishing. Run tools/build-web.ps1 first.'
}
else { Write-Warning 'Landing-page preview only: ./draw/ is unavailable. Publish with -RequireWeb.' }
$targetRoot = [IO.Path]::GetFullPath((Join-Path $repositoryRoot 'target'))
$destination = [IO.Path]::GetFullPath($destination)
if ([IO.Path]::GetDirectoryName($destination) -ne $targetRoot -or [IO.Path]::GetFileName($destination) -ne 'site') {
    throw 'Refusing to replace an unexpected site output directory.'
}
$comparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
if ($WebPublicDirectory -and ($WebPublicDirectory.Equals($destination, $comparison) -or $WebPublicDirectory.StartsWith($destination + [IO.Path]::DirectorySeparatorChar, $comparison))) {
    throw 'Web input cannot be inside the generated site output.'
}
if ((Test-Path -LiteralPath $targetRoot) -and ((Get-Item -LiteralPath $targetRoot -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw 'Refusing to replace site output through a linked target directory.'
}
if (Test-Path -LiteralPath $destination) {
    if ((Get-Item -LiteralPath $destination -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw 'Refusing to replace a linked site output directory.'
    }
    if (Get-ChildItem -LiteralPath $destination -Recurse -Force | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint } | Select-Object -First 1) {
        throw 'Refusing to replace a site output directory containing links.'
    }
    Remove-Item -LiteralPath $destination -Recurse -Force
}
New-Item -ItemType Directory -Force $destination | Out-Null
Copy-Item (Join-Path $repositoryRoot 'site/*') $destination -Recurse -Force
if ($WebPublicDirectory) {
    $drawDestination = Join-Path $destination 'draw'
    New-Item -ItemType Directory -Force $drawDestination | Out-Null
    Get-ChildItem -LiteralPath $WebPublicDirectory -Force | Copy-Item -Destination $drawDestination -Recurse -Force
}
New-Item -ItemType File -Path (Join-Path $destination '.nojekyll') -Force | Out-Null
# Never retain a stale download when the published release has been withdrawn.
$releaseFile = Join-Path $destination 'release.json'
'{}' | Set-Content $releaseFile -Encoding utf8NoBOM
if (-not $Offline) {
    $headers = @{ Accept = 'application/vnd.github+json' }
    if ($env:GITHUB_TOKEN) { $headers.Authorization = 'Bearer ' + $env:GITHUB_TOKEN }
    $releases = Invoke-RestMethod 'https://api.github.com/repos/nyabia/nyatidraw/releases?per_page=30' -Headers $headers
    $release = $releases | Where-Object { -not $_.draft -and $_.tag_name -match '^v\d+\.\d+\.\d+-alpha\.\d+$' } | Select-Object -First 1
    if ($release) {
        $asset = $release.assets | Where-Object name -EQ 'NyatiDraw-win-Setup.exe' | Select-Object -First 1
        # Historical published releases keep their existing download until a new
        # NyatiDraw-identity installer is published; never rename old assets.
        if (-not $asset) {
            $asset = $release.assets | Where-Object name -EQ 'NyatiDraw-Alpha-win-Setup.exe' | Select-Object -First 1
        }
        if ($asset) { @{ version=$release.tag_name; url=$asset.browser_download_url } | ConvertTo-Json | Set-Content $releaseFile -Encoding utf8NoBOM }
    }
}
Write-Output "Static website ready: $destination"
