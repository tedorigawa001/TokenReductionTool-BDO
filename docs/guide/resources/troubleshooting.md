---
title: Troubleshooting
description: Common Bushido issues and how to fix them
sidebar:
  order: 2
---

# Troubleshooting

## AI assistant not using Bushido

**Symptom:** Claude Code (or another agent) runs `cargo test` instead of `bdo cargo test`.

**Checklist:**

1. Verify Bushido is installed:
   ```bash
   bdo --version
   bdo gain
   ```

2. Initialize the hook:
   ```bash
   bdo init --global    # Claude Code
   bdo init --global --cursor    # Cursor
   bdo init --global --opencode  # OpenCode
   ```

3. Restart your AI assistant.

4. Verify hook status:
   ```bash
   bdo init --show
   ```

5. Check `settings.json` has the hook registered (Claude Code):
   ```bash
   cat ~/.claude/settings.json | grep bdo
   ```

## Bushido not found after `cargo install`

**Symptom:**
```bash
$ bdo --version
zsh: command not found: bdo
```

**Cause:** `~/.cargo/bin` is not in your PATH.

**Fix:**

For bash (`~/.bashrc`) or zsh (`~/.zshrc`):
```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

For fish (`~/.config/fish/config.fish`):
```fish
set -gx PATH $HOME/.cargo/bin $PATH
```

Then reload:
```bash
source ~/.zshrc    # or ~/.bashrc
bdo --version
```

## Bushido on Windows

### Double-clicking rtk.exe does nothing

**Symptom:** You double-click `rtk.exe`, a terminal flashes and closes instantly.

**Cause:** Bushido is a command-line tool. With no arguments, it prints usage and exits. The console window opens and closes before you can read anything.

**Fix:** Open a terminal first, then run Bushido from there:
- Press `Win+R`, type `cmd`, press Enter
- Or open PowerShell or Windows Terminal
- Then run: `bdo --version`

### Hook not working (no auto-rewrite)

**Symptom:** `bdo init -g` shows "Falling back to --claude-md mode" on Windows.

**Cause:** The auto-rewrite hook (`bdo-rewrite.sh`) requires a Unix shell. Native Windows doesn't have one.

**Fix:** Use [WSL](https://learn.microsoft.com/en-us/windows/wsl/install) for full hook support:
```bash
# Inside WSL
curl -fsSL https://raw.githubusercontent.com/tedorigawa001/TokenReductionTool/refs/heads/master/install.sh | sh
bdo init -g    # full hook mode works in WSL
```

On native Windows, Bushido falls back to CLAUDE.md injection. Your AI assistant gets Bushido instructions but won't auto-rewrite commands. It can still use Bushido manually: `bdo cargo test`, `bdo git status`, etc.

### Node.js tools not found

**Symptom:**
```
bdo vitest --run
Error: program not found
```

**Cause:** On Windows, Node.js tools are installed as `.CMD`/`.BAT` wrappers. Older Bushido versions couldn't find them.

**Fix:** Update to Bushido v0.23.1+:
```bash
cargo install --git https://github.com/tedorigawa001/TokenReductionTool
bdo --version    # should be 0.23.1+
```

## Compilation error during installation

```bash
rustup update stable
rustup default stable
cargo clean
cargo build --release
cargo install --path . --force
```

Minimum required Rust version: 1.70+.

## OpenCode not using Bushido

```bash
bdo init --global --opencode
# restart OpenCode
bdo init --show    # should show "OpenCode: plugin installed"
```

## Run the diagnostic script

From the Bushido repository root:

```bash
bash scripts/check-installation.sh
```

Checks:
- Bushido installed and in PATH
- Available features
- Claude Code integration
- Hook status

## Still stuck?

Open an issue: https://github.com/tedorigawa001/TokenReductionTool/issues
