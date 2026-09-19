#!/usr/bin/env bash
#
# Launcher tests for BCDX-WP-02.
#
# Verifies the BrineCDX launcher selects the ChatGPT-backed provider, escapes
# machine-wide DeepSeek settings, passes user arguments through, and fails
# closed when the Brine execution environment is missing.
#
# Usage: bash scripts/brinecdx_test.sh

set -uo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
launcher="$script_dir/brinecdx.sh"
catalog="$repo_root/codex-rs/models-manager/models.json"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/brinecdx-test.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

failures=0

report_pass() {
  printf 'ok   %s\n' "$1"
}

report_fail() {
  printf 'FAIL %s\n' "$1"
  failures=$((failures + 1))
}

# run_launcher <case-name> [NAME=value]... -- [codex-args]...
#
# Clears every BrineCDX environment variable first so cases cannot leak into
# each other.
run_launcher() {
  local name="$1"
  shift
  local -a environment=()
  while [[ "$1" != '--' ]]; do
    environment+=("$1")
    shift
  done
  shift

  out_file="$tmp_dir/${name}.out"
  err_file="$tmp_dir/${name}.err"

  env -u BRINE_EXEC_SERVER_URL -u BRINE_EXEC_SERVER_TOKEN \
    -u BRINECDX_MODEL -u BRINECDX_MODEL_PROVIDER -u BRINECDX_REASONING_EFFORT \
    -u BRINECDX_MODEL_CATALOG -u BRINECDX_DRY_RUN -u BRINECDX_CODEX_BIN \
    "${environment[@]}" \
    bash "$launcher" "$@" >"$out_file" 2>"$err_file"
  launcher_status=$?
}

assert_status() {
  local label="$1"
  local expected="$2"
  if [[ "$launcher_status" == "$expected" ]]; then
    report_pass "$label"
  else
    report_fail "$label: expected exit $expected, got $launcher_status"
  fi
}

assert_contains() {
  local label="$1"
  local file="$2"
  local needle="$3"
  if grep -Fq -- "$needle" "$file"; then
    report_pass "$label"
  else
    report_fail "$label: '$needle' not found in $(basename "$file")"
  fi
}

assert_lacks() {
  local label="$1"
  local file="$2"
  local needle="$3"
  if grep -Fq -- "$needle" "$file"; then
    report_fail "$label: '$needle' unexpectedly found in $(basename "$file")"
  else
    report_pass "$label"
  fi
}

assert_before() {
  local label="$1"
  local file="$2"
  local first="$3"
  local second="$4"
  local first_line second_line
  first_line="$(grep -Fn -- "$first" "$file" | head -n 1 | cut -d: -f1)"
  second_line="$(grep -Fn -- "$second" "$file" | head -n 1 | cut -d: -f1)"
  if [[ -n "$first_line" && -n "$second_line" && "$first_line" -lt "$second_line" ]]; then
    report_pass "$label"
  else
    report_fail "$label: expected '$first' before '$second'"
  fi
}

run_launcher help -- --brinecdx-help
assert_status 'help exits successfully without Brine configuration' 0
assert_contains 'help prints launcher usage' "$out_file" 'Usage: brinecdx'

run_launcher missing-url BRINECDX_DRY_RUN=1 -- 
assert_status 'missing BRINE_EXEC_SERVER_URL fails closed' 2
assert_contains 'missing URL explains the missing variable' "$err_file" 'BRINE_EXEC_SERVER_URL is not set'
assert_lacks 'missing URL never resolves an argv' "$out_file" 'brinecdx-argv:'

run_launcher url-none BRINE_EXEC_SERVER_URL=none BRINECDX_DRY_RUN=1 --
assert_status 'BRINE_EXEC_SERVER_URL=none fails closed' 2
assert_contains 'none explains that execution is disabled' "$err_file" 'none disables execution'

run_launcher missing-catalog \
  BRINE_EXEC_SERVER_URL=ws://127.0.0.1:8766 \
  BRINECDX_MODEL_CATALOG="$tmp_dir/absent.json" \
  BRINECDX_DRY_RUN=1 --
assert_status 'missing model catalog fails closed' 2
assert_contains 'missing catalog reports the path' "$err_file" 'model catalog not found'

run_launcher chatgpt-overrides \
  BRINE_EXEC_SERVER_URL=wss://brine.test/exec \
  BRINECDX_DRY_RUN=1 -- --sandbox read-only
assert_status 'launcher resolves with a Brine endpoint' 0
assert_contains 'model is pinned to the ChatGPT Codex model' "$out_file" "-c"
assert_contains 'model pin' "$out_file" "model='gpt-5.6-sol'"
assert_contains 'provider pin' "$out_file" "model_provider='openai'"
assert_contains 'reasoning effort pin' "$out_file" "model_reasoning_effort='medium'"
assert_contains 'catalog escapes the machine-wide catalog' "$out_file" "model_catalog_json='$catalog'"
assert_contains 'login method escapes API-key-only policy' "$out_file" "forced_login_method='chatgpt'"
assert_contains 'user arguments are forwarded' "$out_file" '--sandbox'
assert_contains 'user argument values are forwarded' "$out_file" 'read-only'
assert_before 'launcher overrides precede user arguments' "$out_file" \
  "forced_login_method='chatgpt'" '--sandbox'
assert_contains 'unset token warns without blocking' "$err_file" 'warning: BRINE_EXEC_SERVER_TOKEN is unset'
assert_contains 'unset token is reported' "$err_file" 'token=unset'

run_launcher token-set \
  BRINE_EXEC_SERVER_URL=wss://brine.test/exec \
  BRINE_EXEC_SERVER_TOKEN=secret-token \
  BRINECDX_DRY_RUN=1 --
assert_status 'launcher resolves with a Brine token' 0
assert_contains 'token is reported as set' "$err_file" 'token=set'
assert_lacks 'a configured token does not warn' "$err_file" 'warning:'
assert_lacks 'the token never reaches the Codex argv' "$out_file" 'secret-token'

run_launcher custom-model \
  BRINE_EXEC_SERVER_URL=wss://brine.test/exec \
  BRINECDX_MODEL=gpt-6-astra \
  BRINECDX_MODEL_PROVIDER=openai \
  BRINECDX_REASONING_EFFORT=high \
  BRINECDX_DRY_RUN=1 --
assert_status 'launcher resolves with model overrides' 0
assert_contains 'model override is honored' "$out_file" "model='gpt-6-astra'"
assert_contains 'reasoning override is honored' "$out_file" "model_reasoning_effort='high'"

if [[ "$failures" -ne 0 ]]; then
  printf '\n%d launcher test(s) failed\n' "$failures" >&2
  exit 1
fi

printf '\nall launcher tests passed\n'
