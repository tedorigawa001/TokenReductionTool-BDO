# JavaScript / TypeScript / Node

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `utils::package_manager_exec()` auto-detects pnpm/yarn/npm -- JS modules should use this instead of hardcoding a package manager
- `lint_cmd.rs` is a cross-ecosystem router: detects Python projects and delegates to `mypy_cmd` or `ruff_cmd`
- `vitest_cmd.rs` uses the `parser/` module for structured output parsing
- `playwright_cmd.rs` uses the `parser/` module for test result extraction
- `node_test_filter.rs` hides passing lines of Node's built-in test runner (`node --test`, spec reporter); `npm_cmd.rs` applies it to every npm script's output. It changes nothing unless the output has the runner's `ℹ tests N` summary, and shows a failed run whole if no test failure explains it
- `vitest_coverage_filter.rs` is specific to `npm run test:coverage`: on a successful, recognized Vitest coverage run it hides only all-passing file rows, leaving the coverage table, test summary, and diagnostics intact. Failed or unrecognized output stays intact

## Cross-command

- `lint_cmd` routes to `cmds/python/mypy_cmd` and `cmds/python/ruff_cmd` for Python projects
- `prettier_cmd` is also called by `cmds/system/format_cmd` as a format dispatcher target
