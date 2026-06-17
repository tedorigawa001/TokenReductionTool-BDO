# Claude Code Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Shell-based `PreToolUse` hook -- requires `jq` for JSON parsing
- Returns `updatedInput` JSON for transparent command rewrite (agent doesn't know Bushido is involved)
- Exits silently (exit 0) on any failure: jq missing, bdo missing, bdo too old (< 0.23.0), no match
- Version guard checks `bdo --version` against minimum 0.23.0
- `bdo-awareness.md` is a slim 10-line instructions file embedded into CLAUDE.md by `bdo init`
