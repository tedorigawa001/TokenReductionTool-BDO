# Bushido Installation Guide - For AI Coding Assistants

Bushido is the `bdo` CLI (crate `bushido`), a fork of [rtk (Rust Token Killer)](https://github.com/rtk-ai/rtk).

## Pre-Installation Check (REQUIRED)

**AI assistants should ALWAYS verify if Bushido is already installed before attempting installation.**

```bash
# Check if Bushido is installed
bdo --version

# Verify the token-optimizer commands are available
bdo gain    # Should show token savings stats, NOT "command not found"

# Check installation path
which bdo
```

If `bdo gain` works, Bushido is installed. **DO NOT reinstall**. Skip to "Project Initialization".

## Installation (only if Bushido is not available)

### Quick Install (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/tedorigawa001/TokenReductionTool/master/install.sh | sh
```

After installation, **verify you have the correct bdo**:
```bash
bdo gain  # Must show token savings stats (not "command not found")
```

### Alternative: Manual Installation

```bash
# From the Bushido fork repository
cargo install --git https://github.com/tedorigawa001/TokenReductionTool

# OR from crates.io (if published; crate name is "bushido", binary is "bdo")
cargo install bushido

# Verify after installation
bdo gain  # Should show token savings, not "command not found"
```

## Project Initialization

### Which mode to choose?

```
  Do you want Bushido active across ALL Claude Code projects?
  │
  ├─ YES → bdo init -g              (recommended)
  │         Hook + Bushido.md (~10 tokens in context)
  │         Commands auto-rewritten transparently
  │
  ├─ YES, minimal → bdo init -g --hook-only
  │         Hook only, nothing added to CLAUDE.md
  │         Zero tokens in context
  │
  └─ NO, single project → bdo init
            Local CLAUDE.md only (137 lines)
            No hook, no global effect
```

### Recommended: Global Hook-First Setup

**Best for: All projects, automatic Bushido usage**

```bash
bdo init -g
# → Installs hook to ~/.claude/hooks/rtk-rewrite.sh
# → Creates ~/.claude/Bushido.md (10 lines, meta commands only)
# → Adds @Bushido.md reference to ~/.claude/CLAUDE.md
# → Prompts: "Patch settings.json? [y/N]"
# → If yes: patches + creates backup (~/.claude/settings.json.bak)

# Automated alternatives:
bdo init -g --auto-patch    # Patch without prompting
bdo init -g --no-patch      # Print manual instructions instead

# Verify installation
bdo init --show  # Check hook is installed and executable
```

**Token savings**: ~99.5% reduction (2000 tokens → 10 tokens in context)

**What is settings.json?**
Claude Code's hook registry. Bushido adds a PreToolUse hook that rewrites commands transparently. Without this, Claude won't invoke the hook automatically.

```
  Claude Code          settings.json        rtk-rewrite.sh        Bushido binary
       │                    │                     │                    │
       │  "git status"      │                     │                    │
       │ ──────────────────►│                     │                    │
       │                    │  PreToolUse trigger  │                    │
       │                    │ ───────────────────►│                    │
       │                    │                     │  rewrite command   │
       │                    │                     │  → bdo git status  │
       │                    │◄────────────────────│                    │
       │                    │  updated command     │                    │
       │                    │                                          │
       │  execute: bdo git status                                      │
       │ ─────────────────────────────────────────────────────────────►│
       │                                                               │  filter
       │  "3 modified, 1 untracked ✓"                                  │
       │◄──────────────────────────────────────────────────────────────│
```

**Backup Safety**:
Bushido backs up existing settings.json before changes. Restore if needed:
```bash
cp ~/.claude/settings.json.bak ~/.claude/settings.json
```

### Alternative: Local Project Setup

**Best for: Single project without hook**

```bash
cd /path/to/your/project
bdo init  # Creates ./CLAUDE.md with full Bushido instructions (137 lines)
```

**Token savings**: Instructions loaded only for this project

### Upgrading from Previous Version

#### From old 137-line CLAUDE.md injection (pre-0.22)

```bash
bdo init -g  # Automatically migrates to hook-first mode
# → Removes old 137-line block
# → Installs hook + Bushido.md
# → Adds @Bushido.md reference
```

#### From old hook with inline logic (pre-0.24) — ⚠️ Breaking Change

Bushido 0.24.0 replaced the inline command-detection hook (~200 lines) with a **thin delegator** that calls `bdo rewrite`. The binary now contains the rewrite logic, so adding new commands no longer requires a hook update.

The old hook still works but won't benefit from new rules added in future releases.

```bash
# Upgrade hook to thin delegator
bdo init --global

# Verify the new hook is active
bdo init --show
# Should show: ✅ Hook: ... (thin delegator, up to date)
```

## Common User Flows

### First-Time User (Recommended)
```bash
# 1. Install Bushido
cargo install --git https://github.com/tedorigawa001/TokenReductionTool
bdo gain  # Verify (must show token stats)

# 2. Setup with prompts
bdo init -g
# → Answer 'y' when prompted to patch settings.json
# → Creates backup automatically

# 3. Restart Claude Code
# 4. Test: git status (should use bdo)
```

### CI/CD or Automation
```bash
# Non-interactive setup (no prompts)
bdo init -g --auto-patch

# Verify in scripts
bdo init --show | grep "Hook:"
```

### Conservative User (Manual Control)
```bash
# Get manual instructions without patching
bdo init -g --no-patch

# Review printed JSON snippet
# Manually edit ~/.claude/settings.json
# Restart Claude Code
```

### Temporary Trial
```bash
# Install hook
bdo init -g --auto-patch

# Later: remove everything
bdo init -g --uninstall

# Restore backup if needed
cp ~/.claude/settings.json.bak ~/.claude/settings.json
```

## Installation Verification

```bash
# Basic test
bdo ls .

# Test with git
bdo git status

# Test with pnpm
bdo pnpm list

# Test with Vitest
bdo vitest
```

## Uninstalling

### Complete Removal (Global Installations Only)

```bash
# Complete removal (global installations only)
bdo init -g --uninstall

# What gets removed:
#   - Hook: ~/.claude/hooks/rtk-rewrite.sh
#   - Context: ~/.claude/Bushido.md
#   - Reference: @Bushido.md line from ~/.claude/CLAUDE.md
#   - Registration: Bushido hook entry from settings.json

# Restart Claude Code after uninstall
```

**For Local Projects**: Manually remove Bushido block from `./CLAUDE.md`

### Binary Removal

```bash
# If installed via cargo
cargo uninstall bdo

# If installed via package manager
brew uninstall bdo          # macOS Homebrew
sudo apt remove bdo         # Debian/Ubuntu
sudo dnf remove bdo         # Fedora/RHEL
```

### Restore from Backup (if needed)

```bash
cp ~/.claude/settings.json.bak ~/.claude/settings.json
```

## Essential Commands

### Files
```bash
bdo ls .              # Compact tree view
bdo read file.rs      # Optimized reading
bdo grep "pattern" .  # Grouped search results
```

### Git
```bash
bdo git status        # Compact status
bdo git log -n 10     # Condensed logs
bdo git diff          # Optimized diff
bdo git add .         # → "ok ✓"
bdo git commit -m "msg"  # → "ok ✓ abc1234"
bdo git push          # → "ok ✓ main"
```

### Pnpm (fork only)
```bash
bdo pnpm list     # Dependency tree (-70% tokens)
bdo pnpm outdated # Available updates (-80-90%)
bdo pnpm install  # Silent installation
```

### Tests
```bash
bdo cargo test      # Filtered Cargo test output (-90%)
bdo go test         # Filtered Go tests (NDJSON, -90%)
bdo jest            # Filtered Jest output (-99.6%)
bdo vitest          # Filtered Vitest output (-99.6%)
bdo playwright test # Filtered Playwright output (-94%)
bdo pytest          # Filtered Python tests (-90%)
bdo rake test       # Filtered Ruby tests (-90%)
bdo rspec           # Filtered RSpec tests (-60%)
bdo test <cmd>      # Generic test wrapper - failures only (-90%)
```

### Statistics
```bash
bdo gain              # Token savings
bdo gain --graph      # With ASCII graph
bdo gain --history    # With command history
```

## Validated Token Savings

### Production T3 Stack Project
| Operation | Standard | Bushido | Reduction |
|-----------|----------|-----|-----------|
| `vitest` | 102,199 chars | 377 chars | **-99.6%** |
| `git status` | 529 chars | 217 chars | **-59%** |
| `pnpm list` | ~8,000 tokens | ~2,400 | **-70%** |
| `pnpm outdated` | ~12,000 tokens | ~1,200-2,400 | **-80-90%** |

### Typical Claude Code Session (30 min)
- **Without Bushido**: ~150,000 tokens
- **With Bushido**: ~45,000 tokens
- **Savings**: **70% reduction**

## Troubleshooting

### Bushido command not found after installation
```bash
# Check PATH
echo $PATH | grep -o '[^:]*\.cargo[^:]*'

# Add to PATH if needed (~/.bashrc or ~/.zshrc)
export PATH="$HOME/.cargo/bin:$PATH"

# Reload shell
source ~/.bashrc  # or source ~/.zshrc
```

### Bushido command not available (e.g., vitest)
```bash
# Check branch
cd /path/to/rtk
git branch

# Switch to feat/vitest-support if needed
git checkout feat/vitest-support

# Reinstall
cargo install --path . --force
```

### Compilation error
```bash
# Update Rust
rustup update stable

# Clean and recompile
cargo clean
cargo build --release
cargo install --path . --force
```

## Support and Contributing

- **Website**: https://github.com/tedorigawa001/TokenReductionTool
- **Contact**: https://github.com/tedorigawa001/TokenReductionTool/issues
- **Troubleshooting**: See [docs/guide/resources/troubleshooting.md](docs/guide/resources/troubleshooting.md) for common issues
- **GitHub issues**: https://github.com/tedorigawa001/TokenReductionTool/issues
- **Pull Requests**: https://github.com/tedorigawa001/TokenReductionTool/pulls

## AI Assistant Checklist

Before each session:

- [ ] Verify Bushido is installed: `bdo --version`
- [ ] If not installed → follow "Install from fork"
- [ ] If project not initialized → `bdo init`
- [ ] Use `bdo` for ALL git/pnpm/test/vitest commands
- [ ] Check savings: `bdo gain`

**Golden Rule**: AI coding assistants should ALWAYS use `bdo` as a proxy for shell commands that generate verbose output (git, pnpm, npm, cargo test, vitest, docker, kubectl).
