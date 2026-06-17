#!/usr/bin/env bash
# bdo-hook-version: 3
# Bushido Claude Code hook — rewrites commands to use bdo for token savings.
# Requires: bdo >= 0.23.0, jq
#
# This is a thin delegating hook: all rewrite logic lives in `bdo rewrite`,
# which is the single source of truth (src/discover/registry.rs).
# To add or change rewrite rules, edit the Rust registry — not this file.
#
# Exit code protocol for `bdo rewrite`:
#   0 + stdout  Rewrite found, no deny/ask rule matched → auto-allow
#   1           No Bushido equivalent → pass through unchanged
#   2           Deny rule matched → pass through (Claude Code native deny handles it)
#   3 + stdout  Ask rule matched → rewrite but let Claude Code prompt the user

if ! command -v jq &>/dev/null; then
  echo "[bdo] WARNING: jq is not installed. Hook cannot rewrite commands. Install jq: https://jqlang.github.io/jq/download/" >&2
  exit 0
fi

if ! command -v bdo &>/dev/null; then
  echo "[bdo] WARNING: bdo is not installed or not in PATH. Hook cannot rewrite commands. Install: https://github.com/tedorigawa001/TokenReductionTool#installation" >&2
  exit 0
fi

# Version guard: bdo rewrite was added in 0.23.0.
# Older binaries: warn once and exit cleanly (no silent failure).
# Cache the version check to avoid spawning multiple processes on every hook call.
CACHE_DIR=${XDG_CACHE_HOME:-$HOME/.cache}
CACHE_FILE="$CACHE_DIR/bdo-hook-version-ok"
if [ ! -f "$CACHE_FILE" ]; then
  BDO_VERSION_RAW=$(bdo --version 2>/dev/null)
  BDO_VERSION=${BDO_VERSION_RAW#bdo }
  BDO_VERSION=${BDO_VERSION%% *}
  if [ -n "$BDO_VERSION" ]; then
    IFS=. read -r MAJOR MINOR PATCH <<<"$BDO_VERSION"
    # Require >= 0.23.0
    if [ "$MAJOR" -eq 0 ] && [ "$MINOR" -lt 23 ]; then
      echo "[bdo] WARNING: bdo $BDO_VERSION is too old (need >= 0.23.0). Upgrade: cargo install bdo" >&2
      exit 0
    fi
  fi
  mkdir -p "$CACHE_DIR" 2>/dev/null
  touch "$CACHE_FILE" 2>/dev/null
fi

INPUT=$(cat)
CMD=$(jq -r '.tool_input.command // empty' <<<"$INPUT")

if [ -z "$CMD" ]; then
  exit 0
fi

# Delegate all rewrite + permission logic to the Rust binary.
REWRITTEN=$(bdo rewrite "$CMD" 2>/dev/null)
EXIT_CODE=$?

case $EXIT_CODE in
  0)
    # Rewrite found, no permission rules matched — safe to auto-allow.
    # If the output is identical, the command was already using Bushido.
    [ "$CMD" = "$REWRITTEN" ] && exit 0
    ;;
  1)
    # No Bushido equivalent — pass through unchanged.
    exit 0
    ;;
  2)
    # Deny rule matched — let Claude Code's native deny rule handle it.
    exit 0
    ;;
  3)
    # Ask rule matched — rewrite the command but do NOT auto-allow so that
    # Claude Code prompts the user for confirmation.
    ;;
  *)
    exit 0
    ;;
esac

if [ "$EXIT_CODE" -eq 3 ]; then
  # Ask: rewrite the command, omit permissionDecision so Claude Code prompts.
  jq -c --arg cmd "$REWRITTEN" \
    '.tool_input.command = $cmd | {
      "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "updatedInput": .tool_input
      }
    }' <<<"$INPUT"
else
  # Allow: rewrite the command and auto-allow.
  jq -c --arg cmd "$REWRITTEN" \
    '.tool_input.command = $cmd | {
      "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "allow",
        "permissionDecisionReason": "Bushido auto-rewrite",
        "updatedInput": .tool_input
      }
    }' <<<"$INPUT"
fi
