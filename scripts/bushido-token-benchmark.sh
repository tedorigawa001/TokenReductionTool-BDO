#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TARGET_DIR="${CARGO_TARGET_DIR:-/private/tmp/bdo-target-new}"
TEST_HOME="${BDO_BUSHIDO_TEST_HOME:-/private/tmp/bdo-test-home}"
BDO_BIN="${BDO_BIN:-$TARGET_DIR/debug/bdo}"

mkdir -p "$TEST_HOME/bdo/tee" "$TARGET_DIR"

if [ ! -x "$BDO_BIN" ]; then
  echo "Building bdo at $BDO_BIN" >&2
  CARGO_TARGET_DIR="$TARGET_DIR" cargo build --quiet
fi

export HOME="$TEST_HOME"
export BDO_DB_PATH="$TEST_HOME/bdo/history.db"
export BDO_TEE_DIR="$TEST_HOME/bdo/tee"
export CARGO_TARGET_DIR="$TARGET_DIR"

count_tokens() {
  local input="$1"
  local len=${#input}
  # Same rough estimator used by scripts/benchmark.sh: about 4 chars/token.
  echo $(((len + 3) / 4))
}

run_case() {
  local name="$1"
  local raw_cmd="$2"
  local bdo_cmd="$3"
  local raw_out bdo_out raw_tokens bdo_tokens saved pct raw_lines bdo_lines

  raw_out=$(eval "$raw_cmd" 2>&1 || true)
  bdo_out=$(eval "$bdo_cmd" 2>&1 || true)

  raw_tokens=$(count_tokens "$raw_out")
  bdo_tokens=$(count_tokens "$bdo_out")
  raw_lines=$(printf '%s' "$raw_out" | wc -l | tr -d ' ')
  bdo_lines=$(printf '%s' "$bdo_out" | wc -l | tr -d ' ')
  saved=$((raw_tokens - bdo_tokens))

  if [ "$raw_tokens" -gt 0 ]; then
    pct=$((saved * 100 / raw_tokens))
  else
    pct=0
  fi

  printf '%-24s %8d %8d %6d%% %7d %7d\n' \
    "$name" "$raw_tokens" "$bdo_tokens" "$pct" "$raw_lines" "$bdo_lines"
}

printf '%-24s %8s %8s %6s %7s %7s\n' CASE RAW_TOK BDO_TOK SAVE RAW_LN BDO_LN
run_case 'ls -la' 'ls -la' '"$BDO_BIN" ls -la'
run_case 'ls src' 'ls -la src' '"$BDO_BIN" ls -la src'
run_case 'find rs' 'find . -path ./target -prune -o -path ./.git -prune -o -name "*.rs" -type f -print' '"$BDO_BIN" find "*.rs"'
run_case 'grep fn' 'grep -rn "fn " src || true' '"$BDO_BIN" grep "fn " src'
run_case 'git status full' 'git status' '"$BDO_BIN" git status'
run_case 'git status short' 'git status --short' '"$BDO_BIN" git status --short'
run_case 'git log' 'git log -10 --stat' '"$BDO_BIN" git log -n 10'
run_case 'read default main' 'cat src/main.rs' '"$BDO_BIN" read src/main.rs'
run_case 'read aggressive main' 'cat src/main.rs' '"$BDO_BIN" read src/main.rs -l aggressive'
run_case 'smart main' 'cat src/main.rs' '"$BDO_BIN" smart src/main.rs'
if [ "${BDO_BENCH_SLOW:-0}" = "1" ]; then
  run_case 'cargo check' 'cargo check 2>&1 || true' '"$BDO_BIN" cargo check 2>&1 || true'
fi
