//! Every wrapped tool, through a stand-in that fails in a way no filter expects.
//!
//! A filter recognises the output it knows. A tool can also fail with a bad flag,
//! a broken config or a crash, and the filter then has nothing to report: this is
//! how `tsc` came to say "No errors found", and `cargo nextest` nothing at all,
//! with the error gone. Whatever the tool, the arguments have to arrive, the exit
//! code has to come back, and the tool's own message has to be visible.
//!
//! Unix only: the stand-in is a shell script.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const STANDIN: &str = r#"#!/bin/sh
printf '%s' "$(basename "$0")" >> "$ROOT/log"
for a in "$@"; do printf ' %s' "$a" >> "$ROOT/log"; done
printf '\n' >> "$ROOT/log"
[ -n "$STANDIN_STDOUT" ] && echo "$STANDIN_STDOUT"
[ -n "$STANDIN_STDERR" ] && echo "$STANDIN_STDERR" >&2
exit "$STANDIN_EXIT"
"#;

const MESSAGE: &str = "the-tool-explains-what-went-wrong";

/// `(the tool bdo should run, the arguments to bdo)`. Each contains `zz`, which has
/// to reach the tool.
const COMMANDS: &[(&str, &[&str])] = &[
    ("tree", &["tree", "zzdir"]),
    ("docker", &["docker", "ps", "--zz"]),
    ("docker", &["docker", "images", "--zz"]),
    ("docker", &["docker", "logs", "zzc"]),
    ("docker", &["docker", "compose", "ps", "--zz"]),
    ("kubectl", &["kubectl", "get", "pods", "-n", "zz"]),
    ("kubectl", &["kubectl", "logs", "zzpod"]),
    ("psql", &["psql", "-c", "zzq"]),
    ("aws", &["aws", "sts", "get-caller-identity", "--zz"]),
    ("aws", &["aws", "s3", "ls", "zz"]),
    ("glab", &["glab", "mr", "list", "--zz"]),
    ("glab", &["glab", "issue", "list", "--zz"]),
    ("pnpm", &["pnpm", "list", "--zz"]),
    ("pnpm", &["pnpm", "install", "--zz"]),
    ("pnpm", &["pnpm", "outdated", "--zz"]),
    ("pnpm", &["pnpm", "run", "zz"]),
    ("jest", &["jest", "--zz"]),
    ("vitest", &["vitest", "run", "--zz"]),
    ("prisma", &["prisma", "generate", "--zz"]),
    ("prisma", &["prisma", "migrate", "status", "--zz"]),
    ("tsc", &["tsc", "--noEmit", "--zz"]),
    ("next", &["next", "build", "--zz"]),
    ("eslint", &["lint", "--zz", "src"]),
    ("prettier", &["prettier", "--check", "zz"]),
    ("npx", &["playwright", "test", "--zz"]),
    ("cargo", &["cargo", "build", "--zz"]),
    ("cargo", &["cargo", "test", "--zz"]),
    ("cargo", &["cargo", "clippy", "--zz"]),
    ("cargo", &["cargo", "check", "--zz"]),
    ("cargo", &["cargo", "install", "zz"]),
    ("cargo", &["cargo", "nextest", "run", "--zz"]),
    ("cargo", &["cargo", "fmt", "--zz"]),
    ("npm", &["npm", "run", "zz"]),
    ("npm", &["npm", "install", "zz"]),
    ("npx", &["npx", "zz", "--yy"]),
    ("curl", &["curl", "https://zz.invalid/x"]),
    ("ruff", &["ruff", "check", "--zz"]),
    ("ruff", &["ruff", "format", "--zz"]),
    ("pytest", &["pytest", "-k", "zz"]),
    ("mypy", &["mypy", "zz"]),
    ("rake", &["rake", "zz"]),
    ("rubocop", &["rubocop", "zz"]),
    ("rspec", &["rspec", "zz"]),
    ("pip", &["pip", "list", "--zz"]),
    ("pip", &["pip", "install", "zz"]),
    ("go", &["go", "test", "./zz"]),
    ("go", &["go", "build", "./zz"]),
    ("go", &["go", "vet", "./zz"]),
    ("gt", &["gt", "log", "--zz"]),
    ("gt", &["gt", "submit", "--zz"]),
    ("golangci-lint", &["golangci-lint", "run", "./zz"]),
    ("mvn", &["mvn", "test", "-Dzz=1"]),
    ("dotnet", &["dotnet", "build", "zz.csproj"]),
    ("dotnet", &["dotnet", "test", "zz.csproj"]),
    ("dotnet", &["dotnet", "restore", "zz"]),
    ("dotnet", &["dotnet", "format", "zz"]),
];

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    work: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let (bin, work) = (root.join("bin"), root.join("work"));
        fs::create_dir(&bin).unwrap();
        fs::create_dir(&work).unwrap();
        for (tool, _) in COMMANDS {
            let script = bin.join(tool);
            fs::write(&script, STANDIN).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self {
            _tmp: tmp,
            root,
            work,
        }
    }

    fn bdo(&self, args: &[&str], exit: i32, stdout: &str, stderr: &str) -> Output {
        let _ = fs::remove_file(self.root.join("log"));
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new(env!("CARGO_BIN_EXE_bdo"))
            .args(args)
            .current_dir(&self.work)
            .env("PATH", path)
            .env("ROOT", &self.root)
            .env("HOME", &self.root)
            .env("STANDIN_EXIT", exit.to_string())
            .env("STANDIN_STDOUT", stdout)
            .env("STANDIN_STDERR", stderr)
            .env("BDO_DB_PATH", self.root.join("track.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .output()
            .unwrap()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.root.join("log")).unwrap_or_default()
    }
}

fn shown(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn every_tool_gets_its_arguments_and_keeps_its_exit_code() {
    let env = Env::new();
    for (tool, args) in COMMANDS {
        let failed = env.bdo(args, 3, "", MESSAGE);
        assert_eq!(failed.status.code(), Some(3), "{args:?}: {failed:?}");
        let log = env.log();
        assert!(
            log.contains(&format!("{tool} ")) || log.starts_with(tool),
            "{args:?}: {log}"
        );
        let wanted = args.iter().find(|a| a.contains("zz")).unwrap();
        assert!(log.contains(wanted), "{args:?}: {log}");

        let passed = env.bdo(args, 0, "", "");
        assert_eq!(passed.status.code(), Some(0), "{args:?}: {passed:?}");
    }
}

#[test]
fn a_failure_on_stderr_is_never_hidden() {
    let env = Env::new();
    for (_, args) in COMMANDS {
        let out = env.bdo(args, 3, "", MESSAGE);
        assert!(shown(&out).contains(MESSAGE), "{args:?}: {out:?}");
    }
}

#[test]
fn a_failure_is_only_reported_once() {
    let env = Env::new();
    let repeated: Vec<_> = COMMANDS
        .iter()
        .filter(|(_, args)| {
            shown(&env.bdo(args, 3, "", MESSAGE))
                .matches(MESSAGE)
                .count()
                > 1
        })
        .map(|(_, args)| args.join(" "))
        .collect();
    assert_eq!(repeated, Vec::<String>::new());
}

#[test]
fn a_failure_on_stdout_is_never_hidden() {
    let env = Env::new();
    let hidden: Vec<_> = COMMANDS
        .iter()
        .filter(|(_, args)| !shown(&env.bdo(args, 3, MESSAGE, "")).contains(MESSAGE))
        .map(|(_, args)| args.join(" "))
        .collect();
    assert_eq!(hidden, Vec::<String>::new());
}

#[test]
fn bracketed_logs_do_not_hide_a_failure() {
    let env = Env::new();
    for stdout in [
        "[ERROR] broken configuration",
        "{broken configuration}",
        "Starting compiler\n[INFO] loading config\nbroken configuration",
    ] {
        let out = env.bdo(&["tsc", "--noEmit"], 3, stdout, "");
        assert_eq!(out.status.code(), Some(3));
        assert!(shown(&out).contains("broken configuration"), "{out:?}");
    }
}

/// With nothing to quote, the failure itself still has to be stated. The exit code
/// alone does not reach a reader of the output.
#[test]
fn a_silent_failure_is_stated() {
    let env = Env::new();
    for args in [
        &["tsc", "--noEmit"][..],
        &["cargo", "clippy"][..],
        &["cargo", "build"][..],
        &["cargo", "nextest", "run"][..],
        &["go", "vet", "./x"][..],
        &["mypy", "x"][..],
        &["prettier", "--check", "x"][..],
    ] {
        let out = env.bdo(args, 3, "", "");
        assert!(
            shown(&out).contains("the command failed (exit 3)"),
            "{args:?}: {out:?}"
        );
    }
}

/// The note is for failures the filter did not understand. On success, and when the
/// filter quoted the tool, it would only cost tokens.
#[test]
fn no_note_when_there_is_nothing_to_add() {
    let env = Env::new();
    let ok = env.bdo(&["tsc", "--noEmit"], 0, "", "");
    assert!(!shown(&ok).contains("[bdo] the command failed"), "{ok:?}");

    // `tsc` understands this one, and groups it.
    let understood = env.bdo(
        &["tsc", "--noEmit"],
        2,
        "src/a.ts(1,2): error TS2304: Cannot find name 'x'.",
        "",
    );
    let text = shown(&understood);
    assert!(text.contains("TS2304"), "{text}");
    assert!(!text.contains("the command failed"), "{text}");
}

/// A failing `cargo test`: the filter read stdout, and stderr is mostly progress.
#[test]
fn when_the_filter_read_stdout_only_errors_are_added_from_stderr() {
    let env = Env::new();
    let stdout = "test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured";

    let out = env.bdo(
        &["cargo", "test"],
        101,
        stdout,
        "Compiling x v0.1.0\nFinished test profile\nerror: test failed, to rerun pass --bin x",
    );
    let text = shown(&out);
    assert!(text.contains("error: test failed, to rerun"), "{text}");
    assert!(!text.contains("Compiling"), "{text}");

    let out = env.bdo(&["cargo", "test"], 101, stdout, "Compiling x v0.1.0");
    let text = shown(&out);
    assert!(!text.contains("the command failed"), "{text}");
}

/// Findings in JSON: the filter reports them in its own words, so none of the
/// tool's lines appear in the output, and that is not a sign of anything lost.
#[test]
fn no_note_when_the_filter_read_structured_output() {
    let env = Env::new();
    let findings = r#"[{"filePath":"a.js","messages":[{"ruleId":"no-undef","severity":2,"message":"x is not defined","line":1,"column":1}],"errorCount":1,"warningCount":0}]"#;
    let out = env.bdo(&["lint", "a.js"], 1, findings, "");
    let text = shown(&out);
    assert!(text.contains("no-undef"), "{text}");
    assert!(!text.contains("the command failed"), "{text}");
}

#[test]
fn structured_stdout_does_not_hide_a_separate_stderr_failure() {
    let env = Env::new();
    let out = env.bdo(&["lint", "a.js"], 3, "[]", "fatal: broken configuration");
    assert_eq!(out.status.code(), Some(3));
    assert!(
        shown(&out).contains("fatal: broken configuration"),
        "{out:?}"
    );
}

#[test]
fn only_the_end_of_a_long_failure_is_added() {
    let env = Env::new();
    let long: String = (1..=50).map(|i| format!("line-{i}\n")).collect();
    let out = env.bdo(&["go", "vet", "./x"], 3, "", long.trim_end());
    let text = shown(&out);
    assert!(text.contains("line-50"), "{text}");
    assert!(!text.contains("line-40\n"), "{text}");
}

#[test]
fn next_build_does_not_build_a_directory_called_build() {
    let env = Env::new();
    env.bdo(&["next", "build"], 0, "", "");
    assert_eq!(env.log().trim(), "next build");
    env.bdo(&["next"], 0, "", "");
    assert_eq!(env.log().trim(), "next build");
}

/// A script's `pnpm outdated && deploy` relies on these.
#[test]
fn findings_keep_the_exit_code_that_reports_them() {
    let env = Env::new();
    let out = env.bdo(&["pnpm", "outdated"], 1, "{}", "");
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let out = env.bdo(&["golangci-lint", "run"], 1, "", "");
    assert_eq!(out.status.code(), Some(1), "{out:?}");
}
