# BrineCDX launcher tests: local execution is the default.

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$scriptDir = $PSScriptRoot
$launcher = Join-Path $scriptDir 'brinecdx.ps1'
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("brinecdx-test-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tempRoot | Out-Null
$failures = 0

function Run-Case {
    param([string] $Name, [hashtable] $Environment = @{}, [string[]] $Arguments = @())

    foreach ($variable in @(
        'BRINE_EXEC_SERVER_URL','BRINE_EXEC_SERVER_TOKEN','CODEX_EXEC_SERVER_URL',
        'BRINECDX_EXECUTION_MODE','BRINECDX_MODEL','BRINECDX_MODEL_PROVIDER',
        'BRINECDX_REASONING_EFFORT','BRINECDX_MODEL_CATALOG','BRINECDX_DRY_RUN',
        'BRINECDX_CODEX_BIN'
    )) {
        Remove-Item -LiteralPath "Env:$variable" -ErrorAction SilentlyContinue
    }
    foreach ($key in $Environment.Keys) {
        Set-Item -LiteralPath "Env:$key" -Value $Environment[$key]
    }

    $script:outFile = Join-Path $tempRoot "$Name.out"
    $script:errFile = Join-Path $tempRoot "$Name.err"
    $childArgs = @('-NoProfile','-ExecutionPolicy','Bypass','-File',$launcher) + $Arguments
    $p = Start-Process -FilePath 'powershell' -ArgumentList $childArgs -Wait -PassThru -RedirectStandardOutput $outFile -RedirectStandardError $errFile
    $script:status = $p.ExitCode
}

function Text([string] $Path) {
    if (-not (Test-Path $Path)) { return '' }
    $v = Get-Content $Path -Raw
    if ($null -eq $v) { return '' }
    return $v
}
function Check([bool] $Condition, [string] $Label) {
    if ($Condition) { Write-Host "ok   $Label" }
    else { Write-Host "FAIL $Label"; $script:failures++ }
}

try {
    Run-Case 'local' @{ BRINECDX_DRY_RUN = '1' }
    Check ($status -eq 0) 'local mode starts without exec-server URL'
    Check ((Text $errFile).Contains('execution=local')) 'local mode is announced'
    Check ((Text $outFile).Contains("model='gpt-5.6-sol'")) 'Codex argv is resolved'

    Run-Case 'remote-missing' @{
        BRINECDX_EXECUTION_MODE = 'remote'
        BRINECDX_DRY_RUN = '1'
    }
    Check ($status -eq 2) 'remote mode requires exec-server URL'
    Check ((Text $errFile).Contains('remote execution requires BRINE_EXEC_SERVER_URL')) 'remote error is explicit'

    Run-Case 'remote' @{
        BRINECDX_EXECUTION_MODE = 'remote'
        BRINE_EXEC_SERVER_URL = 'ws://127.0.0.1:8766'
        BRINECDX_DRY_RUN = '1'
    }
    Check ($status -eq 0) 'remote mode remains available'
    Check ((Text $errFile).Contains('execution=remote endpoint=ws://127.0.0.1:8766')) 'remote endpoint is announced'

    $stub = Join-Path $tempRoot 'stub.cmd'
    [System.IO.File]::WriteAllLines($stub, @(
        '@echo off',
        'echo brine-url=%BRINE_EXEC_SERVER_URL%',
        'echo codex-url=%CODEX_EXEC_SERVER_URL%',
        'exit /b 7'
    ))

    Run-Case 'local-clears-remote' @{
        BRINE_EXEC_SERVER_URL = 'ws://127.0.0.1:8766'
        CODEX_EXEC_SERVER_URL = 'ws://127.0.0.1:9999'
        BRINECDX_CODEX_BIN = $stub
    }
    $child = Text $outFile
    Check ($status -eq 7) 'child exit code is propagated'
    Check (-not $child.Contains('127.0.0.1:8766')) 'local mode suppresses BRINE_EXEC_SERVER_URL'
    Check (-not $child.Contains('127.0.0.1:9999')) 'local mode suppresses CODEX_EXEC_SERVER_URL'
} finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failures -ne 0) { exit 1 }
Write-Host 'all launcher tests passed'
