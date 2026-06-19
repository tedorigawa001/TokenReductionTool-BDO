# Codex CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Prompt-level guidance via awareness document -- no programmatic hook
- `bdo-awareness.md` is injected into `AGENTS.md` with an `@Bushido.md` reference
- Installed to `$CODEX_HOME` when set, otherwise `~/.codex/`, by `bdo init --codex`
