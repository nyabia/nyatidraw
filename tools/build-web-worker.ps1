[CmdletBinding()]
param([Parameter(Mandatory)][string] $PublicDirectory, [switch] $DebugBuild)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$public = [IO.Path]::GetFullPath($PublicDirectory)
$relative = [IO.Path]::GetRelativePath($repositoryRoot, $public).Replace('\', '/')
if ($relative -notmatch '^target/dx/nyatidraw-web/(release|debug)/web/public$') {
    throw 'Worker output must be inside the generated web build.'
}
$version = '0.2.127'
$executable = Join-Path $repositoryRoot ".nyatidraw/toolchains/wasm-bindgen-$version/bin/wasm-bindgen"
if ($IsWindows) { $executable += '.exe' }
if (-not (Test-Path -LiteralPath $executable)) { throw 'Run tools/setup-worker-ci.ps1 first.' }
if (((& $executable --version) -join ' ').Trim() -ne "wasm-bindgen $version") { throw 'Worker build tool version mismatch.' }
$profile = if ($DebugBuild) { 'debug' } else { 'release' }
$arguments = @('build', '-p', 'nyatidraw-web-worker', '--target', 'wasm32-unknown-unknown', '--locked')
if (-not $DebugBuild) { $arguments += '--release' }
& cargo @arguments
if ($LASTEXITCODE -ne 0) { throw 'Worker WASM build failed.' }
$generated = Join-Path $repositoryRoot "target/web-worker/$profile"
New-Item -ItemType Directory -Force $generated | Out-Null
& $executable (Join-Path $repositoryRoot "target/wasm32-unknown-unknown/$profile/nyatidraw_web_worker.wasm") --target web --out-name ntdr --out-dir $generated --no-typescript
if ($LASTEXITCODE -ne 0) { throw 'Worker bindings failed.' }
$output = Join-Path $public 'worker'
New-Item -ItemType Directory -Force $output | Out-Null
function ContentName([string] $prefix, [byte[]] $bytes, [string] $extension) {
    $hash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant().Substring(0, 20)
    return "$prefix-$hash.$extension"
}
$wasm = [IO.File]::ReadAllBytes((Join-Path $generated 'ntdr_bg.wasm'))
$wasmName = ContentName 'ntdr' $wasm 'wasm'
[IO.File]::WriteAllBytes((Join-Path $output $wasmName), $wasm)
$bindings = [IO.File]::ReadAllText((Join-Path $generated 'ntdr.js')).Replace('ntdr_bg.wasm', $wasmName)
$bindingsName = ContentName 'ntdr' ([Text.Encoding]::UTF8.GetBytes($bindings)) 'js'
[IO.File]::WriteAllText((Join-Path $output $bindingsName), $bindings)
$storage = [IO.File]::ReadAllText((Join-Path $repositoryRoot 'apps/web-worker/storage.js'))
$storageName = ContentName 'storage' ([Text.Encoding]::UTF8.GetBytes($storage)) 'js'
[IO.File]::WriteAllText((Join-Path $output $storageName), $storage)
$entry = [IO.File]::ReadAllText((Join-Path $repositoryRoot 'apps/web-worker/recovery.js')).Replace('./ntdr.js', "./$bindingsName").Replace('./storage.js', "./$storageName")
$entryName = ContentName 'recovery' ([Text.Encoding]::UTF8.GetBytes($entry)) 'js'
[IO.File]::WriteAllText((Join-Path $output $entryName), $entry)
$manifest = @{ protocol = 1; entry = $entryName; storage = $storageName } | ConvertTo-Json -Compress
[IO.File]::WriteAllText((Join-Path $output 'manifest.json'), $manifest)
Write-Output "Recovery Worker ready: $output"
