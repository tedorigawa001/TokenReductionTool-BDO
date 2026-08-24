# Cursor IDE Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Same delegating pattern as Claude Code hook but outputs Cursor's `updated_input` format. The legacy hook deliberately omits `permission` so Cursor retains its native confirmation boundary.
- Returns `{}` (empty JSON) when no rewrite applies -- Cursor requires JSON output for all code paths
- Requires `jq` and `bdo >= 0.23.0`
