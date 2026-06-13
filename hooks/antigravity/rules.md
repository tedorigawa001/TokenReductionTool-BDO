# Bushido - Rust Token Killer (Google Antigravity)

**Usage**: Token-optimized CLI proxy for shell commands.

## Rule

Always prefix shell commands with `bdo` to minimize token consumption.

Examples:

```bash
bdo git status
bdo cargo test
bdo ls src/
bdo grep "pattern" src/
bdo find "*.rs" .
bdo docker ps
bdo gh pr list
```

## Meta Commands

```bash
bdo gain              # Show token savings
bdo gain --history    # Command history with savings
bdo discover          # Find missed Bushido opportunities
bdo proxy <cmd>       # Run raw (no filtering, for debugging)
```

## Why

Bushido filters and compresses command output before it reaches the LLM context, saving 60-90% tokens on common operations. Always use `bdo <cmd>` instead of raw commands.
