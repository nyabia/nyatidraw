[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'install-files.ps1')

$programsRoot = [System.IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'Programs'))
$installPath = [System.IO.Path]::GetFullPath((Join-Path $programsRoot 'NyatiDraw Development'))
$installedExecutable = Join-Path $installPath 'NyatiDraw.exe'
$projectCommand = '"' + $installedExecutable + '" "%1"'
$projectProgId = 'NyatiDraw.Project.1'
$ownershipMarkerName = 'NyatiDrawDevOwner'
$ownershipMarkerValue = 'NyatiDraw Development installer'
if (-not $installPath.StartsWith($programsRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing an uninstall path outside the current user's Programs folder: $installPath"
}

$runningInstalled = Get-Process -Name 'NyatiDraw' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and ([System.IO.Path]::GetFullPath($_.Path) -eq $installedExecutable) }
if ($runningInstalled) {
    throw 'Close NyatiDraw Development before uninstalling it.'
}

Assert-InstallFilePath $programsRoot $installPath
if (Test-Path -LiteralPath $installPath) {
    # Refuse unknown installations before changing their file associations.
    $null = Read-InstallFileManifest $installPath
    $null = @(Get-InstallFileInventory $installPath)
}

$classes = 'HKCU:\Software\Classes'
function Remove-OwnedRegistryDefault {
    param([string] $Path, [string] $ExpectedValue)
    if (-not $Path.StartsWith('HKCU:\Software\Classes\', [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing a registry path outside user file associations: $Path"
    }
    # Get-Item returns a read-only RegistryKey. Open a short-lived writable
    # handle explicitly and recheck the value before deleting only the default.
    $writable = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($Path.Substring(6), $true)
    if ($null -eq $writable) { return }
    try {
        if ($writable.GetValue('') -ceq $ExpectedValue -and
            $writable.GetValueKind('') -eq [Microsoft.Win32.RegistryValueKind]::String) {
            $writable.DeleteValue('', $false)
        }
    }
    finally { $writable.Dispose() }
}

$extensionKey = Join-Path $classes '.ntdr'
if (Test-Path -LiteralPath $extensionKey) {
    $extensionItem = Get-Item -LiteralPath $extensionKey
    if ($extensionItem.GetValue($ownershipMarkerName) -eq $ownershipMarkerValue -and
        $extensionItem.GetValue('') -eq $projectProgId) {
        Remove-OwnedRegistryDefault $extensionKey $projectProgId
        Remove-ItemProperty -LiteralPath $extensionKey -Name $ownershipMarkerName -Force
        if (-not (Get-Item -LiteralPath $extensionKey).GetValueNames() -and
            -not (Get-Item -LiteralPath $extensionKey).GetSubKeyNames()) {
            Remove-Item -LiteralPath $extensionKey -Force
        }
    }
}
$projectKey = Join-Path $classes $projectProgId
if (Test-Path -LiteralPath $projectKey) {
    $projectItem = Get-Item -LiteralPath $projectKey
    $commandKey = Join-Path $projectKey 'shell\open\command'
    if ($projectItem.GetValue($ownershipMarkerName) -eq $ownershipMarkerValue -and
        (Test-Path -LiteralPath $commandKey) -and
        (Get-Item -LiteralPath $commandKey).GetValue('') -eq $projectCommand) {
        if ($projectItem.GetValue('') -eq 'NyatiDraw Project') {
            Remove-OwnedRegistryDefault $projectKey 'NyatiDraw Project'
        }
        $iconKey = Join-Path $projectKey 'DefaultIcon'
        if (Test-Path -LiteralPath $iconKey) {
            $iconItem = Get-Item -LiteralPath $iconKey
            if ($iconItem.GetValue('') -eq ('"' + $installedExecutable + '",0')) {
                Remove-OwnedRegistryDefault $iconKey ('"' + $installedExecutable + '",0')
                $iconItem = Get-Item -LiteralPath $iconKey
                if ($iconItem.GetValueNames().Count -eq 0 -and $iconItem.GetSubKeyNames().Count -eq 0) { Remove-Item -LiteralPath $iconKey -Force }
            }
        }
        if (Test-Path -LiteralPath $commandKey) {
            $commandItem = Get-Item -LiteralPath $commandKey
            if ($commandItem.GetValue('') -eq $projectCommand) {
                Remove-OwnedRegistryDefault $commandKey $projectCommand
                $commandItem = Get-Item -LiteralPath $commandKey
                if ($commandItem.GetValueNames().Count -eq 0 -and $commandItem.GetSubKeyNames().Count -eq 0) { Remove-Item -LiteralPath $commandKey -Force }
            }
        }
        $openKey = Join-Path $projectKey 'shell\open'
        if (Test-Path -LiteralPath $openKey) {
            $openItem = Get-Item -LiteralPath $openKey
            if ($openItem.GetValueNames().Count -eq 0 -and $openItem.GetSubKeyNames().Count -eq 0) { Remove-Item -LiteralPath $openKey -Force }
        }
        $shellKey = Join-Path $projectKey 'shell'
        if (Test-Path -LiteralPath $shellKey) {
            $shellItem = Get-Item -LiteralPath $shellKey
            if ($shellItem.GetValueNames().Count -eq 0 -and $shellItem.GetSubKeyNames().Count -eq 0) { Remove-Item -LiteralPath $shellKey -Force }
        }
        Remove-ItemProperty -LiteralPath $projectKey -Name $ownershipMarkerName -Force
        if (-not (Get-Item -LiteralPath $projectKey).GetValueNames() -and
            -not (Get-Item -LiteralPath $projectKey).GetSubKeyNames()) {
            Remove-Item -LiteralPath $projectKey -Force
        }
    }
}
$applicationKey = Join-Path $classes 'Applications\NyatiDraw.exe'
if (Test-Path -LiteralPath $applicationKey) {
    $applicationItem = Get-Item -LiteralPath $applicationKey
    $commandKey = Join-Path $applicationKey 'shell\open\command'
    if ($applicationItem.GetValue($ownershipMarkerName) -eq $ownershipMarkerValue -and
        (Test-Path -LiteralPath $commandKey) -and
        (Get-Item -LiteralPath $commandKey).GetValue('') -eq $projectCommand) {
        if ($applicationItem.GetValue('FriendlyAppName') -eq 'NyatiDraw') {
            Remove-ItemProperty -LiteralPath $applicationKey -Name FriendlyAppName -Force
        }
        if (Test-Path -LiteralPath $commandKey) {
            $commandItem = Get-Item -LiteralPath $commandKey
            if ($commandItem.GetValue('') -eq $projectCommand) {
                Remove-OwnedRegistryDefault $commandKey $projectCommand
                $commandItem = Get-Item -LiteralPath $commandKey
                if ($commandItem.GetValueNames().Count -eq 0 -and $commandItem.GetSubKeyNames().Count -eq 0) { Remove-Item -LiteralPath $commandKey -Force }
            }
        }
        $openKey = Join-Path $applicationKey 'shell\open'
        if (Test-Path -LiteralPath $openKey) {
            $openItem = Get-Item -LiteralPath $openKey
            if ($openItem.GetValueNames().Count -eq 0 -and $openItem.GetSubKeyNames().Count -eq 0) { Remove-Item -LiteralPath $openKey -Force }
        }
        $shellKey = Join-Path $applicationKey 'shell'
        if (Test-Path -LiteralPath $shellKey) {
            $shellItem = Get-Item -LiteralPath $shellKey
            if ($shellItem.GetValueNames().Count -eq 0 -and $shellItem.GetSubKeyNames().Count -eq 0) { Remove-Item -LiteralPath $shellKey -Force }
        }
        $supportedTypesKey = Join-Path $applicationKey 'SupportedTypes'
        if (Test-Path -LiteralPath $supportedTypesKey) {
            $supportedTypesItem = Get-Item -LiteralPath $supportedTypesKey
            foreach ($type in @('.png', '.ntdr')) {
                if ($supportedTypesItem.GetValueNames() -contains $type -and
                    $supportedTypesItem.GetValue($type) -eq '') {
                    Remove-ItemProperty -LiteralPath $supportedTypesKey -Name $type -Force
                }
            }
            $supportedTypesItem = Get-Item -LiteralPath $supportedTypesKey
            if ($supportedTypesItem.GetValueNames().Count -eq 0 -and
                $supportedTypesItem.GetSubKeyNames().Count -eq 0) {
                Remove-Item -LiteralPath $supportedTypesKey -Force
            }
        }
        Remove-ItemProperty -LiteralPath $applicationKey -Name $ownershipMarkerName -Force
        if (-not (Get-Item -LiteralPath $applicationKey).GetValueNames() -and
            -not (Get-Item -LiteralPath $applicationKey).GetSubKeyNames()) {
            Remove-Item -LiteralPath $applicationKey -Force
        }
    }
}
$pngOpenWith = Join-Path $classes '.png\OpenWithList\NyatiDraw.exe'
if (Test-Path -LiteralPath $pngOpenWith) {
    $pngItem = Get-Item -LiteralPath $pngOpenWith
    if ($pngItem.GetValue($ownershipMarkerName) -eq $ownershipMarkerValue -and
        $pngItem.GetValueNames().Count -eq 1 -and $pngItem.GetSubKeyNames().Count -eq 0) {
        Remove-Item -LiteralPath $pngOpenWith -Force
    }
}
if (Test-Path -LiteralPath $installPath) {
    Remove-OwnedInstallFiles $installPath
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
[NyatiDraw.ShellNotify]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)

Write-Host 'Removed owned NyatiDraw Development registration and unchanged installed files.'
Write-Host 'User .ntdr projects and PNG files were not touched.'
