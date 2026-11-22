#!/bin/sh
# Pass workflow fixtures to the checker as data; never execute their steps.
set -eu

CHECKER="$(dirname "$0")/check-release-hardening.sh"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

expect() {
    expected=$1
    description=$2
    fixture="$TMP/workflow.yml"
    cat > "$fixture"
    status=0
    output=$(sh "$CHECKER" "$fixture" 2>&1) || status=$?
    if [ "$status" -ne "$expected" ] || ! printf '%s\n' "$output" | grep -Fq "$3"; then
        printf 'FAIL: %s (expected status %s, got %s)\n%s\n' "$description" "$expected" "$status" "$output" >&2
        return 1
    fi
    printf 'PASS: %s\n' "$description"
}

expect 1 'quoted run key with folded script' 'an expression is pasted into a run: script' <<'YAML'
on: push
permissions: read-all
jobs:
  sample:
    runs-on: ubuntu-latest
    steps:
      - 'run': >-
          echo "${{ github.repository }}"
YAML

expect 1 'omitted workflow permissions' 'top-level permissions must be explicitly read-only' <<'YAML'
on: push
jobs:
  sample:
    runs-on: ubuntu-latest
    steps:
      - run: echo ok
YAML

expect 1 'inline run in a flow mapping' 'an expression is pasted into a run: script' <<'YAML'
on: push
permissions: {}
jobs:
  sample:
    runs-on: ubuntu-latest
    steps:
      - {run: 'echo "${{ github.repository }}"'}
YAML

expect 1 'non-contents write scope in a block mapping' 'top-level permissions must be explicitly read-only' <<'YAML'
on: push
'permissions':
  contents: read
  packages: 'write'
jobs:
  sample:
    runs-on: ubuntu-latest
    steps:
      - run: echo ok
YAML

with_permissions() {
    printf 'on: push\npermissions: %s\n' "$1"
    cat <<'YAML'
jobs:
  sample:
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - env:
          REPOSITORY: ${{ github.repository }}
        run: echo "$REPOSITORY"
YAML
}

permission_pass='top-level permissions are explicitly read-only'
permission_fail='top-level permissions must be explicitly read-only'
with_permissions 'read-all' | expect 0 'read-all, job-scoped write, and expression in env' "$permission_pass"
with_permissions '{}' | expect 0 'empty permission map' "$permission_pass"
with_permissions '{contents: read, packages: none}' | expect 0 'read/none permission map' "$permission_pass"
with_permissions 'write-all' | expect 1 'write-all' "$permission_fail"
with_permissions '{contents: read, packages: write}' | expect 1 'write in a flow mapping' "$permission_fail"
with_permissions 'null' | expect 1 'null permissions' "$permission_fail"
with_permissions '{contents: read, contents: read}' | expect 1 'duplicate permission key' 'duplicate key in top-level permissions'
with_permissions '[' | expect 1 'malformed YAML' 'invalid YAML'
