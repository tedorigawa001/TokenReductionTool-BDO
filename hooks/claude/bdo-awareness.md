# Bushido - Rust Token Killer

**Usage**: Token-optimized CLI proxy (60-90% savings on dev operations)

## Meta Commands (always use bdo directly)

```bash
bdo gain              # Show token savings analytics
bdo gain --history    # Show command usage history with savings
bdo discover          # Analyze Claude Code history for missed opportunities
bdo proxy <cmd>       # Execute raw command without filtering (for debugging)
```

## Installation Verification

```bash
bdo --version         # Should show: bdo X.Y.Z
bdo gain              # Should work (not "command not found")
which bdo             # Verify correct binary
```

## Hook-Based Usage

All other commands are automatically rewritten by the Claude Code hook.
Example: `git status` → `bdo git status` (transparent, 0 tokens overhead)

Refer to CLAUDE.md for full command reference.
