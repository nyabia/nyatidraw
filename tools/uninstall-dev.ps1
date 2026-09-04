[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

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

$classes = 'HKCU:\Software\Classes'
$extensionKey = Join-Path $classes '.ntdr'
if (Test-Path -LiteralPath $extensionKey) {
    $extensionItem = Get-Item -LiteralPath $extensionKey
    if ($extensionItem.GetValue($ownershipMarkerName) -eq $ownershipMarkerValue -and
        $extensionItem.GetValue('') -eq $projectProgId) {
        Remove-ItemProperty -LiteralPath $extensionKey -Name $ownershipMarkerName -Force
        $extensionItem.DeleteValue('', $false)
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
        Remove-ItemProperty -LiteralPath $projectKey -Name $ownershipMarkerName -Force
        if ($projectItem.GetValue('') -eq 'NyatiDraw Project') {
            $projectItem.DeleteValue('', $false)
        }
        $iconKey = Join-Path $projectKey 'DefaultIcon'
        if (Test-Path -LiteralPath $iconKey) {
            $iconItem = Get-Item -LiteralPath $iconKey
            if ($iconItem.GetValue('') -eq ('"' + $installedExecutable + '",0')) {
                $iconItem.DeleteValue('', $false)
                $iconItem = Get-Item -LiteralPath $iconKey
                if ($iconItem.GetValueNames().Count -eq 0 -and $iconItem.GetSubKeyNames().Count -eq 0) { Remove-Item -LiteralPath $iconKey -Force }
            }
        }
        if (Test-Path -LiteralPath $commandKey) {
            $commandItem = Get-Item -LiteralPath $commandKey
            if ($commandItem.GetValue('') -eq $projectCommand) {
                $commandItem.DeleteValue('', $false)
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
        Remove-ItemProperty -LiteralPath $applicationKey -Name $ownershipMarkerName -Force
        if ($applicationItem.GetValue('FriendlyAppName') -eq 'NyatiDraw') {
            Remove-ItemProperty -LiteralPath $applicationKey -Name FriendlyAppName -Force
        }
        if (Test-Path -LiteralPath $commandKey) {
            $commandItem = Get-Item -LiteralPath $commandKey
            if ($commandItem.GetValue('') -eq $projectCommand) {
                $commandItem.DeleteValue('', $false)
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
    Remove-Item -LiteralPath $installPath -Recurse -Force
}

Write-Host 'NyatiDraw Development registration and installed files were removed.'
Write-Host 'User .ntdr projects and PNG files were not touched.'
