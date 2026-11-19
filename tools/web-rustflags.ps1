[CmdletBinding()]
param([Parameter(Mandatory)][string] $RepositoryRoot)
$hadEncodedFlags = Test-Path Env:CARGO_ENCODED_RUSTFLAGS
$previousEncodedFlags = $env:CARGO_ENCODED_RUSTFLAGS
$rustFlags = @()
if ($hadEncodedFlags) {
    if (-not [string]::IsNullOrEmpty($previousEncodedFlags)) { $rustFlags = @($previousEncodedFlags.Split([char]0x1f)) }
}
elseif (-not [string]::IsNullOrWhiteSpace($env:RUSTFLAGS)) {
    $rustFlags = @([regex]::Split($env:RUSTFLAGS.Trim(), '\s+'))
}
$taskCargoDirectory = if ($env:CARGO_HOME) { [IO.Path]::GetFullPath($env:CARGO_HOME) } else { Join-Path $HOME '.cargo' }
$toolchainDirectory = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $HOME '.rustup' }
foreach ($mapping in @(@($HOME, '/user'), @($repositoryRoot, '/workspace'), @($taskCargoDirectory, '/cargo'), @($toolchainDirectory, '/rustup'))) {
    $source = [IO.Path]::GetFullPath($mapping[0])
    $variants = @($source, $source.Replace('\', '/'))
    if ($IsWindows -and $source -match '^[A-Za-z]:\\') {
        $variants += '\\?\' + $source
        $variants += '//?/' + $source.Replace('\', '/')
    }
    foreach ($variant in ($variants | Select-Object -Unique)) {
        $rustFlags += "--remap-path-prefix=$variant=$($mapping[1])"
    }
}
$rustFlags += '-Cdebuginfo=0'

Write-Output ($rustFlags -join [char]0x1f)
