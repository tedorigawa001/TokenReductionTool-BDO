//! `bdo --raw <cmd>` / `BDO_RAW=1`: run as if bdo weren't there.
//!
//! The contract these tests pin, end to end with the real binary: raw output
//! is the native command's output byte for byte, with its exit code; `read`
//! shows the whole file; and the switch never changes what the hook does.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn bdo(dir: &Path, args: &[&str], raw_env: bool) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_bdo"));
    c.current_dir(dir)
        .args(args)
        .env("BDO_DB_PATH", dir.join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1")
        .env_remove("BDO_RAW");
    if raw_env {
        c.env("BDO_RAW", "1");
    }
    c.output().expect("spawn bdo")
}

/// A file long enough that `bdo read`'s default view truncates it.
fn big_file(dir: &Path) -> usize {
    let lines = 2000;
    let body: String = (0..lines).map(|i| format!("fn f{i}() {{}}\n")).collect();
    fs::write(dir.join("big.rs"), body).unwrap();
    lines
}

fn line_count(o: &Output) -> usize {
    String::from_utf8_lossy(&o.stdout).lines().count()
}

#[test]
fn raw_read_shows_the_whole_file_by_flag_and_by_env() {
    let dir = tempfile::tempdir().unwrap();
    let total = big_file(dir.path());

    let reduced = bdo(dir.path(), &["read", "big.rs"], false);
    assert!(line_count(&reduced) < total, "default read should reduce");

    let by_flag = bdo(dir.path(), &["--raw", "read", "big.rs"], false);
    assert_eq!(line_count(&by_flag), total);

    let by_env = bdo(dir.path(), &["read", "big.rs"], true);
    assert_eq!(line_count(&by_env), total);
}

#[test]
fn reduced_read_hint_points_at_raw() {
    let dir = tempfile::tempdir().unwrap();
    big_file(dir.path());
    let out = bdo(dir.path(), &["read", "big.rs"], false);
    let hint = String::from_utf8_lossy(&out.stderr);
    assert!(hint.contains("bdo --raw read big.rs"), "{hint}");
}

#[cfg(unix)]
#[test]
fn raw_native_command_is_byte_identical_with_its_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..30 {
        fs::write(dir.path().join(format!("file{i:02}.txt")), "").unwrap();
    }
    let native = Command::new("ls")
        .current_dir(dir.path())
        .arg("-1")
        .output()
        .unwrap();
    let raw = bdo(dir.path(), &["--raw", "ls", "-1"], false);
    assert_eq!(raw.stdout, native.stdout, "raw ls must match native ls");
    assert_eq!(raw.status.code(), native.status.code());

    let native_fail = Command::new("ls")
        .arg("/nonexistent-bdo-raw-test")
        .output()
        .unwrap();
    let raw_fail = bdo(
        dir.path(),
        &["--raw", "ls", "/nonexistent-bdo-raw-test"],
        false,
    );
    assert_eq!(raw_fail.status.code(), native_fail.status.code());
}

#[cfg(unix)]
#[test]
fn raw_unwraps_err_and_test() {
    // The wrapper's own decoration ("[FAIL] Command failed …") disappears;
    // the wrapped command's output and status come through as-is.
    let dir = tempfile::tempdir().unwrap();
    for w in ["err", "test"] {
        let out = bdo(
            dir.path(),
            &["--raw", w, "sh", "-c", "echo hi; exit 3"],
            false,
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "hi\n",
            "bdo --raw {w}"
        );
        assert_eq!(out.status.code(), Some(3), "bdo --raw {w}");
    }
}

#[test]
fn raw_flag_on_bdo_only_command_explains_itself_but_env_stays_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let flagged = bdo(dir.path(), &["--raw", "gain"], false);
    assert!(
        String::from_utf8_lossy(&flagged.stderr).contains("--raw has no effect"),
        "explicit --raw on a bdo-only command should say so"
    );
    // BDO_RAW=1 is ambient — often set globally — so it must not add noise.
    let ambient = bdo(dir.path(), &["gain"], true);
    assert!(!String::from_utf8_lossy(&ambient.stderr).contains("no effect"));
}

#[test]
fn flag_only_invocation_is_a_usage_error_not_command_not_found() {
    // `bdo --raw` alone used to fall back to executing "--raw" and exit 127.
    let dir = tempfile::tempdir().unwrap();
    for args in [&["--raw"][..], &["-v"][..]] {
        let out = bdo(dir.path(), args, false);
        assert_eq!(out.status.code(), Some(2), "bdo {args:?}");
    }
}

#[test]
fn hook_rewrite_is_unchanged_under_bdo_raw() {
    // Setting BDO_RAW=1 globally must not break the hook: rewrites still
    // happen (the rewritten bdo command then runs raw) and nothing is
    // printed to stderr.
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let input = br#"{"tool_name":"Bash","tool_input":{"command":"git status"}}"#;
    let run = |raw_env: bool| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_bdo"));
        c.current_dir(dir.path())
            .args(["hook", "claude"])
            .env("BDO_DB_PATH", dir.path().join("track.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env_remove("BDO_RAW")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if raw_env {
            c.env("BDO_RAW", "1");
        }
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        child.wait_with_output().unwrap()
    };
    let normal = run(false);
    let raw = run(true);
    assert_eq!(raw.stdout, normal.stdout);
    assert!(
        raw.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&raw.stderr)
    );
}

/// Modules that execute their tool themselves (instead of through the core
/// runner) used to keep filtering under `--raw`: `bdo --raw lint ruff check .`
/// still injected `--output-format=json` and printed bdo's summary. Fake tools
/// on PATH echo the argv they receive, so each test sees exactly what bdo ran.
#[cfg(unix)]
mod modules_that_exec_on_their_own {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A dir with fake `ruff`, `black`, `npx` and `uv` that print their argv
    /// and exit 7. PATH holds nothing else, so real JS tools and `pip` are
    /// absent and bdo reaches its own modules (npx / uv fallbacks) rather than
    /// routing to PATH.
    fn fakebin(dir: &Path) -> std::path::PathBuf {
        let bin = dir.join("fakebin");
        fs::create_dir(&bin).unwrap();
        for tool in ["ruff", "black", "npx", "uv"] {
            let p = bin.join(tool);
            // `black` also prints a line its bdo filter compresses, so a
            // filtered run is distinguishable from a raw one.
            let extra = if tool == "black" {
                "echo 'would reformat: a.py'\n"
            } else {
                ""
            };
            // Also record argv to <tool>.argv: in normal mode bdo filters
            // stdout, so the file is the only way to see what it ran.
            let log = bin.join(format!("{tool}.argv"));
            fs::write(
                &p,
                format!(
                    "#!/bin/sh\necho \"$*\" > '{}'\necho \"{tool} ARGV:$*\"\n{extra}exit 7\n",
                    log.display()
                ),
            )
            .unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        bin
    }

    fn run(dir: &Path, bin: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bdo"))
            .current_dir(dir)
            .args(args)
            // Only the fakes: a system dir would leak real tools into the
            // test (GitHub's Ubuntu runners ship /usr/bin/pip, which raw
            // mode rightly execs directly). The fakes' `#!/bin/sh` is an
            // absolute path, so they need nothing else on PATH.
            .env("PATH", bin)
            .env("BDO_DB_PATH", dir.join("track.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env_remove("BDO_RAW")
            .output()
            .expect("spawn bdo")
    }

    fn argv_seen(bin: &Path, tool: &str) -> String {
        fs::read_to_string(bin.join(format!("{tool}.argv")))
            .unwrap_or_default()
            .trim_end()
            .to_string()
    }

    fn first_line(o: &Output) -> String {
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .to_string()
    }

    #[test]
    fn raw_lint_runs_the_linter_without_json_flags_and_unfiltered() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fakebin(dir.path());

        let normal = run(dir.path(), &bin, &["lint", "ruff", "check", "."]);
        assert_ne!(
            first_line(&normal),
            "ruff ARGV:check .",
            "normal mode should still filter"
        );

        let raw = run(dir.path(), &bin, &["--raw", "lint", "ruff", "check", "."]);
        assert_eq!(first_line(&raw), "ruff ARGV:check .");
        assert_eq!(raw.status.code(), Some(7));
    }

    #[test]
    fn raw_format_keeps_check_mode() {
        // --check is what keeps `bdo format` from rewriting files; raw mode
        // must not drop it along with bdo's output reduction.
        let dir = tempfile::tempdir().unwrap();
        let bin = fakebin(dir.path());
        let normal = run(dir.path(), &bin, &["format", "black"]);
        assert_ne!(
            String::from_utf8_lossy(&normal.stdout),
            "black ARGV:--check .\nwould reformat: a.py\n",
            "normal mode should still filter"
        );

        let raw = run(dir.path(), &bin, &["--raw", "format", "black"]);
        assert_eq!(
            String::from_utf8_lossy(&raw.stdout),
            "black ARGV:--check .\nwould reformat: a.py\n"
        );
        assert_eq!(raw.status.code(), Some(7));
    }

    #[test]
    fn raw_js_tools_keep_non_watch_and_drop_json_reporters() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fakebin(dir.path());
        for (args, expected) in [
            (
                &["--raw", "vitest"][..],
                "npx ARGV:--no-install -- vitest run",
            ),
            (
                &["--raw", "jest"][..],
                "npx ARGV:--no-install -- jest --no-watch",
            ),
            (
                &["--raw", "prisma", "generate"][..],
                "npx ARGV:prisma generate",
            ),
            (
                &["--raw", "playwright", "test", "--reporter=line"][..],
                "npx ARGV:--no-install -- playwright test --reporter=line",
            ),
        ] {
            let out = run(dir.path(), &bin, args);
            assert_eq!(first_line(&out), expected, "{args:?}");
            assert_eq!(out.status.code(), Some(7), "{args:?}");
        }
    }

    #[test]
    fn raw_pip_via_uv_drops_json_format() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fakebin(dir.path());
        let list = run(dir.path(), &bin, &["--raw", "pip", "list"]);
        assert_eq!(first_line(&list), "uv ARGV:pip list");
        let outdated = run(dir.path(), &bin, &["--raw", "pip", "outdated"]);
        assert_eq!(first_line(&outdated), "uv ARGV:pip list --outdated");
    }
    #[test]
    fn lint_output_format_option_handled_as_a_unit_in_both_modes() {
        // `check` used to be added only when no --output-format was given, and
        // a separate-token value was left behind: 0.45.7 ran `ruff text .`
        // for `bdo lint ruff check --output-format text .`.
        let dir = tempfile::tempdir().unwrap();
        let bin = fakebin(dir.path());
        let args = ["lint", "ruff", "check", "--output-format", "text", "."];

        let mut raw = vec!["--raw"];
        raw.extend(args);
        run(dir.path(), &bin, &raw);
        assert_eq!(argv_seen(&bin, "ruff"), "check --output-format text .");

        // Normal mode replaces the user's format, value included, with JSON.
        run(dir.path(), &bin, &args);
        assert_eq!(argv_seen(&bin, "ruff"), "check --output-format=json .");

        // No path given: the format's value is not mistaken for one.
        run(
            dir.path(),
            &bin,
            &["--raw", "lint", "ruff", "--output-format", "text"],
        );
        assert_eq!(argv_seen(&bin, "ruff"), "check --output-format text .");

        // eslint's -f: 0.45.7 passed both `-f json -f stylish`, and the
        // later one won, breaking bdo's JSON parse.
        run(
            dir.path(),
            &bin,
            &["lint", "eslint", "-f", "stylish", "src"],
        );
        assert_eq!(argv_seen(&bin, "npx"), "--no-install -- eslint -f json src");
    }
}
