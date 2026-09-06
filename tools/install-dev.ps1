[CmdletBinding()]
param(
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'install-files.ps1')

$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$programsRoot = [System.IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'Programs'))
$installPath = [System.IO.Path]::GetFullPath((Join-Path $programsRoot 'NyatiDraw Development'))
$sourcePath = Join-Path $repositoryRoot 'target\dx\nyatidraw-desktop\release\windows\app'
$sourceExecutable = Join-Path $sourcePath 'nyatidraw-desktop.exe'
$installedExecutable = Join-Path $installPath 'NyatiDraw.exe'
$classes = 'HKCU:\Software\Classes'
$projectProgId = 'NyatiDraw.Project.1'
$ownershipMarkerName = 'NyatiDrawDevOwner'
$ownershipMarkerValue = 'NyatiDraw Development installer'
$projectCommand = '"' + $installedExecutable + '" "%1"'
$projectKey = Join-Path $classes $projectProgId
$projectCommandKey = Join-Path $projectKey 'shell\open\command'
$applicationKey = Join-Path $classes 'Applications\NyatiDraw.exe'
$applicationCommandKey = Join-Path $applicationKey 'shell\open\command'
$extensionKey = Join-Path $classes '.ntdr'

if (-not $installPath.StartsWith($programsRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing an install path outside the current user's Programs folder: $installPath"
}

$pngOpenWith = Join-Path $classes '.png\OpenWithList\NyatiDraw.exe'
$pngProgIds = Join-Path $classes '.png\OpenWithProgIds'
$projectProgIds = Join-Path $extensionKey 'OpenWithProgIds'

function Test-OpenWithProgId {
    param([string] $Path)
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    $item = Get-Item -LiteralPath $Path
    return ($item.GetValueNames() -ccontains $projectProgId -and
        $item.GetValue($projectProgId) -ceq '' -and
        $item.GetValueKind($projectProgId) -eq [Microsoft.Win32.RegistryValueKind]::String)
}

function Test-RegistryKeyShape {
    param(
        [Parameter(Mandatory)] [string] $Path,
        [AllowNull()] [object] $ExpectedDefaultValue,
        [Parameter(Mandatory)] [hashtable] $ExpectedValues,
        [Parameter(Mandatory)] [AllowEmptyCollection()] [string[]] $ExpectedSubkeys
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return $false
    }

    $item = Get-Item -LiteralPath $Path
    $allValueNames = @($item.GetValueNames())
    $actualValueNames = @($allValueNames | Where-Object { $_ -ne '' })
    if ($actualValueNames.Count -ne $ExpectedValues.Count) {
        return $false
    }
    foreach ($name in $ExpectedValues.Keys) {
        if ($actualValueNames -cnotcontains [string] $name -or
            $item.GetValue([string] $name) -cne $ExpectedValues[$name] -or
            $item.GetValueKind([string] $name) -ne [Microsoft.Win32.RegistryValueKind]::String) {
            return $false
        }
    }

    if ($null -eq $ExpectedDefaultValue) {
        if ($allValueNames -contains '') {
            return $false
        }
    }
    else {
        try {
            if ($allValueNames -notcontains '' -or
                $item.GetValue('') -cne $ExpectedDefaultValue -or
                $item.GetValueKind('') -ne [Microsoft.Win32.RegistryValueKind]::String) {
                return $false
            }
        }
        catch [System.ArgumentException] {
            return $false
        }
    }

    $actualSubkeys = @($item.GetSubKeyNames())
    if ($actualSubkeys.Count -ne $ExpectedSubkeys.Count) {
        return $false
    }
    foreach ($subkey in $ExpectedSubkeys) {
        if ($actualSubkeys -cnotcontains $subkey) {
            return $false
        }
    }
    return $true
}

function Test-NyatiDrawRegistration {
    $rootMarkerValues = @{ $ownershipMarkerName = $ownershipMarkerValue }
    $applicationValues = @{
        FriendlyAppName = 'NyatiDraw'
        $ownershipMarkerName = $ownershipMarkerValue
    }

    # Accept the exact previous installer shape for an additive upgrade. The PNG
    # ProgID list is shared with other applications; inspect only our named value.
    $hasPngProgId = (Test-Path -LiteralPath $pngProgIds) -and
        ((Get-Item -LiteralPath $pngProgIds).GetValueNames() -contains $projectProgId)
    $hasProjectProgIds = Test-Path -LiteralPath $projectProgIds
    if ($hasPngProgId -ne $hasProjectProgIds) { return $false }
    if ($hasPngProgId -and -not (Test-OpenWithProgId $pngProgIds)) { return $false }
    $extensionSubkeys = @()
    if ($hasProjectProgIds) { $extensionSubkeys = @('OpenWithProgIds') }
    $shapes = @(
        @{ Path = $extensionKey; Default = $projectProgId; Values = $rootMarkerValues; Subkeys = $extensionSubkeys }
        @{ Path = $projectKey; Default = 'NyatiDraw Project'; Values = $rootMarkerValues; Subkeys = @('DefaultIcon', 'shell') }
        @{ Path = (Join-Path $projectKey 'DefaultIcon'); Default = ('"' + $installedExecutable + '",0'); Values = @{}; Subkeys = @() }
        @{ Path = (Join-Path $projectKey 'shell'); Default = $null; Values = @{}; Subkeys = @('open') }
        @{ Path = (Join-Path $projectKey 'shell\open'); Default = $null; Values = @{}; Subkeys = @('command') }
        @{ Path = $projectCommandKey; Default = $projectCommand; Values = @{}; Subkeys = @() }
        @{ Path = $applicationKey; Default = $null; Values = $applicationValues; Subkeys = @('shell', 'SupportedTypes') }
        @{ Path = (Join-Path $applicationKey 'shell'); Default = $null; Values = @{}; Subkeys = @('open') }
        @{ Path = (Join-Path $applicationKey 'shell\open'); Default = $null; Values = @{}; Subkeys = @('command') }
        @{ Path = $applicationCommandKey; Default = $projectCommand; Values = @{}; Subkeys = @() }
        @{ Path = (Join-Path $applicationKey 'SupportedTypes'); Default = $null; Values = @{ '.png' = ''; '.ntdr' = '' }; Subkeys = @() }
        @{ Path = $pngOpenWith; Default = $null; Values = $rootMarkerValues; Subkeys = @() }
    )
    if ($hasProjectProgIds) {
        $shapes += @{ Path = $projectProgIds; Default = $null; Values = @{ $projectProgId = '' }; Subkeys = @() }
    }

    foreach ($shape in $shapes) {
        if (-not (Test-RegistryKeyShape -Path $shape.Path -ExpectedDefaultValue $shape.Default -ExpectedValues $shape.Values -ExpectedSubkeys $shape.Subkeys)) {
            return $false
        }
    }
    return $true
}

# Refuse ownership collisions before building or replacing any installed files.
# Unowned or partially-owned trees remain fail-closed. Exact shape checks also
# reject foreign values/subkeys on an otherwise familiar registration.
$registrationRoots = @($extensionKey, $projectKey, $applicationKey, $pngOpenWith)
$registrationExists = $false
foreach ($path in $registrationRoots) {
    if (Test-Path -LiteralPath $path) {
        $registrationExists = $true
        break
    }
}
if ($registrationExists -and
    -not (Test-NyatiDrawRegistration)) {
    throw 'NyatiDraw registration is not the exact owned development registration; refusing to overwrite it.'
}
if ($registrationExists) {
    Write-Host 'Existing NyatiDraw development registration matched the exact owned shape; continuing.'
}
if (-not $registrationExists -and (Test-Path -LiteralPath $pngProgIds) -and
    ((Get-Item -LiteralPath $pngProgIds).GetValueNames() -contains $projectProgId)) {
    throw 'An unowned NyatiDraw PNG ProgID value exists; refusing to overwrite it.'
}

Assert-InstallFilePath $programsRoot $installPath
if (Test-Path -LiteralPath $installPath) { Assert-UnchangedInstallation $installPath }

if (-not $SkipBuild) {
    Push-Location $repositoryRoot
    try {
        & (Join-Path $PSScriptRoot 'build-dev.ps1')
    }
    finally {
        Pop-Location
    }
}

if (-not (Test-Path -LiteralPath $sourceExecutable -PathType Leaf)) {
    throw "Dioxus release output is missing: $sourceExecutable"
}

$runningInstalled = Get-Process -Name 'NyatiDraw' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and ([System.IO.Path]::GetFullPath($_.Path) -eq $installedExecutable) }
if ($runningInstalled) {
    throw 'Close the installed NyatiDraw Development process before updating it.'
}

New-Item -ItemType Directory -Path $programsRoot -Force | Out-Null
$nonce = [Guid]::NewGuid().ToString('N')
$stagingPath = "$installPath.~staging-$nonce"
$backupPath = "$installPath.~backup-$nonce"
Assert-InstallFilePath $programsRoot $stagingPath
Assert-InstallFilePath $programsRoot $backupPath
# Reject source junctions before Copy-Item traverses the bundle.
$null = @(Get-InstallFileInventory $sourcePath)
if (Test-Path -LiteralPath (Join-Path $sourcePath $installManifestName)) {
    throw 'Release bundle contains the reserved installation ownership manifest.'
}
New-Item -ItemType Directory -Path $stagingPath | Out-Null
Copy-Item -Path (Join-Path $sourcePath '*') -Destination $stagingPath -Recurse -Force
Rename-Item -LiteralPath (Join-Path $stagingPath 'nyatidraw-desktop.exe') -NewName 'NyatiDraw.exe'
$ownedFiles = @(Get-InstallFileInventory $stagingPath)
@{ Schema = 1; Owner = $installManifestOwner; Files = $ownedFiles } |
    ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $stagingPath $installManifestName) -Encoding UTF8

try {
    Assert-InstallFilePath $programsRoot $installPath
    Assert-InstallFilePath $programsRoot $backupPath
    if (Test-Path -LiteralPath $installPath) {
        Assert-UnchangedInstallation $installPath
        [IO.Directory]::Move($installPath, $backupPath)
    }
    Assert-InstallFilePath $programsRoot $stagingPath
    # Directory.Move refuses an existing destination instead of nesting the
    # staging directory inside it if another installer wins the race.
    [IO.Directory]::Move($stagingPath, $installPath)
    if (Test-Path -LiteralPath $backupPath) {
        Remove-OwnedInstallFiles $backupPath
    }
}
catch {
    if (-not (Test-Path -LiteralPath $installPath) -and (Test-Path -LiteralPath $backupPath)) {
        Assert-InstallFilePath $programsRoot $backupPath
        Assert-InstallFilePath $programsRoot $installPath
        [IO.Directory]::Move($backupPath, $installPath)
    }
    throw
}

New-Item -Path $projectKey -Force | Out-Null
Set-Item -Path $projectKey -Value 'NyatiDraw Project'
Set-ItemProperty -Path $projectKey -Name $ownershipMarkerName -Value $ownershipMarkerValue
New-Item -Path (Join-Path $projectKey 'DefaultIcon') -Force | Out-Null
Set-Item -Path (Join-Path $projectKey 'DefaultIcon') -Value ('"' + $installedExecutable + '",0')
New-Item -Path (Join-Path $projectKey 'shell\open\command') -Force | Out-Null
Set-Item -Path (Join-Path $projectKey 'shell\open\command') -Value $projectCommand

New-Item -Path $extensionKey -Force | Out-Null
Set-Item -Path $extensionKey -Value $projectProgId
Set-ItemProperty -Path $extensionKey -Name $ownershipMarkerName -Value $ownershipMarkerValue

New-Item -Path $applicationKey -Force | Out-Null
Set-ItemProperty -Path $applicationKey -Name 'FriendlyAppName' -Value 'NyatiDraw'
Set-ItemProperty -Path $applicationKey -Name $ownershipMarkerName -Value $ownershipMarkerValue
New-Item -Path (Join-Path $applicationKey 'shell\open\command') -Force | Out-Null
Set-Item -Path (Join-Path $applicationKey 'shell\open\command') -Value $projectCommand
New-Item -Path (Join-Path $applicationKey 'SupportedTypes') -Force | Out-Null
Set-ItemProperty -Path (Join-Path $applicationKey 'SupportedTypes') -Name '.png' -Value ''
Set-ItemProperty -Path (Join-Path $applicationKey 'SupportedTypes') -Name '.ntdr' -Value ''

New-Item -Path $pngOpenWith -Force | Out-Null
Set-ItemProperty -Path $pngOpenWith -Name $ownershipMarkerName -Value $ownershipMarkerValue

# OpenWithList is retained only as an owned legacy registration. Current Shell
# discovery uses OpenWithProgIds. Never replace the shared PNG key or UserChoice.
foreach ($path in @($pngProgIds, $projectProgIds)) {
    if (-not (Test-Path -LiteralPath $path)) { New-Item -Path $path -Force | Out-Null }
    New-ItemProperty -LiteralPath $path -Name $projectProgId -PropertyType String -Value '' -Force | Out-Null
}

if (-not ('NyatiDraw.ShellNotify' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace NyatiDraw {
    public static class ShellNotify {
        [DllImport("shell32.dll")]
        public static extern void SHChangeNotify(uint eventId, uint flags, IntPtr item1, IntPtr item2);
    }
}
'@
}
[NyatiDraw.ShellNotify]::SHChangeNotify(0x08000000, 0x1000, [IntPtr]::Zero, [IntPtr]::Zero)

Write-Host "NyatiDraw development build installed: $installedExecutable"
Write-Host '.ntdr is registered for direct open; PNG default-app settings were not changed.'
