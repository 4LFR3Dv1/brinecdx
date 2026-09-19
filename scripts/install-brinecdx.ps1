param(
    [string]$InstallDir,
    [switch]$AddToUserPath
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$factoryRoot = Split-Path -Parent $repoRoot

if (-not $InstallDir) { $InstallDir = Join-Path $factoryRoot 'bin' }
$InstallDir = [IO.Path]::GetFullPath($InstallDir)
$launcher = Join-Path $repoRoot 'scripts\brinecdx.ps1'

if (-not (Test-Path $launcher)) { throw "Launcher not found: $launcher" }
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

$shim = Join-Path $InstallDir 'brinecdx.cmd'
$shimContent = @(
    '@echo off',
    ('powershell.exe -NoProfile -ExecutionPolicy Bypass -File "{0}" %*' -f $launcher),
    'exit /b %ERRORLEVEL%'
) -join [Environment]::NewLine
Set-Content -Path $shim -Value $shimContent -Encoding Ascii

$normalizedInstall = $InstallDir.TrimEnd('\')
$currentPathEntries = @($env:PATH -split ';' | ForEach-Object { $_.Trim().TrimEnd('\') } | Where-Object { $_ })
$onCurrentPath = $currentPathEntries -contains $normalizedInstall

if ($AddToUserPath) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $userEntries = @($userPath -split ';' | ForEach-Object { $_.Trim().TrimEnd('\') } | Where-Object { $_ })
    if ($userEntries -notcontains $normalizedInstall) {
        $newUserPath = if ([string]::IsNullOrWhiteSpace($userPath)) { $InstallDir } else { $userPath.TrimEnd(';') + ';' + $InstallDir }
        [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
        Write-Host "Added $InstallDir to the user PATH. Open a new terminal before invoking brinecdx by name."
    }
} elseif (-not $onCurrentPath) {
    Write-Warning "$InstallDir is not on this terminal's PATH. Run '$shim' directly, add the directory to PATH, or rerun this installer with -AddToUserPath."
}

Write-Host "Installed BrineCDX shim: $shim"
Write-Host "Daily runtime state: $HOME\.brinecdx\runtime.json"
Write-Host "The R1 witness file is not used or modified by this launcher."
