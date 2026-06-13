# Bushido - Rust Token Killer (Codex CLI)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `bdo`.

Examples:

```bash
bdo git status
bdo cargo test
bdo npm run build
bdo pytest -q
```

## Meta Commands

```bash
bdo gain            # Token savings analytics
bdo gain --history  # Recent command savings history
bdo proxy <cmd>     # Run raw command without filtering
```

## Verification

```bash
bdo --version
bdo gain
which bdo
```
