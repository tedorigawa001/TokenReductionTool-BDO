#!/usr/bin/env bash
#
# Bushido Bushido maintenance check.
# Runs the safest local verification steps that are available on this machine.
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

pass() {
    printf 'PASS  %s\n' "$1"
}

warn() {
    printf 'WARN  %s\n' "$1"
}

TEST_HOME="${BDO_BUSHIDO_TEST_HOME:-/private/tmp/bdo-test-home}"
TEST_DATA_DIR="$TEST_HOME/bdo"
CARGO_HOME_FOR_CHECK="${CARGO_HOME:-$HOME/.cargo}"
CARGO_TARGET_DIR_FOR_CHECK="${CARGO_TARGET_DIR:-/private/tmp/bdo-target-new}"

run_if_available() {
    local tool="$1"
    local label="$2"
    shift 2

    if command -v "$tool" >/dev/null 2>&1; then
        "$@"
        pass "$label"
    else
        warn "$label skipped: $tool not found"
    fi
}

cargo_isolated() {
    env \
        HOME="$TEST_HOME" \
        CARGO_HOME="$CARGO_HOME_FOR_CHECK" \
        CARGO_TARGET_DIR="$CARGO_TARGET_DIR_FOR_CHECK" \
        BDO_DB_PATH="$TEST_DATA_DIR/history.db" \
        BDO_TEE_DIR="$TEST_DATA_DIR/tee" \
        CARGO_NET_OFFLINE="${CARGO_NET_OFFLINE:-true}" \
        cargo "$@"
}

printf 'Bushido Bushido check\n'
printf 'root: %s\n' "$ROOT"
printf 'test home: %s\n' "$TEST_HOME"

if [[ -d .git ]]; then
    pass "git repository detected"
else
    warn "not a git repository; initialize or clone with history before long-term maintenance"
fi

run_if_available rustc "rustc available" rustc --version
if command -v cargo >/dev/null 2>&1; then
    cargo_isolated metadata --no-deps --format-version 1 >/dev/null
    pass "cargo metadata available"

    cargo_isolated fmt --all --check
    pass "cargo fmt --all --check"

    cargo_isolated check
    pass "cargo check"

    cargo_isolated test --all
    pass "cargo test --all"

    cargo_isolated clippy --all-targets
    pass "cargo clippy --all-targets"
else
    warn "Rust checks skipped until cargo is installed"
fi

if [[ -x scripts/check-test-presence.sh ]]; then
    bash scripts/check-test-presence.sh
    pass "test presence check"
else
    warn "scripts/check-test-presence.sh is not executable or missing"
fi
