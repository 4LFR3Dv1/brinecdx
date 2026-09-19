#!/usr/bin/env bash
#
# BrineCDX launcher (BCDX-WP-02).
#
# Starts the Codex CLI from this checkout against the Brine execution
# environment with the ChatGPT-backed provider selected explicitly. The
# machine-wide ~/.codex/config.toml is never modified: every BrineCDX-owned
# setting is passed as a `-c` override, and later user arguments win over the
# launcher defaults.
#
# BrineCDX fails closed. BRINE_EXEC_SERVER_URL must name a Brine exec-server and
# must not be `none`; there is no host-local execution fallback.

set -euo pipefail

default_model='gpt-5.6-sol'
default_model_provider='openai'
default_reasoning_effort='medium'

usage() {
  cat <<'EOF'
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
  export BRINE_EXEC_SERVER_URL=wss://brine.example/exec
  export BRINE_EXEC_SERVER_TOKEN=...
  scripts/brinecdx.sh
EOF
}

fail() {
  printf 'brinecdx: %s\n' "$*" >&2
  exit 2
}

# Pass values as TOML literal strings so Windows paths and shell
# metacharacters survive the config round-trip unchanged.
toml_literal() {
  case "$1" in
    *"'"*) fail "value must not contain a single quote: $1" ;;
    *) printf "'%s'" "$1" ;;
  esac
}

if [[ "${1:-}" == "--brinecdx-help" ]]; then
  usage
  exit 0
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"

model="${BRINECDX_MODEL:-$default_model}"
model_provider="${BRINECDX_MODEL_PROVIDER:-$default_model_provider}"
reasoning_effort="${BRINECDX_REASONING_EFFORT:-$default_reasoning_effort}"
catalog="${BRINECDX_MODEL_CATALOG:-$repo_root/codex-rs/models-manager/models.json}"

brine_url="$(printf '%s' "${BRINE_EXEC_SERVER_URL:-}" | tr -d '[:space:]')"
if [[ -z "$brine_url" ]]; then
  fail "BRINE_EXEC_SERVER_URL is not set; BrineCDX has no host-local fallback"
fi
if [[ "${brine_url,,}" == "none" ]]; then
  fail "BRINE_EXEC_SERVER_URL=none disables execution; BrineCDX requires a Brine endpoint"
fi
if [[ ! -f "$catalog" ]]; then
  fail "model catalog not found: $catalog"
fi

if [[ -n "${BRINE_EXEC_SERVER_TOKEN:-}" ]]; then
  token_state='set'
else
  token_state='unset'
  printf 'brinecdx: warning: BRINE_EXEC_SERVER_TOKEN is unset; the Brine endpoint must accept an unauthenticated upgrade\n' >&2
fi

overrides=(
  -c "model=$(toml_literal "$model")"
  -c "model_provider=$(toml_literal "$model_provider")"
  -c "model_reasoning_effort=$(toml_literal "$reasoning_effort")"
  -c "model_catalog_json=$(toml_literal "$catalog")"
  -c "forced_login_method=$(toml_literal 'chatgpt')"
)

printf 'brinecdx: model=%s provider=%s effort=%s\n' "$model" "$model_provider" "$reasoning_effort" >&2
printf 'brinecdx: brine exec-server=%s token=%s\n' "$brine_url" "$token_state" >&2

if [[ "${BRINECDX_DRY_RUN:-}" == '1' ]]; then
  for argument in "${overrides[@]}" "$@"; do
    printf 'brinecdx-argv: %s\n' "$argument"
  done
  exit 0
fi

if [[ -n "${BRINECDX_CODEX_BIN:-}" ]]; then
  exec "$BRINECDX_CODEX_BIN" "${overrides[@]}" "$@"
fi

exec cargo run \
  --manifest-path "$repo_root/codex-rs/Cargo.toml" \
  -p codex-cli \
  --bin codex \
  -- \
  "${overrides[@]}" \
  "$@"
