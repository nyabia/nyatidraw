[CmdletBinding()]
param(
    [Parameter(Mandatory)] [ValidateSet('baseline', 'export')] [string] $Mode,
    [Parameter(Mandatory)] [ValidateRange(1, 99)] [int] $Run,
    [ValidatePattern('^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$')] [string] $Campaign = 'performance-foreground',
    [string] $ExecutablePath,
    [string] $FixturePath
)
$ErrorActionPreference = 'Stop'
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$fixture = Join-Path $repositoryRoot 'target\release\examples\desktop_performance_fixture.exe'
$executable = Join-Path $env:LOCALAPPDATA 'Programs\NyatiDraw Development\NyatiDraw.exe'
if ($ExecutablePath) { $executable = (Resolve-Path -LiteralPath $ExecutablePath).Path }
if ($FixturePath) { $fixture = (Resolve-Path -LiteralPath $FixturePath).Path }
$runRoot = Join-Path $repositoryRoot "target\$Campaign\$Mode-$Run"
if (Test-Path -LiteralPath $runRoot) { throw "Run folder already exists: $runRoot" }
if (-not (Test-Path -LiteralPath $executable) -or -not (Test-Path -LiteralPath $fixture)) {
    throw 'Build desktop_performance_fixture in release and run install-dev.ps1 first.'
}
$project = Join-Path $runRoot 'performance-scratch.ntdr'
& $fixture create $project
if ($LASTEXITCODE -ne 0) { throw 'Fixture creation failed.' }
# Missing layout selects the normal default; use a dedicated path for every run.
$oldLayout = $env:NAYATI_LAYOUT_PATH
$oldTiming = $env:NAYATI_PERFORMANCE
$oldScenario = $env:NAYATI_PERFORMANCE_SCENARIO
try {
    $env:NAYATI_LAYOUT_PATH = Join-Path $runRoot 'workspace.layout'
    $env:NAYATI_PERFORMANCE = '1'
    $env:NAYATI_PERFORMANCE_SCENARIO = $Mode
    $application = Start-Process -FilePath $executable -ArgumentList ('"' + $project + '"') -WindowStyle Hidden -RedirectStandardOutput (Join-Path $runRoot 'out.log') -RedirectStandardError (Join-Path $runRoot 'err.log') -PassThru
    $application.Id | Set-Content (Join-Path $runRoot 'app.pid')
    Write-Output "PID=$($application.Id); activate the NyatiDraw window, then create $([IO.Path]::ChangeExtension($project, 'performance-start')) within two minutes."
} finally {
    $env:NAYATI_LAYOUT_PATH = $oldLayout
    $env:NAYATI_PERFORMANCE = $oldTiming
    $env:NAYATI_PERFORMANCE_SCENARIO = $oldScenario
}
