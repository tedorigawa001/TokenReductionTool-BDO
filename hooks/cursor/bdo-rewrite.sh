#!/usr/bin/env bash
# bdo-hook-version: 1
# Bushido Cursor Agent hook — rewrites shell commands to use bdo for token savings.
# Works with both Cursor editor and cursor-cli (they share ~/.cursor/hooks.json).
# Cursor preToolUse hook format: receives JSON on stdin, returns JSON on stdout.
# Requires: bdo >= 0.23.0, jq
#
# This is a thin delegating hook: all rewrite logic lives in `bdo rewrite`,
# which is the single source of truth (src/discover/registry.rs).
# To add or change rewrite rules, edit the Rust registry — not this file.

if ! command -v jq &>/dev/null; then
  echo "[bdo] WARNING: jq is not installed. Hook cannot rewrite commands. Install jq: https://jqlang.github.io/jq/download/" >&2
  exit 0
fi

if ! command -v bdo &>/dev/null; then
  echo "[bdo] WARNING: bdo is not installed or not in PATH. Hook cannot rewrite commands. Install: https://github.com/tedorigawa001/TokenReductionTool#installation" >&2
  exit 0
fi

# Version guard: bdo rewrite was added in 0.23.0.
BDO_VERSION=$(bdo --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
if [ -n "$BDO_VERSION" ]; then
  MAJOR=$(echo "$BDO_VERSION" | cut -d. -f1)
  MINOR=$(echo "$BDO_VERSION" | cut -d. -f2)
  if [ "$MAJOR" -eq 0 ] && [ "$MINOR" -lt 23 ]; then
    echo "[bdo] WARNING: bdo $BDO_VERSION is too old (need >= 0.23.0). Upgrade: cargo install bdo" >&2
    exit 0
  fi
fi

INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

if [ -z "$CMD" ]; then
  echo '{}'
  exit 0
fi

# Delegate all rewrite logic to the Rust binary.
# bdo rewrite exits 1 when there's no rewrite — hook passes through silently.
REWRITTEN=$(bdo rewrite "$CMD" 2>/dev/null) || { echo '{}'; exit 0; }

# No change — nothing to do.
if [ "$CMD" = "$REWRITTEN" ]; then
  echo '{}'
  exit 0
fi

jq -n --arg cmd "$REWRITTEN" '{
  "permission": "allow",
  "updated_input": { "command": $cmd }
}'
