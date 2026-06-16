# Claude Code Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Shell-based `PreToolUse` hook -- requires `jq` for JSON parsing
- Returns `updatedInput` JSON for transparent command rewrite (agent doesn't know Bushido is involved)
- Exits silently (exit 0) on any failure: jq missing, bdo missing, bdo too old (< 0.23.0), no match
- Version guard checks `bdo --version` against minimum 0.23.0
- `rtk-awareness.md` is a slim 10-line instructions file embedded into CLAUDE.md by `bdo init`

## Testing

```bash
# Run the full test suite (60+ assertions)
bash hooks/test-bdo-rewrite.sh

# Test against a specific hook path
HOOK=/path/to/bdo-rewrite.sh bash hooks/test-bdo-rewrite.sh

# Enable audit logging during testing
BDO_HOOK_AUDIT=1 BDO_AUDIT_DIR=/tmp bash hooks/test-bdo-rewrite.sh
```
