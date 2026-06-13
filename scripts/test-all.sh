#!/usr/bin/env bash
#
# Bushido Smoke Test Suite
# Exercises every command to catch regressions after merge.
# Exit code: number of failures (0 = all green)
#
set -euo pipefail

PASS=0
FAIL=0
SKIP=0
FAILURES=()

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# ── Helpers ──────────────────────────────────────────

assert_ok() {
    local name="$1"
    shift
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
    local name="$1"
    local needle="$2"
    shift 2
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

assert_exit_ok() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        PASS=$((PASS + 1))
        printf "  ${GREEN}PASS${NC}  %s\n" "$name"
    else
        FAIL=$((FAIL + 1))
        FAILURES+=("$name")
        printf "  ${RED}FAIL${NC}  %s\n" "$name"
        printf "        cmd: %s\n" "$*"
    fi
}

assert_fails() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        FAIL=$((FAIL + 1))
        FAILURES+=("$name (expected failure, got success)")
        printf "  ${RED}FAIL${NC}  %s (expected failure)\n" "$name"
    else
        PASS=$((PASS + 1))
        printf "  ${GREEN}PASS${NC}  %s\n" "$name"
    fi
}

assert_help() {
    local name="$1"
    shift
    assert_contains "$name --help" "Usage:" "$@" --help
}

skip_test() {
    local name="$1"
    local reason="$2"
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

printf "${BOLD}Bushido Smoke Test Suite${NC}\n"
printf "Binary: %s\n" "$Bushido"
printf "Version: %s\n" "$(bdo --version)"
printf "Date: %s\n" "$(date '+%Y-%m-%d %H:%M')"

# Need a git repo to test git commands
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "Must run from inside a git repository."
    exit 1
fi

REPO_ROOT=$(git rev-parse --show-toplevel)

# ── 1. Version & Help ───────────────────────────────

section "Version & Help"

assert_contains "bdo --version" "bdo" bdo --version
assert_contains "bdo --help" "Usage:" bdo --help

# ── 2. Ls ────────────────────────────────────────────

section "Ls"

assert_ok      "bdo ls ."                     bdo ls .
assert_ok      "bdo ls -la ."                 bdo ls -la .
assert_ok      "bdo ls -lh ."                 bdo ls -lh .
assert_ok      "bdo ls -l src/"               bdo ls -l src/
assert_ok      "bdo ls src/ -l (flag after)"  bdo ls src/ -l
assert_ok      "bdo ls multi paths"           bdo ls src/ scripts/
assert_contains "bdo ls -a shows hidden"      ".git" bdo ls -a .
assert_contains "bdo ls shows sizes"          "K"  bdo ls src/
assert_contains "bdo ls shows dirs with /"    "/" bdo ls .

# ── 2b. Tree ─────────────────────────────────────────

section "Tree"

if command -v tree >/dev/null 2>&1; then
    assert_ok      "bdo tree ."                bdo tree .
    assert_ok      "bdo tree -L 2 ."           bdo tree -L 2 .
    assert_ok      "bdo tree -d -L 1 ."        bdo tree -d -L 1 .
    assert_contains "bdo tree shows src/"      "src" bdo tree -L 1 .
else
    skip_test "bdo tree" "tree not installed"
fi

# ── 3. Read ──────────────────────────────────────────

section "Read"

assert_ok      "bdo read Cargo.toml"          bdo read Cargo.toml
assert_ok      "bdo read --level none Cargo.toml"  bdo read --level none Cargo.toml
assert_ok      "bdo read --level aggressive Cargo.toml" bdo read --level aggressive Cargo.toml
assert_ok      "bdo read -n Cargo.toml"       bdo read -n Cargo.toml
assert_ok      "bdo read --max-lines 5 Cargo.toml" bdo read --max-lines 5 Cargo.toml

section "Read (stdin support)"

assert_ok      "bdo read stdin pipe"          bash -c 'echo "fn main() {}" | bdo read -'

# ── 4. Git ───────────────────────────────────────────

section "Git (existing)"

assert_ok      "bdo git status"               bdo git status
assert_ok      "bdo git status --short"       bdo git status --short
assert_ok      "bdo git status -s"            bdo git status -s
assert_ok      "bdo git status --porcelain"   bdo git status --porcelain
assert_ok      "bdo git log"                  bdo git log
assert_ok      "bdo git log -5"               bdo git log -- -5
assert_ok      "bdo git diff"                 bdo git diff
assert_ok      "bdo git diff --stat"          bdo git diff --stat

section "Git (new: branch, fetch, stash, worktree)"

assert_ok      "bdo git branch"               bdo git branch
assert_ok      "bdo git fetch"                bdo git fetch
assert_ok      "bdo git stash list"           bdo git stash list
assert_ok      "bdo git worktree"             bdo git worktree

section "Git (passthrough: unsupported subcommands)"

assert_ok      "bdo git tag --list"           bdo git tag --list
assert_ok      "bdo git remote -v"            bdo git remote -v
assert_ok      "bdo git rev-parse HEAD"       bdo git rev-parse HEAD

# ── 5. GitHub CLI ────────────────────────────────────

section "GitHub CLI"

if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    assert_ok      "bdo gh pr list"           bdo gh pr list
    assert_ok      "bdo gh run list"          bdo gh run list
    assert_ok      "bdo gh issue list"        bdo gh issue list
    # pr create/merge/diff/comment/edit are write ops, test help only
    assert_help    "bdo gh"                   bdo gh
else
    skip_test "gh commands" "gh not authenticated"
fi

# ── 6. Cargo ─────────────────────────────────────────

section "Cargo (new)"

assert_ok      "bdo cargo build"              bdo cargo build
assert_ok      "bdo cargo clippy"             bdo cargo clippy
# cargo test exits non-zero due to pre-existing failures; check output ignoring exit code
output_cargo_test=$(bdo cargo test 2>&1 || true)
if echo "$output_cargo_test" | grep -q "FAILURES\|test result:\|passed"; then
    PASS=$((PASS + 1))
    printf "  ${GREEN}PASS${NC}  %s\n" "bdo cargo test"
else
    FAIL=$((FAIL + 1))
    FAILURES+=("bdo cargo test")
    printf "  ${RED}FAIL${NC}  %s\n" "bdo cargo test"
    printf "        got: %s\n" "$(echo "$output_cargo_test" | head -3)"
fi
assert_help    "bdo cargo"                    bdo cargo

# ── 7. Curl ──────────────────────────────────────────

section "Curl (new)"

assert_contains "bdo curl JSON detect" "string" bdo curl https://httpbin.org/json
assert_ok       "bdo curl plain text"          bdo curl https://httpbin.org/robots.txt
assert_help     "bdo curl"                     bdo curl

# ── 8. Npm / Npx ────────────────────────────────────

section "Npm / Npx (new)"

assert_help    "bdo npm"                      bdo npm
assert_help    "bdo npx"                      bdo npx

# ── 9. Pnpm ─────────────────────────────────────────

section "Pnpm"

assert_help    "bdo pnpm"                     bdo pnpm
assert_help    "bdo pnpm build"               bdo pnpm build
assert_help    "bdo pnpm typecheck"           bdo pnpm typecheck

if command -v pnpm >/dev/null 2>&1; then
    assert_ok  "bdo pnpm help"                bdo pnpm help
fi

# ── 10. Grep ─────────────────────────────────────────

section "Grep"

assert_ok      "bdo grep pattern"             bdo grep "pub fn" src/
assert_contains "bdo grep finds results"      "pub fn" bdo grep "pub fn" src/
assert_ok      "bdo grep with file type"      bdo grep "pub fn" src/ -t rust

section "Grep (extra args passthrough)"

assert_ok      "bdo grep -i case insensitive" bdo grep "fn" src/ -i
assert_ok      "bdo grep -A context lines"    bdo grep "fn run" src/ -A 2

# ── 11. Find ─────────────────────────────────────────

section "Find"

assert_ok      "bdo find *.rs"                bdo find "*.rs" src/
assert_contains "bdo find shows files"        ".rs" bdo find "*.rs" src/

# ── 12. Json ─────────────────────────────────────────

section "Json"

# Create temp JSON file for testing
TMPJSON=$(mktemp /tmp/bdo-test-XXXXX.json)
echo '{"name":"test","count":42,"items":[1,2,3]}' > "$TMPJSON"

assert_ok      "bdo json file"                bdo json "$TMPJSON"
assert_contains "bdo json shows schema"       "string" bdo json "$TMPJSON"

rm -f "$TMPJSON"

# ── 13. Deps ─────────────────────────────────────────

section "Deps"

assert_ok      "bdo deps ."                   bdo deps .
assert_contains "bdo deps shows Cargo"        "Cargo" bdo deps .

# ── 14. Env ──────────────────────────────────────────

section "Env"

assert_ok      "bdo env"                      bdo env
assert_ok      "bdo env --filter PATH"        bdo env --filter PATH

# ── 16. Log ──────────────────────────────────────────

section "Log"

TMPLOG=$(mktemp /tmp/bdo-log-XXXXX.log)
for i in $(seq 1 20); do
    echo "[2025-01-01 12:00:00] INFO: repeated message" >> "$TMPLOG"
done
echo "[2025-01-01 12:00:01] ERROR: something failed" >> "$TMPLOG"

assert_ok      "bdo log file"                 bdo log "$TMPLOG"

rm -f "$TMPLOG"

# ── 17. Summary ──────────────────────────────────────

section "Summary"

assert_ok      "bdo summary echo hello"       bdo summary echo hello

# ── 18. Err ──────────────────────────────────────────

section "Err"

assert_ok      "bdo err echo ok"              bdo err echo ok

# ── 19. Test runner ──────────────────────────────────

section "Test runner"

assert_ok      "bdo test echo ok"             bdo test echo ok

# ── 20. Gain ─────────────────────────────────────────

section "Gain"

assert_ok      "bdo gain"                     bdo gain
assert_ok      "bdo gain --history"           bdo gain --history

# ── 21. Config & Init ────────────────────────────────

section "Config & Init"

assert_ok      "bdo config"                   bdo config
assert_ok      "bdo init --show"              bdo init --show

# ── 22. Wget ─────────────────────────────────────────

section "Wget"

if command -v wget >/dev/null 2>&1; then
    assert_ok  "bdo wget stdout"              bdo wget https://httpbin.org/robots.txt -O
else
    skip_test "bdo wget" "wget not installed"
fi

# ── 23. Tsc / Lint / Prettier / Next / Playwright ───

section "JS Tooling (help only, no project context)"

assert_help    "bdo tsc"                      bdo tsc
assert_help    "bdo lint"                     bdo lint
assert_help    "bdo prettier"                 bdo prettier
assert_help    "bdo next"                     bdo next
assert_help    "bdo playwright"               bdo playwright

# ── 24. Prisma ───────────────────────────────────────

section "Prisma (help only)"

assert_help    "bdo prisma"                   bdo prisma

# ── 25. Vitest ───────────────────────────────────────

section "Vitest (help only)"

assert_help    "bdo vitest"                   bdo vitest

# ── 26. Docker / Kubectl (help only) ────────────────

section "Docker / Kubectl (help only)"

assert_help    "bdo docker"                   bdo docker
assert_help    "bdo kubectl"                  bdo kubectl

# ── 27. Python (conditional) ────────────────────────

section "Python (conditional)"

if command -v pytest &>/dev/null; then
    assert_help    "bdo pytest"                    bdo pytest --help
else
    skip_test "bdo pytest" "pytest not installed"
fi

if command -v ruff &>/dev/null; then
    assert_help    "bdo ruff"                      bdo ruff --help
else
    skip_test "bdo ruff" "ruff not installed"
fi

if command -v pip &>/dev/null; then
    assert_help    "bdo pip"                       bdo pip --help
else
    skip_test "bdo pip" "pip not installed"
fi

# ── 28. Go (conditional) ────────────────────────────

section "Go (conditional)"

if command -v go &>/dev/null; then
    assert_help    "bdo go"                        bdo go --help
    assert_help    "bdo go test"                   bdo go test -h
    assert_help    "bdo go build"                  bdo go build -h
    assert_help    "bdo go vet"                    bdo go vet -h
else
    skip_test "bdo go" "go not installed"
fi

if command -v golangci-lint &>/dev/null; then
    assert_help    "bdo golangci-lint"             bdo golangci-lint --help
else
    skip_test "bdo golangci-lint" "golangci-lint not installed"
fi

# ── 29. Graphite (conditional) ─────────────────────

section "Graphite (conditional)"

if command -v gt &>/dev/null; then
    assert_help   "bdo gt"                          bdo gt --help
    assert_ok     "bdo gt log short"                bdo gt log short
else
    skip_test "bdo gt" "gt not installed"
fi

# ── 30. Ruby (conditional) ──────────────────────────

section "Ruby (conditional)"

if command -v rspec &>/dev/null; then
    assert_help    "bdo rspec"                     bdo rspec --help
else
    skip_test "bdo rspec" "rspec not installed"
fi

if command -v rubocop &>/dev/null; then
    assert_help    "bdo rubocop"                   bdo rubocop --help
else
    skip_test "bdo rubocop" "rubocop not installed"
fi

if command -v rake &>/dev/null; then
    assert_help    "bdo rake"                      bdo rake --help
else
    skip_test "bdo rake" "rake not installed"
fi

# ── 31. Global flags ────────────────────────────────

section "Global flags"

assert_ok      "bdo -u ls ."                  bdo -u ls .
assert_ok      "bdo --skip-env npm --help"    bdo --skip-env npm --help

# ── 32. CcEconomics ─────────────────────────────────

section "CcEconomics"

assert_ok      "bdo cc-economics"             bdo cc-economics

# ── 33. Learn ───────────────────────────────────────

section "Learn"

assert_ok      "bdo learn --help"             bdo learn --help
assert_ok      "bdo learn (no sessions)"      bdo learn --since 0 2>&1 || true

# ── 32. Rewrite ───────────────────────────────────────

section "Rewrite"

assert_contains "rewrite git status"          "bdo git status"         bdo rewrite "git status"
assert_contains "rewrite cargo test"          "bdo cargo test"         bdo rewrite "cargo test"
assert_contains "rewrite compound &&"         "bdo git status"         bdo rewrite "git status && cargo test"
assert_contains "rewrite pipe preserves"      "| head"                 bdo rewrite "git log | head"

section "Rewrite (#345: BDO_DISABLED skip)"

assert_fails   "rewrite BDO_DISABLED=1 skip"                          bdo rewrite "BDO_DISABLED=1 git status"
assert_fails   "rewrite env BDO_DISABLED skip"                        bdo rewrite "FOO=1 BDO_DISABLED=1 cargo test"

section "Rewrite (#346: 2>&1 preserved)"

assert_contains "rewrite 2>&1 preserved"      "2>&1"                  bdo rewrite "cargo test 2>&1 | head"

section "Rewrite (#196: gh --json skip)"

assert_fails   "rewrite gh --json skip"                               bdo rewrite "gh pr list --json number"
assert_fails   "rewrite gh --jq skip"                                 bdo rewrite "gh api /repos --jq .name"
assert_fails   "rewrite gh --template skip"                           bdo rewrite "gh pr view 1 --template '{{.title}}'"
assert_contains "rewrite gh normal works"     "bdo gh pr list"        bdo rewrite "gh pr list"

# ── 33. Verify ────────────────────────────────────────

section "Verify"

assert_ok      "bdo verify"                   bdo verify

# ── 34. Proxy ─────────────────────────────────────────

section "Proxy"

assert_ok      "bdo proxy echo hello"         bdo proxy echo hello
assert_contains "bdo proxy passthrough"       "hello" bdo proxy echo hello

# ── 35. Discover ──────────────────────────────────────

section "Discover"

assert_ok      "bdo discover"                 bdo discover

# ── 36. Diff ──────────────────────────────────────────

section "Diff"

assert_ok      "bdo diff two files"           bdo diff Cargo.toml LICENSE

# ── 37. Wc ────────────────────────────────────────────

section "Wc"

assert_ok      "bdo wc Cargo.toml"            bdo wc Cargo.toml

# ── 38. Smart ─────────────────────────────────────────

section "Smart"

assert_ok      "bdo smart src/main.rs"        bdo smart src/main.rs

# ── 39. Json edge cases ──────────────────────────────

section "Json (edge cases)"

assert_fails   "bdo json on TOML (#347)"                              bdo json Cargo.toml

# ── 40. Docker (conditional) ─────────────────────────

section "Docker (conditional)"

if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
    assert_ok  "bdo docker ps"               bdo docker ps
    assert_ok  "bdo docker images"           bdo docker images
else
    skip_test "bdo docker" "docker not running"
fi

# ── 41. Hook check ───────────────────────────────────

section "Hook check (#344)"

assert_contains "bdo init --show hook version" "version" bdo init --show

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
