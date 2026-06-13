---
title: Discover and Session
description: Find missed savings opportunities with bdo discover, and track Bushido adoption with bdo session
sidebar:
  order: 2
---

# Discover and Session

## bdo discover — find missed savings

`bdo discover` analyzes your Claude Code command history to identify commands that ran without Bushido filtering and calculates how many tokens you lost.

```bash
bdo discover                    # analyze current project history
bdo discover --all              # all projects
bdo discover --all --since 7    # last 7 days, all projects
```

**Example output:**

```
Missed savings analysis (last 7 days)
────────────────────────────────────
Command              Count   Est. lost
cargo test              12     ~48,000 tokens
git log                  8     ~12,000 tokens
pnpm list                3      ~6,000 tokens
────────────────────────────────────
Total missed:           23     ~66,000 tokens

Run `bdo init --global` to capture these automatically.
```

If commands appear in the missed list after installing Bushido, it usually means the hook isn't active for that agent. See [Troubleshooting](../resources/troubleshooting.md) — "Agent not using Bushido".

## bdo session — adoption tracking

`bdo session` shows Bushido adoption across recent Claude Code sessions: how many shell commands ran through Bushido vs. raw.

```bash
bdo session
```

**Example output:**

```
Recent sessions (last 10)
─────────────────────────────────────────────────────
Session                         Total   Bushido   Coverage
2026-04-06 14:32  (45 cmds)       45    43      95.6%
2026-04-05 09:14  (38 cmds)       38    38     100.0%
2026-04-04 16:50  (52 cmds)       52    49      94.2%
─────────────────────────────────────────────────────
Average coverage: 96.6%
```

Low coverage on a session usually means Bushido was disabled (`BDO_DISABLED=1`) or the hook wasn't active for a specific subagent.
