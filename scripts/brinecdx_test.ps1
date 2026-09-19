# Launcher tests for BCDX-WP-02.
#
# Windows counterpart of scripts/brinecdx_test.sh. Verifies the BrineCDX
# launcher selects the ChatGPT-backed provider, escapes machine-wide DeepSeek
# settings, forwards user arguments and exit codes, and fails closed when the
# Brine execution environment is missing.
#
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File scripts/brinecdx_test.ps1

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$scriptDir = $PSScriptRoot
$launcher = Join-Path $scriptDir 'brinecdx.ps1'
$repoRoot = Split-Path -Parent $scriptDir
$catalog = Join-Path $repoRoot 'codex-rs\models-manager\models.json'

$tempBase = [System.IO.Path]::GetTempPath()
$tempRoot = Join-Path $tempBase ("brinecdx-test-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tempRoot | Out-Null

$failures = 0

function Report-Pass {
    param([string] $Label)

    Write-Host "ok   $Label"
}

function Report-Fail {
    param([string] $Label)

    Write-Host "FAIL $Label"
    $script:failures = $script:failures + 1
}

function Get-Text {
    param([string] $Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return ''
    }

    $text = Get-Content -LiteralPath $Path -Raw
    if ($null -eq $text) {
        # Get-Content -Raw returns $null for an empty file.
        return ''
    }

    return $text
}

function Assert-Status {
    param([string] $Label, [int] $Expected)

    if ($script:launcherStatus -eq $Expected) {
        Report-Pass $Label
    } else {
        Report-Fail "$Label`: expected exit $Expected, got $($script:launcherStatus)"
    }
}

function Assert-Contains {
    param([string] $Label, [string] $Path, [string] $Needle)

    if ((Get-Text $Path).Contains($Needle)) {
        Report-Pass $Label
    } else {
        Report-Fail "$Label`: '$Needle' not found in $([System.IO.Path]::GetFileName($Path))"
    }
}

function Assert-Lacks {
    param([string] $Label, [string] $Path, [string] $Needle)

    if ((Get-Text $Path).Contains($Needle)) {
        Report-Fail "$Label`: '$Needle' unexpectedly found in $([System.IO.Path]::GetFileName($Path))"
    } else {
        Report-Pass $Label
    }
}

# Invoke-Launcher <name> <environment> <arguments>
#
# Clears every BrineCDX environment variable first so cases cannot leak into
# each other, then runs the launcher in a child process.
function Invoke-Launcher {
    param(
        [string] $Name,
        [hashtable] $Environment = @{},
        [string[]] $Arguments = @()
    )

    foreach ($variable in @(
            'BRINE_EXEC_SERVER_URL',
            'BRINE_EXEC_SERVER_TOKEN',
            'BRINECDX_MODEL',
            'BRINECDX_MODEL_PROVIDER',
            'BRINECDX_REASONING_EFFORT',
            'BRINECDX_MODEL_CATALOG',
            'BRINECDX_DRY_RUN',
            'BRINECDX_CODEX_BIN')) {
        Remove-Item -LiteralPath "Env:$variable" -ErrorAction SilentlyContinue
    }

    foreach ($key in $Environment.Keys) {
        Set-Item -LiteralPath "Env:$key" -Value $Environment[$key]
    }

    $stdout = Join-Path $tempRoot "$Name.out"
    $stderr = Join-Path $tempRoot "$Name.err"
    $childArguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $launcher) + $Arguments
    $process = Start-Process -FilePath 'powershell' -ArgumentList $childArguments -Wait -PassThru `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr

    $script:launcherStatus = $process.ExitCode
    $script:outFile = $stdout
    $script:errFile = $stderr
}

try {
    Invoke-Launcher -Name 'help' -Arguments @('--brinecdx-help')
    Assert-Status 'help exits successfully without Brine configuration' 0
    Assert-Contains 'help prints launcher usage' $outFile 'Usage: brinecdx'

    Invoke-Launcher -Name 'missing-url' -Environment @{ BRINECDX_DRY_RUN = '1' }
    Assert-Status 'missing BRINE_EXEC_SERVER_URL fails closed' 2
    Assert-Contains 'missing URL explains the missing variable' $errFile 'BRINE_EXEC_SERVER_URL is not set'
    Assert-Lacks 'missing URL never resolves an argv' $outFile 'brinecdx-argv:'

    Invoke-Launcher -Name 'url-none' -Environment @{
        BRINE_EXEC_SERVER_URL = 'none'
        BRINECDX_DRY_RUN      = '1'
    }
    Assert-Status 'BRINE_EXEC_SERVER_URL=none fails closed' 2
    Assert-Contains 'none explains that execution is disabled' $errFile 'none disables execution'

    Invoke-Launcher -Name 'missing-catalog' -Environment @{
        BRINE_EXEC_SERVER_URL  = 'ws://127.0.0.1:8766'
        BRINECDX_MODEL_CATALOG = (Join-Path $tempRoot 'absent.json')
        BRINECDX_DRY_RUN       = '1'
    }
    Assert-Status 'missing model catalog fails closed' 2
    Assert-Contains 'missing catalog reports the path' $errFile 'model catalog not found'

    Invoke-Launcher -Name 'chatgpt-overrides' -Environment @{
        BRINE_EXEC_SERVER_URL = 'wss://brine.test/exec'
        BRINECDX_DRY_RUN      = '1'
    } -Arguments @('--sandbox', 'read-only')
    Assert-Status 'launcher resolves with a Brine endpoint' 0
    Assert-Contains 'model pin' $outFile "model='gpt-5.6-sol'"
    Assert-Contains 'provider pin' $outFile "model_provider='openai'"
    Assert-Contains 'reasoning effort pin' $outFile "model_reasoning_effort='medium'"
    Assert-Contains 'catalog escapes the machine-wide catalog' $outFile "model_catalog_json='$catalog'"
    Assert-Contains 'login method escapes API-key-only policy' $outFile "forced_login_method='chatgpt'"
    Assert-Contains 'user arguments are forwarded' $outFile '--sandbox'
    Assert-Contains 'unset token warns without blocking' $errFile 'warning: BRINE_EXEC_SERVER_TOKEN is unset'
    Assert-Contains 'unset token is reported' $errFile 'token=unset'

    Invoke-Launcher -Name 'token-set' -Environment @{
        BRINE_EXEC_SERVER_URL   = 'wss://brine.test/exec'
        BRINE_EXEC_SERVER_TOKEN = 'secret-token'
        BRINECDX_DRY_RUN        = '1'
    }
    Assert-Status 'launcher resolves with a Brine token' 0
    Assert-Contains 'token is reported as set' $errFile 'token=set'
    Assert-Lacks 'a configured token does not warn' $errFile 'warning:'
    Assert-Lacks 'the token never reaches the Codex argv' $outFile 'secret-token'

    $stub = Join-Path $tempRoot 'stub.cmd'
    "@echo off`r`necho stub-args: %*`r`nexit /b 7" | Set-Content -LiteralPath $stub -Encoding Ascii
    Invoke-Launcher -Name 'stub' -Environment @{
        BRINE_EXEC_SERVER_URL = 'wss://brine.test/exec'
        BRINECDX_CODEX_BIN    = $stub
    } -Arguments @('--sandbox', 'read-only')
    Assert-Status 'launcher propagates the Codex exit code' 7
    Assert-Contains 'the stub receives the ChatGPT overrides' $outFile "model='gpt-5.6-sol'"
    Assert-Contains 'the stub receives the user arguments' $outFile '--sandbox'
} finally {
    if ($tempRoot.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($failures -ne 0) {
    [Console]::Error.WriteLine("$failures launcher test(s) failed")
    exit 1
}

Write-Host "`nall launcher tests passed"
