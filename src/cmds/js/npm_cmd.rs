//! Filters npm output and auto-injects the "run" subcommand when appropriate.

use super::node_test_filter;
use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

/// Commands and aliases npm runs itself, so "run" is NOT injected before them.
/// From npm 11.19.1's `lib/utils/cmd-list.js`; `tests/npm_command_identity.rs`
/// holds the same list. The `run` aliases with no script argument only list
/// scripts, and injecting another `run` would execute a script named after them.
const NPM_SUBCOMMANDS: &[&str] = &[
    // Commands.
    "access", "adduser", "approve-scripts", "audit", "bugs", "cache", "ci", "completion", "config",
    "dedupe", "deny-scripts", "deprecate", "diff", "dist-tag", "docs", "doctor", "edit", "exec",
    "explain", "explore", "find-dupes", "fund", "get", "help", "help-search", "init", "install",
    "install-ci-test", "install-scripts", "install-test", "link", "ll", "login", "logout", "ls",
    "org", "outdated", "owner", "pack", "ping", "pkg", "prefix", "profile", "prune", "publish",
    "query", "rebuild", "repo", "restart", "root", "run", "sbom", "search", "set", "shrinkwrap",
    "stage", "star", "stars", "start", "stop", "team", "test", "token", "trust", "undeprecate",
    "uninstall", "unpublish", "unstar", "update", "version", "view", "whoami",
    // Aliases.
    "author", "home", "issues", "info", "show", "find", "add", "unlink", "remove", "rm", "r", "un",
    "rb", "list", "ln", "create", "i", "it", "cit", "u", "up", "c", "s", "se", "tst", "t", "ddp",
    "v", "run-script", "clean-install", "clean-install-test", "x", "why", "la", "verison", "ic",
    "innit", "in", "ins", "inst", "insta", "instal", "isnt", "isnta", "isntal", "isntall",
    "install-clean", "isntall-clean", "hlep", "dist-tags", "upgrade", "udpate", "rum", "sit", "urn",
    "ogr", "add-user",
    // Removed from npm, kept so they still reach npm rather than a script.
    "bin",
];

pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    // Determine if this is "npm run <script>" or another npm subcommand (install, list, etc.)
    // Only inject "run" when args look like a script name, not a known npm subcommand.
    let first_arg = args.first().map(|s| s.as_str());
    let is_run_explicit = first_arg == Some("run");
    let is_npm_subcommand = first_arg
        .map(|a| NPM_SUBCOMMANDS.contains(&a) || a.starts_with('-'))
        .unwrap_or(false);

    let mut effective_args: Vec<String> = Vec::with_capacity(args.len() + 1);
    if is_run_explicit || is_npm_subcommand {
        effective_args.extend_from_slice(args);
    } else {
        // "bdo npm build" → "npm run build" (assume script name)
        effective_args.push("run".to_string());
        effective_args.extend_from_slice(args);
    }

    run_filtered("npm", &effective_args, verbose, skip_env)
}

/// Run an npx tool through the same filtered pipeline as `npm`.
///
/// Used for unrouted tools in the `Commands::Npx` fallback so that
/// `rtk npx cowsay hello` dispatches to `npx`, not `npm`. Honors `--skip-env`
/// the same way `run` does.
pub fn exec(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    run_filtered("npx", args, verbose, skip_env)
}

/// Shared command-execution path for `run` (npm) and `exec` (npx).
///
/// Builds the resolved command, appends args, applies `SKIP_ENV_VALIDATION`,
/// emits the verbose log line, and routes through `runner::run_filtered` with
/// the npm output filter.
fn run_filtered(name: &str, args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    let mut cmd = resolved_command(name);
    for arg in args {
        cmd.arg(arg);
    }

    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }

    let args_display = args.join(" ");
    if verbose > 0 {
        eprintln!("Running: {} {}", name, args_display);
    }

    runner::run_filtered_with_exit(
        cmd,
        name,
        &args_display,
        filter_npm_output_for_run,
        runner::RunOptions::default(),
    )
}

/// [`filter_npm_output`], then, for a script that runs `node --test`, its
/// passing tests hidden (see [`node_test_filter::compact`]).
fn filter_npm_output_for_run(output: &str, exit_code: i32) -> String {
    let filtered = filter_npm_output(output);
    node_test_filter::compact(&filtered, exit_code).unwrap_or(filtered)
}

/// Filter npm run output - strip boilerplate, progress bars, npm WARN
fn filter_npm_output(output: &str) -> String {
    let mut result = Vec::new();

    for line in output.lines() {
        // Skip npm boilerplate
        if line.starts_with('>') && line.contains('@') {
            continue;
        }
        // Skip npm lifecycle scripts
        if line.trim_start().starts_with("npm WARN") {
            continue;
        }
        if line.trim_start().starts_with("npm notice") {
            continue;
        }
        // Skip progress indicators
        if line.contains("⸩") || line.contains("⸨") || line.contains("...") && line.len() < 10 {
            continue;
        }
        // Skip empty lines
        if line.trim().is_empty() {
            continue;
        }

        result.push(line.to_string());
    }

    if result.is_empty() {
        "ok".to_string()
    } else {
        result.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_npm_output() {
        let output = r#"
> project@1.0.0 build
> next build

npm WARN deprecated inflight@1.0.6: This module is not supported
npm notice

   Creating an optimized production build...
   ✓ Build completed
"#;
        let result = filter_npm_output(output);
        assert!(!result.contains("npm WARN"));
        assert!(!result.contains("npm notice"));
        assert!(!result.contains("> project@"));
        assert!(result.contains("Build completed"));
    }

    #[test]
    fn test_npm_subcommand_routing() {
        // Uses the shared NPM_SUBCOMMANDS constant — no drift between prod and test
        fn needs_run_injection(args: &[&str]) -> bool {
            let first = args.first().copied();
            let is_run_explicit = first == Some("run");
            let is_subcommand = first
                .map(|a| NPM_SUBCOMMANDS.contains(&a) || a.starts_with('-'))
                .unwrap_or(false);
            !is_run_explicit && !is_subcommand
        }

        // Known subcommands should NOT get "run" injected
        for subcmd in NPM_SUBCOMMANDS {
            assert!(
                !needs_run_injection(&[subcmd]),
                "'npm {}' should NOT inject 'run'",
                subcmd
            );
        }

        // Script names SHOULD get "run" injected
        for script in &["build", "dev", "lint", "typecheck", "deploy"] {
            assert!(
                needs_run_injection(&[script]),
                "'npm {}' SHOULD inject 'run'",
                script
            );
        }

        // Flags should NOT get "run" injected
        assert!(!needs_run_injection(&["--version"]));
        assert!(!needs_run_injection(&["-h"]));

        // Explicit "run" should NOT inject another "run"
        assert!(!needs_run_injection(&["run", "build"]));
    }

    #[test]
    fn test_npm_test_hides_passing_node_test_lines() {
        let output = "\n> app@1.0.0 test\n> node --test\n\n\
✔ adds (0.3ms)\n✔ subtracts (0.2ms)\nℹ tests 2\nℹ pass 2\nℹ fail 0\n";
        let result = filter_npm_output_for_run(output, 0);
        assert!(!result.contains("✔ adds"), "{result}");
        assert!(!result.contains("> app@"), "{result}");
        assert!(result.contains("ℹ tests 2"), "{result}");
        assert!(result.contains("2 passing test lines hidden"), "{result}");
    }

    #[test]
    fn test_npm_test_failure_keeps_node_test_details() {
        let output = "✔ adds (0.3ms)\n✖ breaks (0.4ms)\nℹ tests 2\nℹ fail 1\n\
\n✖ failing tests:\n\ntest at a.test.mjs:3:1\n✖ breaks (0.4ms)\n  AssertionError: 1 == 2\n";
        let result = filter_npm_output_for_run(output, 1);
        assert!(!result.contains("✔ adds"), "{result}");
        assert!(result.contains("✖ breaks"), "{result}");
        assert!(result.contains("AssertionError: 1 == 2"), "{result}");
    }

    #[test]
    fn test_filter_npm_output_empty() {
        let output = "\n\n\n";
        let result = filter_npm_output(output);
        assert_eq!(result, "ok");
    }
}
