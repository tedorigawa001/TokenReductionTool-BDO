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

### Double-clicking bdo.exe does nothing

**Symptom:** You double-click `bdo.exe`, a terminal flashes and closes instantly.

**Cause:** Bushido is a command-line tool. With no arguments, it prints usage and exits. The console window opens and closes before you can read anything.

**Fix:** Open a terminal first, then run Bushido from there:
- Press `Win+R`, type `cmd`, press Enter
- Or open PowerShell or Windows Terminal
- Then run: `bdo --version`

### Hook not working (no auto-rewrite)

**Symptom:** Commands are not rewritten after running `bdo init -g` on Windows.

**Cause:** The native hook requires `bdo.exe` to be available on `PATH`, and the IDE or agent must be restarted after registration.

**Fix:** Confirm `bdo --version` works in the same environment as the IDE, rerun `bdo init -g`, and restart the IDE or agent. WSL remains supported, but is not required for the native binary hook.

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
