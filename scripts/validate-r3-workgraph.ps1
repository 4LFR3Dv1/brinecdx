param(
    [switch]$BuildBinaries,
    [string]$CargoTargetDir
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$factoryRoot = Split-Path -Parent $repoRoot
$codexRs = Join-Path $repoRoot 'codex-rs'

if (-not $CargoTargetDir) {
    $CargoTargetDir = Join-Path $factoryRoot '.cargo-target'
}
$CargoTargetDir = [IO.Path]::GetFullPath($CargoTargetDir)

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'cargo is not available on PATH.'
}

function Invoke-CargoStep {
    param(
        [string]$Label,
        [string[]]$Arguments
    )

    Write-Host ""
    Write-Host "==> $Label"
    & cargo @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Label failed with exit code $LASTEXITCODE."
    }
}

function Show-FreeDisk {
    $driveName = [IO.Path]::GetPathRoot($CargoTargetDir).TrimEnd('\').TrimEnd(':')
    $drive = Get-PSDrive $driveName
    Write-Host ("Free disk: {0:N2} GB" -f ($drive.Free / 1GB))
}

$branch = (& git -C $repoRoot branch --show-current).Trim()
$head = (& git -C $repoRoot rev-parse HEAD).Trim()
Write-Host "BrineCDX R3 WorkGraph validation"
Write-Host "Repo:   $repoRoot"
Write-Host "Branch: $branch"
Write-Host "HEAD:   $head"
Write-Host "Target: $CargoTargetDir"
Show-FreeDisk

$env:CARGO_TARGET_DIR = $CargoTargetDir
$env:CARGO_INCREMENTAL = '0'

Push-Location $codexRs
try {
    Invoke-CargoStep 'brine-runtime WorkGraph tests' @(
        'test',
        '-p', 'codex-brine-runtime',
        '-j', '2'
    )

    Invoke-CargoStep 'brine-runtime-extension lifecycle tests' @(
        'test',
        '-p', 'codex-brine-runtime-extension',
        '-j', '2'
    )

    Invoke-CargoStep 'app-server goal-to-Work mapping test' @(
        'test',
        '-p', 'codex-app-server',
        '--lib',
        'brine_work_status_preserves_goal_terminal_semantics',
        '-j', '2'
    )

    Invoke-CargoStep 'app-server R3 compile check' @(
        'check',
        '-p', 'codex-app-server',
        '--bin', 'codex-app-server',
        '-j', '2'
    )

    if ($BuildBinaries) {
        $listeners = Get-NetTCPConnection -LocalPort 4545,4222 -State Listen -ErrorAction SilentlyContinue
        if ($listeners) {
            $owners = $listeners |
                ForEach-Object {
                    $process = Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue
                    [pscustomobject]@{
                        Port = $_.LocalPort
                        PID = $_.OwningProcess
                        Process = if ($process) { $process.ProcessName } else { '<unknown>' }
                    }
                }
            $owners | Format-Table -AutoSize | Out-Host
            throw 'Ports 4545/4222 are still in use. Exit active BrineCDX before rebuilding R3 runtime binaries.'
        }

        Invoke-CargoStep 'R3 runtime-server build' @(
            'build',
            '-p', 'codex-brine-runtime',
            '--bin', 'brine-runtime-server',
            '-j', '2'
        )

        Invoke-CargoStep 'R3 app-server build' @(
            'build',
            '-p', 'codex-app-server',
            '--bin', 'codex-app-server',
            '-j', '2'
        )

        $debugDir = Join-Path $CargoTargetDir 'debug'
        foreach ($name in @('brine-runtime-server.exe', 'codex-app-server.exe')) {
            $path = Join-Path $debugDir $name
            if (-not (Test-Path $path)) {
                throw "Expected binary was not produced: $path"
            }
            Get-Item $path |
                Select-Object Name, Length, LastWriteTime, FullName |
                Format-List |
                Out-Host
        }
    }
}
finally {
    Pop-Location
}

Show-FreeDisk
Write-Host ""
Write-Host 'R3 focused validation PASS.'
if (-not $BuildBinaries) {
    Write-Host 'Run again with -BuildBinaries after exiting active BrineCDX to materialize the R3 runtime/app-server binaries.'
}
