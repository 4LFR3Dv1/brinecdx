$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$factoryRoot = Split-Path -Parent $repoRoot
$runtimeAddress = '127.0.0.1:4545'
$appServerUrl = 'ws://127.0.0.1:4222'
$runtimePort = 4545
$appServerPort = 4222

$brineHome = if ($env:BRINECDX_HOME) { $env:BRINECDX_HOME } else { Join-Path $HOME '.brinecdx' }
$statePath = Join-Path $brineHome 'runtime.json'
$logDir = Join-Path $brineHome 'logs'
New-Item -ItemType Directory -Force -Path $brineHome, $logDir | Out-Null

function Resolve-DebugDirectory {
    $candidates = @()
    if ($env:CARGO_TARGET_DIR) { $candidates += (Join-Path $env:CARGO_TARGET_DIR 'debug') }
    $candidates += (Join-Path $factoryRoot '.cargo-target\debug')
    $candidates += (Join-Path $repoRoot 'codex-rs\target\debug')

    foreach ($candidate in ($candidates | Select-Object -Unique)) {
        $codex = Join-Path $candidate 'codex.exe'
        $appServer = Join-Path $candidate 'codex-app-server.exe'
        $runtime = Join-Path $candidate 'brine-runtime-server.exe'
        if ((Test-Path $codex) -and (Test-Path $appServer) -and (Test-Path $runtime)) { return $candidate }
    }

    throw "Could not find codex.exe, codex-app-server.exe and brine-runtime-server.exe in a common debug target. Build the three BrineCDX binaries first."
}

function Get-Listener {
    param([int]$Port)
    Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
}

function Wait-Listener {
    param([int]$Port, [int]$TimeoutSeconds = 8)
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (Get-Listener -Port $Port) { return }
        Start-Sleep -Milliseconds 150
    }
    throw "Timed out waiting for local listener on port $Port."
}

function Assert-ExpectedListener {
    param($Listener, [string]$ExpectedProcessName, [string]$ExpectedExecutable)
    if (-not $Listener) { return $false }
    $process = Get-Process -Id $Listener.OwningProcess -ErrorAction Stop
    if ($process.ProcessName -ne $ExpectedProcessName) {
        throw "Port $($Listener.LocalPort) is owned by PID $($Listener.OwningProcess) ($($process.ProcessName)), not $ExpectedProcessName."
    }
    if ($process.Path) {
        $actual = [IO.Path]::GetFullPath($process.Path)
        $expected = [IO.Path]::GetFullPath($ExpectedExecutable)
        if ($actual -ne $expected) { throw "Port $($Listener.LocalPort) is owned by $actual, expected $expected." }
    }
    return $true
}

$debugDir = Resolve-DebugDirectory
$codexExe = Join-Path $debugDir 'codex.exe'
$appServerExe = Join-Path $debugDir 'codex-app-server.exe'
$runtimeExe = Join-Path $debugDir 'brine-runtime-server.exe'

$runtimeListener = Get-Listener -Port $runtimePort
if (Assert-ExpectedListener -Listener $runtimeListener -ExpectedProcessName 'brine-runtime-server' -ExpectedExecutable $runtimeExe) {
    $runtimeProcess = Get-CimInstance Win32_Process -Filter "ProcessId=$($runtimeListener.OwningProcess)" -ErrorAction SilentlyContinue
    if ($runtimeProcess -and $runtimeProcess.CommandLine -and $runtimeProcess.CommandLine -notlike "*$statePath*") {
        throw "Port $runtimePort is already served by brine-runtime-server PID $($runtimeListener.OwningProcess) using a different state file. Daily state is $statePath. Stop that runtime first; refusing to attach daily development to another state (for example the R1 witness)."
    }
} else {
    Start-Process -FilePath $runtimeExe -ArgumentList @('"' + $statePath + '"', $runtimeAddress) -WorkingDirectory $repoRoot -WindowStyle Hidden -RedirectStandardOutput (Join-Path $logDir 'runtime.stdout.log') -RedirectStandardError (Join-Path $logDir 'runtime.stderr.log') | Out-Null
    Wait-Listener -Port $runtimePort
}

$appServerListener = Get-Listener -Port $appServerPort
if (-not (Assert-ExpectedListener -Listener $appServerListener -ExpectedProcessName 'codex-app-server' -ExpectedExecutable $appServerExe)) {
    $previousRuntimeAddress = $env:BRINECDX_RUNTIME_AUTHORITY_ADDR
    $env:BRINECDX_RUNTIME_AUTHORITY_ADDR = $runtimeAddress
    try {
        Start-Process -FilePath $appServerExe -ArgumentList @('--listen', $appServerUrl) -WorkingDirectory $repoRoot -WindowStyle Hidden -RedirectStandardOutput (Join-Path $logDir 'app-server.stdout.log') -RedirectStandardError (Join-Path $logDir 'app-server.stderr.log') | Out-Null
    } finally {
        if ($null -eq $previousRuntimeAddress) { Remove-Item Env:BRINECDX_RUNTIME_AUTHORITY_ADDR -ErrorAction SilentlyContinue } else { $env:BRINECDX_RUNTIME_AUTHORITY_ADDR = $previousRuntimeAddress }
    }
    Wait-Listener -Port $appServerPort
}

& $codexExe --remote $appServerUrl @args
exit $LASTEXITCODE
