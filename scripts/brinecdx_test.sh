#!/usr/bin/env bash
# BrineCDX launcher tests: local execution is the default.

set -uo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
launcher="$script_dir/brinecdx.sh"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/brinecdx-test.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

failures=0
pass(){ printf 'ok   %s\n' "$1"; }
fail(){ printf 'FAIL %s\n' "$1"; failures=$((failures+1)); }

run_case() {
  local name="$1"
  shift
  local -a environment=()
  while [[ "$1" != "--" ]]; do
    environment+=("$1")
    shift
  done
  shift

  out_file="$tmp_dir/$name.out"
  err_file="$tmp_dir/$name.err"

  env -u BRINE_EXEC_SERVER_URL -u BRINE_EXEC_SERVER_TOKEN -u CODEX_EXEC_SERVER_URL \
    -u BRINECDX_EXECUTION_MODE -u BRINECDX_MODEL -u BRINECDX_MODEL_PROVIDER \
    -u BRINECDX_REASONING_EFFORT -u BRINECDX_MODEL_CATALOG -u BRINECDX_DRY_RUN \
    -u BRINECDX_CODEX_BIN "${environment[@]}" bash "$launcher" "$@" >"$out_file" 2>"$err_file"
  status=$?
}

check_status(){ [[ "$status" == "$2" ]] && pass "$1" || fail "$1: expected $2 got $status"; }
contains(){ grep -Fq -- "$3" "$2" && pass "$1" || fail "$1"; }
lacks(){ grep -Fq -- "$3" "$2" && fail "$1" || pass "$1"; }

run_case local BRINECDX_DRY_RUN=1 --
check_status 'local mode starts without exec-server URL' 0
contains 'local mode is announced' "$err_file" 'execution=local'
contains 'Codex argv is resolved' "$out_file" "model='gpt-5.6-sol'"

run_case remote-missing BRINECDX_EXECUTION_MODE=remote BRINECDX_DRY_RUN=1 --
check_status 'remote mode requires exec-server URL' 2
contains 'remote error is explicit' "$err_file" 'remote execution requires BRINE_EXEC_SERVER_URL'

run_case remote BRINECDX_EXECUTION_MODE=remote BRINE_EXEC_SERVER_URL=ws://127.0.0.1:8766 BRINECDX_DRY_RUN=1 --
check_status 'remote mode remains available' 0
contains 'remote endpoint is announced' "$err_file" 'execution=remote endpoint=ws://127.0.0.1:8766'

stub="$tmp_dir/stub.sh"
cat >"$stub" <<'EOF'
#!/usr/bin/env bash
printf 'brine-url=%s\n' "${BRINE_EXEC_SERVER_URL:-}"
printf 'codex-url=%s\n' "${CODEX_EXEC_SERVER_URL:-}"
exit 7
EOF
chmod +x "$stub"

run_case local-clears-remote \
  BRINE_EXEC_SERVER_URL=ws://127.0.0.1:8766 \
  CODEX_EXEC_SERVER_URL=ws://127.0.0.1:9999 \
  BRINECDX_CODEX_BIN="$stub" --
check_status 'child exit code is propagated' 7
lacks 'local mode suppresses BRINE_EXEC_SERVER_URL' "$out_file" '127.0.0.1:8766'
lacks 'local mode suppresses CODEX_EXEC_SERVER_URL' "$out_file" '127.0.0.1:9999'

if [[ "$failures" -ne 0 ]]; then
  printf '\n%d launcher test(s) failed\n' "$failures" >&2
  exit 1
fi
printf '\nall launcher tests passed\n'
