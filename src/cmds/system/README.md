# System and Generic Utilities

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `read.rs` uses `core/filter` for language-aware code stripping (FilterLevel: none/minimal/aggressive)
- `grep_cmd.rs` reads `core/config` for `limits.grep_max_results` and `limits.grep_max_per_file`. Format-altering flags (`-c`, `-l`, `-L`, `-o`, `-Z`) bypass Bushido filtering and run raw.
- `local_llm.rs` (`bdo smart`) uses `core/filter` for heuristic file summarization
- `format_cmd.rs` is a cross-ecosystem dispatcher: auto-detects and routes to `prettier_cmd` or `ruff_cmd` (black is handled inline, not as a separate module)
- `stale` / `review` use a shared bounded reader for content and `.bdostaleignore`: only link-free regular UTF-8 files up to 1 MiB are read. Oversized files are skipped in full (not partially scanned); content-scan skip counts are reported. Unsafe or unreadable ignore files act as an empty ignore list. Reads are anchored to the repository root, including subdirectory invocations.

## Cross-command

- `format_cmd` routes to `cmds/js/prettier_cmd` and `cmds/python/ruff_cmd`
