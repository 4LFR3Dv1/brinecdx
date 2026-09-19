# BrineCDX launcher.
#
# Local Codex execution is the default. The legacy Brine exec-server path is
# available only through BRINECDX_EXECUTION_MODE=remote.
#
# Model/provider overrides remain until the broader R0 reset removes them.

[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $CodexArgs
)

$ErrorActionPreference = 'Stop'

$DefaultModel = 'gpt-5.6-sol'
$DefaultModelProvider = 'openai'
$DefaultReasoningEffort = 'medium'

function Write-LauncherError {
    param([string] $Message)
    [Console]::Error.WriteLine("brinecdx: $Message")
}

function Fail {
    param([string] $Message)
    Write-LauncherError $Message
    exit 2
}

function Show-Usage {
    @'
Usage: brinecdx [codex-args...]

Execution:
  local   default; execute directly on this host through Codex.
  remote  set BRINECDX_EXECUTION_MODE=remote and BRINE_EXEC_SERVER_URL.

Launcher-owned Codex settings (override with later `-c` arguments):
  model                   gpt-5.6-sol   (BRINECDX_MODEL)
  model_provider          openai        (BRINECDX_MODEL_PROVIDER)
  model_reasoning_effort  medium        (BRINECDX_REASONING_EFFORT)
  model_catalog_json      codex-rs/models-manager/models.json (BRINECDX_MODEL_CATALOG)
  forced_login_method     chatgpt

Optional:
  BRINECDX_EXECUTION_MODE local|remote (default: local)
  BRINE_EXEC_SERVER_URL   required only in remote mode
  BRINE_EXEC_SERVER_TOKEN optional bearer token in remote mode
  BRINECDX_CODEX_BIN      run this Codex binary instead of cargo run
  BRINECDX_DRY_RUN=1      print resolved argv and exit
'@ | Write-Host
}

function Get-EnvValue {
    param([string] $Name)
    $value = [Environment]::GetEnvironmentVariable($Name)
    if ($null -eq $value) { return $null }
    return $value.Trim()
}

function ConvertTo-TomlLiteral {
    param([string] $Value)
    if ($Value.Contains("'")) {
        Fail "value must not contain a single quote: $Value"
    }
    return "'$Value'"
}

function Restore-EnvValue {
    param([string] $Name, [AllowNull()][string] $Value)
    if ($null -eq $Value) {
        Remove-Item -LiteralPath "Env:$Name" -ErrorAction SilentlyContinue
    } else {
        Set-Item -LiteralPath "Env:$Name" -Value $Value
    }
}

if ($CodexArgs -contains '--brinecdx-help') {
    Show-Usage
    exit 0
}

$RepoRoot = Split-Path -Parent $PSScriptRoot

$Model = Get-EnvValue 'BRINECDX_MODEL'
if (-not $Model) { $Model = $DefaultModel }

$ModelProvider = Get-EnvValue 'BRINECDX_MODEL_PROVIDER'
if (-not $ModelProvider) { $ModelProvider = $DefaultModelProvider }

$ReasoningEffort = Get-EnvValue 'BRINECDX_REASONING_EFFORT'
if (-not $ReasoningEffort) { $ReasoningEffort = $DefaultReasoningEffort }

$Catalog = Get-EnvValue 'BRINECDX_MODEL_CATALOG'
if (-not $Catalog) { $Catalog = Join-Path $RepoRoot 'codex-rs\models-manager\models.json' }
if (-not (Test-Path -LiteralPath $Catalog -PathType Leaf)) {
    Fail "model catalog not found: $Catalog"
}

$ExecutionMode = Get-EnvValue 'BRINECDX_EXECUTION_MODE'
if (-not $ExecutionMode) { $ExecutionMode = 'local' }
$ExecutionMode = $ExecutionMode.ToLowerInvariant()
if ($ExecutionMode -notin @('local', 'remote')) {
    Fail "BRINECDX_EXECUTION_MODE must be 'local' or 'remote'"
}

$BrineUrl = Get-EnvValue 'BRINE_EXEC_SERVER_URL'
$Token = Get-EnvValue 'BRINE_EXEC_SERVER_TOKEN'

if ($ExecutionMode -eq 'remote') {
    if (-not $BrineUrl) {
        Fail 'remote execution requires BRINE_EXEC_SERVER_URL'
    }
    if ($BrineUrl -eq 'none') {
        Fail 'BRINE_EXEC_SERVER_URL=none disables remote execution'
    }
    $TokenState = if ($Token) { 'set' } else { 'unset' }
    [Console]::Error.WriteLine("brinecdx: execution=remote endpoint=$BrineUrl token=$TokenState")
} else {
    [Console]::Error.WriteLine('brinecdx: execution=local')
}

$Overrides = @(
    '-c', "model=$(ConvertTo-TomlLiteral $Model)"
    '-c', "model_provider=$(ConvertTo-TomlLiteral $ModelProvider)"
    '-c', "model_reasoning_effort=$(ConvertTo-TomlLiteral $ReasoningEffort)"
    '-c', "model_catalog_json=$(ConvertTo-TomlLiteral $Catalog)"
    '-c', "forced_login_method=$(ConvertTo-TomlLiteral 'chatgpt')"
)

[Console]::Error.WriteLine("brinecdx: model=$Model provider=$ModelProvider effort=$ReasoningEffort")

if ((Get-EnvValue 'BRINECDX_DRY_RUN') -eq '1') {
    foreach ($Argument in ($Overrides + $CodexArgs)) {
        Write-Output "brinecdx-argv: $Argument"
    }
    exit 0
}

$SavedBrineUrl = [Environment]::GetEnvironmentVariable('BRINE_EXEC_SERVER_URL')
$SavedBrineToken = [Environment]::GetEnvironmentVariable('BRINE_EXEC_SERVER_TOKEN')
$SavedCodexUrl = [Environment]::GetEnvironmentVariable('CODEX_EXEC_SERVER_URL')
$ExitCode = 0

try {
    if ($ExecutionMode -eq 'local') {
        # Force the downstream environment provider onto Codex's host-local
        # environment even if these variables are persisted in the parent shell.
        Remove-Item -LiteralPath 'Env:BRINE_EXEC_SERVER_URL' -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath 'Env:BRINE_EXEC_SERVER_TOKEN' -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath 'Env:CODEX_EXEC_SERVER_URL' -ErrorAction SilentlyContinue
    }

    if ($env:BRINECDX_CODEX_BIN) {
        & $env:BRINECDX_CODEX_BIN @Overrides @CodexArgs
    } else {
        & cargo run `
            --manifest-path (Join-Path $RepoRoot 'codex-rs\Cargo.toml') `
            -p codex-cli `
            --bin codex `
            -- `
            @Overrides `
            @CodexArgs
    }

    if ($null -ne $LASTEXITCODE) { $ExitCode = $LASTEXITCODE }
} finally {
    Restore-EnvValue 'BRINE_EXEC_SERVER_URL' $SavedBrineUrl
    Restore-EnvValue 'BRINE_EXEC_SERVER_TOKEN' $SavedBrineToken
    Restore-EnvValue 'CODEX_EXEC_SERVER_URL' $SavedCodexUrl
}

if ($ExitCode) { exit $ExitCode }
