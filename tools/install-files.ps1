# File ownership for the development installer. Never recursively delete an
# installation: projects or other files may have been placed beside the app.
$installManifestName = '.nyatidraw-install.json'
$installManifestOwner = 'NyatiDraw Development installer'

function Assert-InstallFilePath {
    param([string] $Root, [string] $Path)
    $rootPath = [IO.Path]::GetFullPath($Root).TrimEnd('\')
    $fullPath = [IO.Path]::GetFullPath($Path)
    if ($fullPath -ine $rootPath -and
        -not $fullPath.StartsWith($rootPath + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw "Installation path escapes its root: $fullPath"
    }
    # Check ancestors too: a junction at Programs or the installation root must
    # not redirect a validated lexical path into another directory.
    $ancestor = $fullPath
    while ($ancestor) {
        $item = Get-Item -LiteralPath $ancestor -Force -ErrorAction SilentlyContinue
        if ($item) {
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw "Refusing an installation path containing a reparse point: $ancestor"
            }
        }
        $ancestor = [IO.Path]::GetDirectoryName($ancestor)
    }
}

function Get-InstallFileInventory {
    param([string] $Root)
    Assert-InstallFilePath $Root $Root
    $pending = [Collections.Generic.Stack[string]]::new()
    $pending.Push($Root)
    while ($pending.Count -gt 0) {
        foreach ($entry in Get-ChildItem -LiteralPath $pending.Pop() -Force) {
            Assert-InstallFilePath $Root $entry.FullName
            if ($entry.PSIsContainer) { $pending.Push($entry.FullName) }
            else {
                [pscustomobject]@{
                    Path = $entry.FullName.Substring($Root.TrimEnd('\').Length + 1).Replace('\', '/')
                    Sha256 = (Get-FileHash -LiteralPath $entry.FullName -Algorithm SHA256).Hash
                }
            }
        }
    }
}

function Read-InstallFileManifest {
    param([string] $Root)
    $manifestPath = Join-Path $Root $installManifestName
    Assert-InstallFilePath $Root $manifestPath
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
        throw "Installation has no ownership manifest; preserving it: $Root"
    }
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    if ($manifest.Schema -ne 1 -or $manifest.Owner -cne $installManifestOwner -or @($manifest.Files).Count -eq 0) {
        throw "Invalid installation ownership manifest: $manifestPath"
    }
    $paths = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($file in $manifest.Files) {
        if ($file.Path -isnot [string] -or $file.Sha256 -notmatch '^[0-9a-fA-F]{64}$' -or
            $file.Path -match '[\\:\x00-\x1f<>"|?*]' -or
            $file.Path -match '(^|/)(\.{1,2}|)(/|$)' -or
            $file.Path -match '[. ](/|$)' -or
            $file.Path -ieq $installManifestName -or -not $paths.Add($file.Path)) {
            throw "Invalid or duplicate owned installation path in $manifestPath"
        }
        Assert-InstallFilePath $Root (Join-Path $Root $file.Path)
    }
    if (-not $paths.Contains('NyatiDraw.exe')) { throw "Manifest does not own NyatiDraw.exe: $manifestPath" }
    return $manifest
}

function Assert-UnchangedInstallation {
    param([string] $Root)
    $manifest = Read-InstallFileManifest $Root
    $expected = @{}
    foreach ($file in $manifest.Files) { $expected[$file.Path] = $file.Sha256 }
    foreach ($file in Get-InstallFileInventory $Root) {
        if ($file.Path -ieq $installManifestName) { continue }
        if (-not $expected.ContainsKey($file.Path) -or $expected[$file.Path] -ine $file.Sha256) {
            throw "Installation contains an added or modified file; move it outside the install folder before updating: $($file.Path)"
        }
        $expected.Remove($file.Path)
    }
    if ($expected.Count -ne 0) { throw "Installation files are missing; refusing an automatic update: $Root" }
}

function Remove-OwnedInstallFiles {
    param([string] $Root)
    $manifest = Read-InstallFileManifest $Root
    # Finish the entire path/reparse scan before removing the first file.
    $inventory = @(Get-InstallFileInventory $Root)
    $retainedOwned = $false
    foreach ($file in $manifest.Files) {
        $path = Join-Path $Root $file.Path
        Assert-InstallFilePath $Root $path
        if (Test-Path -LiteralPath $path) {
            if ((Test-Path -LiteralPath $path -PathType Leaf) -and
                (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ieq $file.Sha256) {
                Remove-Item -LiteralPath $path -Force
            }
            else {
                $retainedOwned = $true
                Write-Warning "Preserved modified installation file: $path"
            }
        }
    }
    if (-not $retainedOwned) { Remove-Item -LiteralPath (Join-Path $Root $installManifestName) -Force }
    # Empty directory removal is non-recursive and cannot delete a late-arriving file.
    $directories = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    [void] $directories.Add($Root)
    foreach ($file in $inventory) {
        $directory = [IO.Path]::GetDirectoryName((Join-Path $Root $file.Path))
        while ($directory -and $directory.Length -ge $Root.Length) {
            [void] $directories.Add($directory)
            if ($directory -ieq $Root) { break }
            $directory = [IO.Path]::GetDirectoryName($directory)
        }
    }
    foreach ($directory in ($directories | Sort-Object Length -Descending)) {
        Assert-InstallFilePath $Root $directory
        if ((Test-Path -LiteralPath $directory -PathType Container) -and
            @(Get-ChildItem -LiteralPath $directory -Force).Count -eq 0) {
            [IO.Directory]::Delete($directory, $false)
        }
    }
    if (Test-Path -LiteralPath $Root) { Write-Warning "Preserved remaining installation contents: $Root" }
}
