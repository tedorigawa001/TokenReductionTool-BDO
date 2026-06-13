#!/usr/bin/env bash
#
# Bushido Smoke Tests — Aristote Project (Vite + React + TS + ESLint)
# Tests Bushido commands in a real JS/TS project context.
# Usage: bash scripts/test-aristote.sh
#
set -euo pipefail

ARISTOTE="/Users/florianbruniaux/Sites/MethodeAristote/aristote-school-boost"

PASS=0
FAIL=0
SKIP=0
FAILURES=()

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

assert_ok() {
    local name="$1"; shift
    local output
    if output=$("$@" 2>&1); then
        PASS=$((PASS + 1))
        printf "  ${GREEN}PASS${NC}  %s\n" "$name"
    else
        FAIL=$((FAIL + 1))
        FAILURES+=("$name")
        printf "  ${RED}FAIL${NC}  %s\n" "$name"
        printf "        cmd: %s\n" "$*"
        printf "        out: %s\n" "$(echo "$output" | head -3)"
    fi
}

assert_contains() {
    local name="$1"; local needle="$2"; shift 2
    local output
    if output=$("$@" 2>&1) && echo "$output" | grep -q "$needle"; then
        PASS=$((PASS + 1))
        printf "  ${GREEN}PASS${NC}  %s\n" "$name"
    else
        FAIL=$((FAIL + 1))
        FAILURES+=("$name")
        printf "  ${RED}FAIL${NC}  %s\n" "$name"
        printf "        expected: '%s'\n" "$needle"
        printf "        got: %s\n" "$(echo "$output" | head -3)"
    fi
}

# Allow non-zero exit but check output
assert_output() {
    local name="$1"; local needle="$2"; shift 2
    local output
    output=$("$@" 2>&1) || true
    if echo "$output" | grep -q "$needle"; then
        PASS=$((PASS + 1))
        printf "  ${GREEN}PASS${NC}  %s\n" "$name"
    else
        FAIL=$((FAIL + 1))
        FAILURES+=("$name")
        printf "  ${RED}FAIL${NC}  %s\n" "$name"
        printf "        expected: '%s'\n" "$needle"
        printf "        got: %s\n" "$(echo "$output" | head -3)"
    fi
}

skip_test() {
    local name="$1"; local reason="$2"
    SKIP=$((SKIP + 1))
    printf "  ${YELLOW}SKIP${NC}  %s (%s)\n" "$name" "$reason"
}

section() {
    printf "\n${BOLD}${CYAN}── %s ──${NC}\n" "$1"
}

# ── Preamble ─────────────────────────────────────────

Bushido=$(command -v bdo || echo "")
if [[ -z "$Bushido" ]]; then
    echo "bdo not found in PATH. Run: cargo install --path ."
    exit 1
fi

if [[ ! -d "$ARISTOTE" ]]; then
    echo "Aristote project not found at $ARISTOTE"
    exit 1
fi

printf "${BOLD}Bushido Smoke Tests — Aristote Project${NC}\n"
printf "Binary: %s (%s)\n" "$Bushido" "$(bdo --version)"
printf "Project: %s\n" "$ARISTOTE"
printf "Date: %s\n\n" "$(date '+%Y-%m-%d %H:%M')"

# ── 1. File exploration ──────────────────────────────

section "Ls & Find"

assert_ok       "bdo ls project root"           bdo ls "$ARISTOTE"
assert_ok       "bdo ls src/"                   bdo ls "$ARISTOTE/src"
assert_ok       "bdo ls --depth 3"              bdo ls --depth 3 "$ARISTOTE/src"
assert_contains "bdo ls shows components/"      "components" bdo ls "$ARISTOTE/src"
assert_ok       "bdo find *.tsx"                bdo find "*.tsx" "$ARISTOTE/src"
assert_ok       "bdo find *.ts"                 bdo find "*.ts" "$ARISTOTE/src"
assert_contains "bdo find finds App.tsx"        "App.tsx" bdo find "*.tsx" "$ARISTOTE/src"

# ── 2. Read ──────────────────────────────────────────

section "Read"

assert_ok       "bdo read tsconfig.json"        bdo read "$ARISTOTE/tsconfig.json"
assert_ok       "bdo read package.json"         bdo read "$ARISTOTE/package.json"
assert_ok       "bdo read App.tsx"              bdo read "$ARISTOTE/src/App.tsx"
assert_ok       "bdo read --level aggressive"   bdo read --level aggressive "$ARISTOTE/src/App.tsx"
assert_ok       "bdo read --max-lines 10"       bdo read --max-lines 10 "$ARISTOTE/src/App.tsx"

# ── 3. Grep ──────────────────────────────────────────

section "Grep"

assert_ok       "bdo grep import"               bdo grep "import" "$ARISTOTE/src"
assert_ok       "bdo grep with type filter"     bdo grep "useState" "$ARISTOTE/src" -t tsx
assert_contains "bdo grep finds components"     "import" bdo grep "import" "$ARISTOTE/src"

# ── 4. Git ───────────────────────────────────────────

section "Git (in Aristote repo)"

# bdo git doesn't support -C, use git -C via subshell
assert_ok       "bdo git status"                bash -c "cd $ARISTOTE && bdo git status"
assert_ok       "bdo git log"                   bash -c "cd $ARISTOTE && bdo git log"
assert_ok       "bdo git branch"                bash -c "cd $ARISTOTE && bdo git branch"

# ── 5. Deps ──────────────────────────────────────────

section "Deps"

assert_ok       "bdo deps"                      bdo deps "$ARISTOTE"
assert_contains "bdo deps shows package.json"   "package.json" bdo deps "$ARISTOTE"

# ── 6. Json ──────────────────────────────────────────

section "Json"

assert_ok       "bdo json tsconfig"             bdo json "$ARISTOTE/tsconfig.json"
assert_ok       "bdo json package.json"         bdo json "$ARISTOTE/package.json"

# ── 7. Env ───────────────────────────────────────────

section "Env"

assert_ok       "bdo env"                       bdo env
assert_ok       "bdo env --filter NODE"         bdo env --filter NODE

# ── 8. Tsc ───────────────────────────────────────────

section "TypeScript (tsc)"

if command -v npx >/dev/null 2>&1 && [[ -d "$ARISTOTE/node_modules" ]]; then
    assert_output "bdo tsc (in aristote)" "error\|✅\|TS" bdo tsc --project "$ARISTOTE"
else
    skip_test "bdo tsc" "node_modules not installed"
fi

# ── 9. ESLint ────────────────────────────────────────

section "ESLint (lint)"

if command -v npx >/dev/null 2>&1 && [[ -d "$ARISTOTE/node_modules" ]]; then
    assert_output "bdo lint (in aristote)" "error\|warning\|✅\|violations\|clean" bdo lint --project "$ARISTOTE"
else
    skip_test "bdo lint" "node_modules not installed"
fi

# ── 10. Build (Vite) ─────────────────────────────────

section "Build (Vite via bdo next)"

if [[ -d "$ARISTOTE/node_modules" ]]; then
    # Aristote uses Vite, not Next — but bdo next wraps the build script
    # Test with a timeout since builds can be slow
    skip_test "bdo next build" "Vite project, not Next.js — use npm run build directly"
else
    skip_test "bdo next build" "node_modules not installed"
fi

# ── 11. Diff ─────────────────────────────────────────

section "Diff"

# Diff two config files that exist in the project
assert_ok       "bdo diff tsconfigs"            bdo diff "$ARISTOTE/tsconfig.json" "$ARISTOTE/tsconfig.app.json"

# ── 12. Summary & Err ────────────────────────────────

section "Summary & Err"

assert_ok       "bdo summary ls"                bdo summary ls "$ARISTOTE/src"
assert_ok       "bdo err ls"                    bdo err ls "$ARISTOTE/src"

# ── 13. Gain ─────────────────────────────────────────

section "Gain (after above commands)"

assert_ok       "bdo gain"                      bdo gain
assert_ok       "bdo gain --history"            bdo gain --history

# ══════════════════════════════════════════════════════
# Report
# ══════════════════════════════════════════════════════

printf "\n${BOLD}══════════════════════════════════════${NC}\n"
printf "${BOLD}Results: ${GREEN}%d passed${NC}, ${RED}%d failed${NC}, ${YELLOW}%d skipped${NC}\n" "$PASS" "$FAIL" "$SKIP"

if [[ ${#FAILURES[@]} -gt 0 ]]; then
    printf "\n${RED}Failures:${NC}\n"
    for f in "${FAILURES[@]}"; do
        printf "  - %s\n" "$f"
    done
fi

printf "${BOLD}══════════════════════════════════════${NC}\n"

exit "$FAIL"
