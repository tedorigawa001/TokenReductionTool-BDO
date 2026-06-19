---
title: Supported Agents
description: How to integrate Bushido with Claude Code, Cursor, Copilot, Cline, Windsurf, Codex, OpenCode, Hermes, Kilo Code, and Antigravity
sidebar:
  order: 3
---

# Supported Agents

Bushido supports all major AI coding agents across 3 integration tiers. Mistral Vibe support is planned.

## How it works

Each agent integration intercepts CLI commands before execution and rewrites them to their Bushido equivalent. The agent runs `bdo cargo test` instead of `cargo test`, sees filtered output, and uses up to 90% fewer tokens — without any change to your workflow.

All rewrite logic lives in the Bushido binary (`bdo rewrite`). Agent hooks are thin delegates that parse the agent-specific JSON format and call `bdo rewrite` for the actual decision.

```
Agent runs "cargo test"
  -> Hook intercepts (PreToolUse / plugin event)
  -> Calls bdo rewrite "cargo test"
  -> Returns "bdo cargo test"
  -> Agent executes filtered command
  -> LLM sees 90% fewer tokens
```

## Supported agents

| Agent | Integration tier | Can rewrite transparently? |
|-------|-----------------|---------------------------|
| Claude Code | Shell hook (`PreToolUse`) | Yes |
| VS Code Copilot Chat | Shell hook (`PreToolUse`) | Yes |
| GitHub Copilot CLI | Shell hook (`preToolUse` `modifiedArgs`) | Yes |
| Cursor | Shell hook (`preToolUse`) | Yes |
| Gemini CLI | Rust binary (`BeforeTool`) | Yes |
| OpenCode | TypeScript plugin (`tool.execute.before`) | Yes |
| OpenClaw | TypeScript plugin (`before_tool_call`) | Yes |
| Pi | TypeScript extension (`tool_call` event) | Yes |
| Hermes | Python plugin (`terminal` command mutation) | Yes |
| Cline / Roo Code | Rules file (prompt-level) | N/A |
| Windsurf | Rules file (prompt-level) | N/A |
| Codex CLI | AGENTS.md instructions | N/A |
| Kilo Code | Rules file (prompt-level) | N/A |
| Google Antigravity | Rules file (prompt-level) | N/A |
| Mistral Vibe | Planned ([#800](https://github.com/tedorigawa001/TokenReductionTool/issues/800)) | Pending upstream |

## Installation by agent

### Claude Code

```bash
bdo init --global    # installs hook + patches settings.json
```

Restart Claude Code. Verify:

```bash
bdo init --show    # shows hook status
```

### Cursor

```bash
bdo init --global --agent cursor
```

Restart Cursor. The hook uses `preToolUse` with Cursor's `updated_input` format.

### GitHub Copilot (VS Code Chat + CLI)

```bash
bdo init --copilot            # project-scoped (.github/hooks/)
bdo init --global --copilot   # user-scoped (~/.copilot/hooks/, respects $COPILOT_HOME)
```

Project-scoped writes `.github/hooks/bdo-rewrite.json` (both hosts get transparent rewrite — VS Code Chat via `updatedInput`, Copilot CLI via `modifiedArgs`) plus the Bushido block in `.github/copilot-instructions.md`. User-scoped writes the same hook config to `~/.copilot/hooks/bdo-rewrite.json` and the Bushido block to `~/.copilot/copilot-instructions.md` (both respect `$COPILOT_HOME` if set).

Uninstall:

```bash
bdo init --uninstall --copilot
bdo init --uninstall --global --copilot
```

Removes only Bushido's hook file (and, for project, the Bushido block in `copilot-instructions.md`). Other files in `.github/hooks/` or `~/.copilot/hooks/` and your own instruction content are untouched.

### Gemini CLI

```bash
bdo init --global --gemini
```

### OpenCode

```bash
bdo init --global --opencode
```

Creates `~/.config/opencode/plugins/rtk.ts`. Uses the `tool.execute.before` hook.

### Pi

```bash
# Project-local (default)
bdo init --agent pi

# Global — all projects
bdo init --agent pi --global
```

Creates `.pi/extensions/rtk.ts` (local) or `~/.pi/agent/extensions/rtk.ts` (global). Pi auto-discovers extensions from both paths on startup.

Uninstall:

```bash
bdo init --uninstall --agent pi
bdo init --uninstall --agent pi --global
```

Removes only the installed Pi extension file.

### OpenClaw

```bash
openclaw plugins install ./openclaw
```

Plugin in the `openclaw/` directory. Uses the `before_tool_call` hook, delegates to `bdo rewrite`.

### Hermes

```bash
bdo init --agent hermes
```

Creates `~/.hermes/plugins/bdo-rewrite/` and enables it through `plugins.enabled` in the Hermes config. Hermes loads Python plugins, so the plugin entrypoint is Python, but it is only a thin adapter. It mutates the Hermes `terminal` tool `command` before execution and delegates all rewrite decisions to Rust through `bdo rewrite`. The repository source and tests for that adapter live in `hooks/hermes/`; only installed runtime files use the `~/.hermes/plugins/bdo-rewrite/` path.

The plugin fails open. If `bdo` is missing at load time, the hook is not registered. If `bdo rewrite` errors, the tool is not `terminal`, the payload has no string `command`, or the plugin raises an exception, Hermes runs the original command unchanged. The same `bdo rewrite` limitations apply: already-prefixed `bdo` commands, compound shell commands, heredocs, and commands without filters are not rewritten.

### Cline / Roo Code

```bash
bdo init --agent cline    # creates .clinerules in current project
```

Cline reads `.clinerules` as custom instructions. Bushido adds guidance telling Cline to prefer `bdo <cmd>` over raw commands.

### Windsurf

```bash
bdo init --global --agent windsurf    # creates .windsurfrules in current project
```

### Codex CLI

```bash
bdo init --codex           # project-scoped (AGENTS.md)
bdo init --global --codex  # user-global (~/.codex/AGENTS.md)
```

### Kilo Code

```bash
bdo init --agent kilocode    # creates .kilocode/rules/rtk-rules.md in current project
```

Kilo Code reads `.kilocode/rules/` as custom instructions. Bushido adds guidance telling Kilo Code to prefer `bdo <cmd>` over raw commands.

### Google Antigravity

```bash
bdo init --agent antigravity    # creates .agents/rules/antigravity-rtk-rules.md in current project
```

Antigravity reads `.agents/rules/` as custom instructions. Bushido adds guidance telling Antigravity to prefer `bdo <cmd>` over raw commands.

### Mistral Vibe (planned)

Support is blocked on upstream `BeforeToolCallback` ([mistral-vibe#531](https://github.com/mistralai/mistral-vibe/issues/531)). Tracked in [#800](https://github.com/tedorigawa001/TokenReductionTool/issues/800).

## Integration tiers explained

| Tier | Mechanism | How rewrites work |
|------|-----------|------------------|
| **Full hook** | Shell script or Rust binary, intercepts via agent API | Transparent — agent never sees the raw command |
| **Plugin** | TypeScript, JavaScript, or Python in agent's plugin system | Transparent, in-place mutation when the agent allows it |
| **Rules file** | Prompt-level instructions | Guidance only — agent is told to prefer `bdo <cmd>` |

Rules file integrations (Cline, Windsurf, Codex, Kilo Code, Antigravity) rely on the model following instructions. Full hook integrations (Claude Code, Cursor, Gemini) are guaranteed — the command is rewritten before the agent sees it. Plugin integrations (OpenCode, Pi) use in-place mutation via the agent's TypeScript extension API.

## Windows support

The shell hook (`bdo-rewrite.sh`) requires a Unix shell. On native Windows:

- `bdo init -g` automatically falls back to **CLAUDE.md injection mode** (prompt-level instructions)
- Filters work normally (`bdo cargo test`, `bdo git status`)
- Auto-rewrite does not work — the AI assistant is instructed to use Bushido but commands are not intercepted

For full hook support on Windows, use [WSL](https://learn.microsoft.com/en-us/windows/wsl/install). Inside WSL, all agents with shell hook integration (Claude Code, Cursor, Gemini) work identically to Linux.

## Graceful degradation

Hooks never block command execution. If Bushido is missing, the hook exits cleanly and the raw command runs unchanged:

- Bushido binary not found: warning to stderr, exit 0
- Invalid JSON input: pass through unchanged
- Bushido version too old: warning to stderr, exit 0
- Filter logic error: fallback to raw command output

## Override: disable Bushido for one command

```bash
BDO_DISABLED=1 git status    # runs raw git status, no rewrite
```

Or exclude commands permanently in `~/.config/bdo/config.toml`:

```toml
[hooks]
exclude_commands = ["git rebase", "git cherry-pick"]
```
