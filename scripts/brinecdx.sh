#!/usr/bin/env bash
#
# BrineCDX launcher.
#
# Local Codex execution is the default. The legacy Brine exec-server path is
# available only through BRINECDX_EXECUTION_MODE=remote.

set -euo pipefail

default_model='gpt-5.6-sol'
default_model_provider='openai'
default_reasoning_effort='medium'

usage() {
  cat <<'EOF'
Usage: brinecdx [codex-args...]

Execution:
  local   default; execute directly on this host through Codex.
  remote  set BRINECDX_EXECUTION_MODE=remote and BRINE_EXEC_SERVER_URL.

Optional:
  BRINECDX_EXECUTION_MODE local|remote (default: local)
  BRINE_EXEC_SERVER_URL   required only in remote mode
  BRINE_EXEC_SERVER_TOKEN optional bearer token in remote mode
  BRINECDX_CODEX_BIN      run this Codex binary instead of cargo run
  BRINECDX_DRY_RUN=1      print resolved argv and exit
EOF
}

fail() {
  printf 'brinecdx: %s\n' "$*" >&2
  exit 2
}

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
execution_mode="${BRINECDX_EXECUTION_MODE:-local}"
execution_mode="$(printf '%s' "$execution_mode" | tr '[:upper:]' '[:lower:]')"

case "$execution_mode" in
  local|remote) ;;
  *) fail "BRINECDX_EXECUTION_MODE must be 'local' or 'remote'" ;;
esac

if [[ ! -f "$catalog" ]]; then
  fail "model catalog not found: $catalog"
fi

if [[ "$execution_mode" == "remote" ]]; then
  brine_url="$(printf '%s' "${BRINE_EXEC_SERVER_URL:-}" | tr -d '[:space:]')"
  [[ -n "$brine_url" ]] || fail "remote execution requires BRINE_EXEC_SERVER_URL"
  [[ "${brine_url,,}" != "none" ]] || fail "BRINE_EXEC_SERVER_URL=none disables remote execution"
  token_state='unset'
  [[ -n "${BRINE_EXEC_SERVER_TOKEN:-}" ]] && token_state='set'
  printf 'brinecdx: execution=remote endpoint=%s token=%s\n' "$brine_url" "$token_state" >&2
else
  printf 'brinecdx: execution=local\n' >&2
fi

overrides=(
  -c "model=$(toml_literal "$model")"
  -c "model_provider=$(toml_literal "$model_provider")"
  -c "model_reasoning_effort=$(toml_literal "$reasoning_effort")"
  -c "model_catalog_json=$(toml_literal "$catalog")"
  -c "forced_login_method=$(toml_literal 'chatgpt')"
)

printf 'brinecdx: model=%s provider=%s effort=%s\n' "$model" "$model_provider" "$reasoning_effort" >&2

if [[ "${BRINECDX_DRY_RUN:-}" == '1' ]]; then
  for argument in "${overrides[@]}" "$@"; do
    printf 'brinecdx-argv: %s\n' "$argument"
  done
  exit 0
fi

if [[ "$execution_mode" == "local" ]]; then
  unset BRINE_EXEC_SERVER_URL
  unset BRINE_EXEC_SERVER_TOKEN
  unset CODEX_EXEC_SERVER_URL
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
