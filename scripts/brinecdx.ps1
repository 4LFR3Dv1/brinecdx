# BrineCDX launcher (BCDX-WP-02).
#
# Windows counterpart of scripts/brinecdx.sh. Starts the Codex CLI from this
# checkout against the Brine execution environment with the ChatGPT-backed
# provider selected explicitly. The machine-wide ~/.codex/config.toml is never
# modified: every BrineCDX-owned setting is passed as a `-c` override, and later
# user arguments win over the launcher defaults.
#
# BrineCDX fails closed. BRINE_EXEC_SERVER_URL must name a Brine exec-server and
# must not be `none`; there is no host-local execution fallback.

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

Launcher-owned Codex settings (override with later `-c` arguments):
  model                   gpt-5.6-sol   (BRINECDX_MODEL)
  model_provider          openai        (BRINECDX_MODEL_PROVIDER)
  model_reasoning_effort  medium        (BRINECDX_REASONING_EFFORT)
  model_catalog_json      codex-rs/models-manager/models.json (BRINECDX_MODEL_CATALOG)
  forced_login_method     chatgpt

The catalog override escapes machine-wide model catalogs that do not contain
the Codex models, and forced_login_method=chatgpt escapes API-key-only login
policies. Both exist because ~/.codex/config.toml may be configured for another
provider.

Required:
  BRINE_EXEC_SERVER_URL   ws:// or wss:// endpoint of the Brine exec-server.
                          BrineCDX never falls back to host-local execution.

Optional:
  BRINE_EXEC_SERVER_TOKEN Codex sends this as `Authorization: Bearer <token>`.
  BRINECDX_CODEX_BIN      run this Codex binary instead of `cargo run`.
  BRINECDX_DRY_RUN=1      print the resolved argv and exit without running.

Example:
  $env:BRINE_EXEC_SERVER_URL = 'wss://brine.example/exec'
  $env:BRINE_EXEC_SERVER_TOKEN = '...'
  scripts/brinecdx.ps1
'@ | Write-Host
}

function Get-EnvValue {
    param([string] $Name)

    $value = [Environment]::GetEnvironmentVariable($Name)
    if ($null -eq $value) {
        return $null
    }

    return $value.Trim()
}

# Pass values as TOML literal strings so Windows paths and shell
# metacharacters survive the config round-trip unchanged.
function ConvertTo-TomlLiteral {
    param([string] $Value)

    if ($Value.Contains("'")) {
        Fail "value must not contain a single quote: $Value"
    }

    return "'$Value'"
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

$BrineUrl = Get-EnvValue 'BRINE_EXEC_SERVER_URL'
if (-not $BrineUrl) {
    Fail 'BRINE_EXEC_SERVER_URL is not set; BrineCDX has no host-local fallback'
}
if ($BrineUrl -eq 'none') {
    Fail 'BRINE_EXEC_SERVER_URL=none disables execution; BrineCDX requires a Brine endpoint'
}
if (-not (Test-Path -LiteralPath $Catalog -PathType Leaf)) {
    Fail "model catalog not found: $Catalog"
}

$Token = Get-EnvValue 'BRINE_EXEC_SERVER_TOKEN'
if ($Token) {
    $TokenState = 'set'
} else {
    $TokenState = 'unset'
    Write-LauncherError 'warning: BRINE_EXEC_SERVER_TOKEN is unset; the Brine endpoint must accept an unauthenticated upgrade'
}

$Overrides = @(
    '-c', "model=$(ConvertTo-TomlLiteral $Model)"
    '-c', "model_provider=$(ConvertTo-TomlLiteral $ModelProvider)"
    '-c', "model_reasoning_effort=$(ConvertTo-TomlLiteral $ReasoningEffort)"
    '-c', "model_catalog_json=$(ConvertTo-TomlLiteral $Catalog)"
    '-c', "forced_login_method=$(ConvertTo-TomlLiteral 'chatgpt')"
)

[Console]::Error.WriteLine("brinecdx: model=$Model provider=$ModelProvider effort=$ReasoningEffort")
[Console]::Error.WriteLine("brinecdx: brine exec-server=$BrineUrl token=$TokenState")

if ((Get-EnvValue 'BRINECDX_DRY_RUN') -eq '1') {
    foreach ($Argument in ($Overrides + $CodexArgs)) {
        Write-Output "brinecdx-argv: $Argument"
    }

    exit 0
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

if ($LASTEXITCODE) {
    exit $LASTEXITCODE
}
