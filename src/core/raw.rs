//! Raw mode: `bdo --raw <cmd>` / `BDO_RAW=1` runs as if bdo weren't there.
//!
//! One switch replaces the per-command escape hatches (`bdo read -l none`,
//! `BDO_NO_TOML=1`, `bdo proxy`), which all still work.
//!
//! Raw mode is decided from argv *before* clap parses, because the faithful
//! meaning differs by command, and for most commands it is not "bdo with the
//! filter switched off": several modules inject machine-readable flags
//! (`--format json`, `--message-format`, …) into the command they run, so
//! skipping only the filter would print JSON, not the command's real output.
//! Instead:
//!
//! - a command with a same-named program on PATH (`git`, `ls`, `cargo`, …)
//!   runs that program with the arguments untouched — exactly what
//!   `bdo proxy` does;
//! - the wrappers `err` / `summary` / `test` run the wrapped command itself;
//! - `read` shows the full file (what `-l none` did);
//! - bdo-only commands (`gain`, `map`, …) have no unfiltered form and run
//!   as usual.
//!
//! A command routed `Normal` still reaches its module. A module that runs
//! through the core runner gets passthrough mode there automatically. A module
//! that executes on its own (`exec_capture`) must check [`is_active`] and call
//! `runner::run_raw` itself — it would otherwise filter regardless. That
//! applies to the commands reachable here: `lint`, `format`, `vitest`/`jest`,
//! `prisma`, `playwright`, and `pip` in uv-only environments.

use std::ffi::{OsStr, OsString};
use std::sync::atomic::{AtomicBool, Ordering};

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Turn raw mode on for the rest of this process.
pub fn activate() {
    ACTIVE.store(true, Ordering::Relaxed);
}

/// Whether raw mode is on. Consulted by the core runner, `read`, and the
/// TOML fallback.
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// `BDO_RAW` is set to a truthy value (`1`, `true`, `yes`, `on`).
pub fn env_requests_raw() -> bool {
    std::env::var("BDO_RAW")
        .map(|v| is_truthy(&v))
        .unwrap_or(false)
}

fn is_truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Leading global flags stripped from argv.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Globals {
    /// `--raw` appeared among the leading flags.
    pub raw_flag: bool,
    /// `-v` / `-vv` / `--verbose` count.
    pub verbose: u8,
    /// Index of the first argument after the leading global flags.
    pub rest_start: usize,
}

/// Scan the global flags that may precede the subcommand. `--raw` must come
/// before the subcommand: after it, many subcommands treat every remaining
/// argument as the wrapped command's own (`bdo git log --raw` passes `--raw`
/// to git). `BDO_RAW=1` has no such restriction.
pub fn split_leading_globals(args: &[OsString]) -> Globals {
    let mut g = Globals::default();
    for (i, arg) in args.iter().enumerate() {
        match arg.to_str() {
            Some("--raw") => g.raw_flag = true,
            Some("--verbose") => g.verbose = g.verbose.saturating_add(1),
            Some("--ultra-compact" | "--skip-env") => {}
            Some(s) if s.len() > 1 && s.starts_with('-') && s[1..].chars().all(|c| c == 'v') => {
                g.verbose = g.verbose.saturating_add((s.len() - 1) as u8);
            }
            _ => {
                g.rest_start = i;
                return g;
            }
        }
    }
    g.rest_start = args.len();
    g
}

/// bdo commands named for what they do rather than for the program they run
/// (`lint` → eslint/ruff/…, `format` → prettier/black/…). A same-named program
/// on PATH, if one exists, is unrelated — Windows ships `format.com`, the disk
/// formatter — so these must never be routed to PATH. Their modules honor raw
/// mode themselves.
const BDO_NAMED_COMMANDS: &[&str] = &["lint", "format"];

/// Where a raw-mode invocation goes.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// Exec this program with these arguments, unfiltered.
    Native {
        program: OsString,
        args: Vec<OsString>,
    },
    /// Let clap parse and dispatch as usual; raw mode stays active for the
    /// command to consult.
    Normal,
}

/// Decide how raw mode handles `rest` (argv after the leading globals).
///
/// `bdo_only` lists subcommands that exist only in bdo; `on_path` reports
/// whether a program by that name can be executed. Both are injected so the
/// routing is testable without touching PATH.
pub fn route(rest: &[OsString], bdo_only: &[&str], on_path: impl Fn(&OsStr) -> bool) -> Route {
    let Some(sub) = rest.first() else {
        return Route::Normal;
    };
    let wrapped = || match rest.get(1..) {
        Some([program, args @ ..]) => Route::Native {
            program: program.clone(),
            args: args.to_vec(),
        },
        _ => Route::Normal, // nothing wrapped: let clap report the usage error
    };
    match sub.to_str() {
        // Already unfiltered.
        Some("proxy" | "run") => Route::Normal,
        Some("err" | "summary") => wrapped(),
        // `test --changed` plans its own commands; only a literal wrapped
        // command has a raw form.
        Some("test") if !rest.iter().any(|a| a == "--changed") => wrapped(),
        Some("test") => Route::Normal,
        // No `read` program exists; `read` itself honors raw mode.
        Some("read") => Route::Normal,
        Some(s) if bdo_only.contains(&s) || s.starts_with('-') => Route::Normal,
        Some(s) if BDO_NAMED_COMMANDS.contains(&s) => Route::Normal,
        _ if on_path(sub) => Route::Native {
            program: sub.clone(),
            args: rest[1..].to_vec(),
        },
        _ => Route::Normal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    const BDO_ONLY: &[&str] = &["gain", "map", "read", "proxy", "run", "log"];

    fn exists(names: &'static [&'static str]) -> impl Fn(&OsStr) -> bool {
        move |p| names.iter().any(|n| p == *n)
    }

    #[test]
    fn test_truthy_values() {
        for v in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(is_truthy(v), "{v}");
        }
        for v in ["0", "false", "", "no", "off", "2"] {
            assert!(!is_truthy(v), "{v}");
        }
    }

    #[test]
    fn test_split_leading_globals() {
        let g = split_leading_globals(&os(&["--raw", "git", "log"]));
        assert_eq!((g.raw_flag, g.verbose, g.rest_start), (true, 0, 1));

        let g = split_leading_globals(&os(&["-vv", "--raw", "--skip-env", "ls"]));
        assert_eq!((g.raw_flag, g.verbose, g.rest_start), (true, 2, 3));

        // --raw after the subcommand belongs to the wrapped command.
        let g = split_leading_globals(&os(&["git", "log", "--raw"]));
        assert_eq!((g.raw_flag, g.rest_start), (false, 0));

        let g = split_leading_globals(&os(&["--raw"]));
        assert_eq!((g.raw_flag, g.rest_start), (true, 1));
    }

    #[test]
    fn test_route_native_program_keeps_args_untouched() {
        assert_eq!(
            route(
                &os(&["git", "log", "--oneline"]),
                BDO_ONLY,
                exists(&["git"])
            ),
            Route::Native {
                program: "git".into(),
                args: os(&["log", "--oneline"]),
            }
        );
    }

    #[test]
    fn test_route_unwraps_err_summary_and_test() {
        for w in ["err", "summary", "test"] {
            assert_eq!(
                route(&os(&[w, "cargo", "build"]), BDO_ONLY, exists(&[])),
                Route::Native {
                    program: "cargo".into(),
                    args: os(&["build"]),
                },
                "{w}"
            );
        }
        assert_eq!(route(&os(&["err"]), BDO_ONLY, exists(&[])), Route::Normal);
    }

    #[test]
    fn test_route_test_changed_stays_normal() {
        assert_eq!(
            route(&os(&["test", "--changed"]), BDO_ONLY, exists(&["test"])),
            Route::Normal
        );
    }

    #[test]
    fn test_route_bdo_only_and_read_stay_normal_even_if_on_path() {
        // macOS ships /usr/bin/log; `bdo log` is bdo's own filter and must not
        // be swapped for Apple's tool.
        for c in ["read", "gain", "map", "log", "proxy"] {
            assert_eq!(
                route(&os(&[c, "x"]), BDO_ONLY, exists(&["log", "read"])),
                Route::Normal,
                "{c}"
            );
        }
    }

    #[test]
    fn test_route_tool_without_same_named_program_stays_normal() {
        // `bdo lint` runs eslint; there is no `lint` program. The module
        // honors raw mode itself.
        assert_eq!(
            route(&os(&["lint"]), BDO_ONLY, exists(&["eslint"])),
            Route::Normal
        );
    }

    #[test]
    fn test_route_bdo_named_commands_never_go_to_path() {
        // Windows ships format.com, the disk formatter. `bdo --raw format`
        // must reach bdo's formatter wrapper, never that.
        for c in ["format", "lint"] {
            assert_eq!(
                route(&os(&[c, "."]), BDO_ONLY, exists(&["format", "lint"])),
                Route::Normal,
                "{c}"
            );
        }
    }
}
