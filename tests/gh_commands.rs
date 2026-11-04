//! `bdo gh` against a stand-in `gh` that records its arguments and replays a canned
//! answer, so write commands can be checked without touching GitHub.
//!
//! The stand-in is a shell script, which is why this is Unix only.
#![cfg(unix)]

use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const FAKE_GH: &str = r#"#!/bin/sh
for arg in "$@"; do printf '%s\n' "$arg" >> "$FAKE_GH_ARGS"; done
[ -f "$FAKE_GH_STDOUT" ] && cat "$FAKE_GH_STDOUT"
[ -f "$FAKE_GH_STDERR" ] && cat "$FAKE_GH_STDERR" >&2
exit "${FAKE_GH_EXIT:-0}"
"#;

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        let gh = bin.join("gh");
        fs::write(&gh, FAKE_GH).unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        Self { _dir: dir, root }
    }

    fn answer(&self, stdout: &str) -> &Self {
        fs::write(self.root.join("stdout"), stdout).unwrap();
        self
    }

    fn fails_with(&self, stderr: &str) -> &Self {
        fs::write(self.root.join("stderr"), stderr).unwrap();
        self
    }

    fn run(&self, args: &[&str], exit: i32) -> Output {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new(env!("CARGO_BIN_EXE_bdo"))
            .arg("gh")
            .args(args)
            .current_dir(&self.root)
            .env("PATH", path)
            .env("FAKE_GH_ARGS", self.root.join("args"))
            .env("FAKE_GH_STDOUT", self.root.join("stdout"))
            .env("FAKE_GH_STDERR", self.root.join("stderr"))
            .env("FAKE_GH_EXIT", exit.to_string())
            .env("BDO_DB_PATH", self.root.join("track.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env_remove("BDO_RAW")
            .output()
            .unwrap()
    }

    /// What the stand-in received, one argument per line.
    fn received(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("args"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args, 0);
        assert!(out.status.success(), "gh {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

fn author(login: &str) -> serde_json::Value {
    json!({ "login": login })
}

// ----------------------------------------------------------- read commands

#[test]
fn pr_list_shows_each_pull_request() {
    let env = Env::new();
    env.answer(
        &json!([
            { "number": 7, "title": "First title", "state": "OPEN", "author": author("alice"), "updatedAt": "2026-01-01T00:00:00Z" },
            { "number": 8, "title": "Second title", "state": "MERGED", "author": author("bob"), "updatedAt": "2026-01-01T00:00:00Z" },
        ])
        .to_string(),
    );
    let s = env.ok(&["pr", "list"]);
    for needle in ["7", "First title", "8", "Second title"] {
        assert!(s.contains(needle), "{needle} missing: {s}");
    }
}

#[test]
fn a_long_list_says_how_many_it_left_out() {
    let env = Env::new();
    let prs: Vec<_> = (0..60)
        .map(|i| {
            json!({ "number": i, "title": format!("title-{i}-end"), "state": "OPEN",
                    "author": author("a"), "updatedAt": "2026-01-01T00:00:00Z" })
        })
        .collect();
    env.answer(&json!(prs).to_string());
    let s = env.ok(&["pr", "list"]);
    let shown = (0..60)
        .filter(|i| s.contains(&format!("title-{i}-end")))
        .count();
    assert!(
        shown == 60 || s.contains("more"),
        "{shown} of 60, nothing said about the rest: {s}"
    );
}

#[test]
fn an_empty_list_says_so() {
    let env = Env::new();
    env.answer("[]");
    assert!(env.ok(&["pr", "list"]).contains("No Pull Requests"));
    assert!(env
        .ok(&["issue", "list"])
        .to_lowercase()
        .contains("no issues"));
}

#[test]
fn a_list_passes_its_filters_on() {
    let env = Env::new();
    env.answer("[]");
    env.ok(&["pr", "list", "--state", "closed", "--author", "me"]);
    let args = env.received();
    for needle in ["--state", "closed", "--author", "me"] {
        assert!(args.iter().any(|a| a == needle), "{needle}: {args:?}");
    }
}

#[test]
fn pr_view_keeps_what_a_reviewer_needs() {
    let env = Env::new();
    env.answer(
        &json!({
            "number": 7, "title": "The title", "state": "OPEN", "author": author("alice"),
            "body": "The body explains why.", "url": "https://example.com/pr/7",
            "mergeable": "CONFLICTING", "reviews": [], "statusCheckRollup": [],
        })
        .to_string(),
    );
    let s = env.ok(&["pr", "view", "7"]);
    for needle in [
        "7",
        "The title",
        "The body explains why.",
        "https://example.com/pr/7",
    ] {
        assert!(s.contains(needle), "{needle} missing: {s}");
    }
    assert!(s.contains("[x]"), "mergeable state missing: {s}");
}

#[test]
fn issue_view_keeps_its_body() {
    let env = Env::new();
    env.answer(
        &json!({ "number": 3, "title": "Bug", "state": "OPEN", "author": author("a"),
                 "body": "Steps to reproduce.", "url": "https://example.com/i/3" })
        .to_string(),
    );
    let s = env.ok(&["issue", "view", "3"]);
    assert!(s.contains("Bug"), "{s}");
    assert!(s.contains("Steps to reproduce."), "{s}");
}

#[test]
fn pr_checks_names_what_failed() {
    let env = Env::new();
    env.answer("build\tfail\t1m\thttps://example.com/1\nlint\tpass\t1m\thttps://example.com/2\n");
    let s = env.ok(&["pr", "checks", "7"]);
    assert!(s.contains("build"), "{s}");
    assert!(s.contains("1") && s.to_lowercase().contains("fail"), "{s}");
}

#[test]
fn run_list_shows_each_run() {
    let env = Env::new();
    env.answer(
        &json!([{ "databaseId": 11, "name": "CI", "status": "completed", "conclusion": "failure",
                  "createdAt": "2026-01-01T00:00:00Z" }])
        .to_string(),
    );
    let s = env.ok(&["run", "list"]);
    assert!(s.contains("CI"), "{s}");
    assert!(s.contains("11"), "{s}");
}

// ---------------------------------------------------------- write commands

/// These change things on GitHub, so each argument has to arrive as written.
#[test]
fn write_commands_reach_gh_exactly() {
    for args in [
        &["pr", "merge", "5", "--squash", "--delete-branch"][..],
        &[
            "pr",
            "create",
            "--title",
            "a b \"c\" $d",
            "--body",
            "line one\nline two",
        ][..],
        &["pr", "comment", "5", "--body", "looks good"][..],
        &["pr", "edit", "5", "--add-label", "bug"][..],
        &["pr", "close", "5"][..],
        &["issue", "close", "3"][..],
        &["release", "create", "v1.0.0", "--notes", "n"][..],
    ] {
        let env = Env::new();
        env.ok(args);
        // A multi-line argument records as several lines.
        let want: Vec<String> = args
            .iter()
            .flat_map(|a| a.lines().map(str::to_string))
            .collect();
        assert_eq!(env.received(), want, "{args:?}");
    }
}

#[test]
fn a_failed_write_keeps_its_exit_code_and_message() {
    for args in [
        &["pr", "merge", "5", "--squash"][..],
        &["pr", "create", "--title", "t", "--body", "b"][..],
        &["pr", "comment", "5", "--body", "b"][..],
        &["pr", "close", "5"][..],
    ] {
        let env = Env::new();
        env.fails_with("not allowed to do that\n");
        let out = env.run(args, 4);
        assert_eq!(out.status.code(), Some(4), "{args:?}: {out:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("not allowed to do that"),
            "{args:?}: {out:?}"
        );
    }
}

// ----------------------------------------------------------------- failures

#[test]
fn a_failed_read_keeps_its_exit_code_and_message() {
    for args in [
        &["pr", "list"][..],
        &["pr", "view", "7"][..],
        &["pr", "checks", "7"][..],
        &["issue", "list"][..],
        &["run", "list"][..],
        &["api", "repos/a/b"][..],
    ] {
        let env = Env::new();
        env.fails_with("HTTP 401: Bad credentials\n");
        let out = env.run(args, 1);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {out:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("Bad credentials"),
            "{args:?}: {out:?}"
        );
    }
}

#[test]
fn checks_keep_the_exit_code_that_means_failed() {
    // `gh pr checks` exits 1 when a check failed and 8 when some are pending.
    let env = Env::new();
    env.answer("build\tfail\t1m\thttps://example.com/1\n");
    let out = env.run(&["pr", "checks", "7"], 1);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("build"),
        "{out:?}"
    );

    let env = Env::new();
    env.answer("build\tpending\t1m\thttps://example.com/1\n");
    assert_eq!(env.run(&["pr", "checks", "7"], 8).status.code(), Some(8));
}

#[test]
fn an_answer_it_cannot_parse_is_not_lost() {
    let env = Env::new();
    env.answer("this is not JSON");
    let out = env.run(&["pr", "list"], 0);
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(all.contains("this is not JSON"), "{all}");
}

// ------------------------------------------------------------- passthrough

#[test]
fn explicit_json_is_the_bytes_gh_printed() {
    let env = Env::new();
    let raw = "[ {\"number\":   7} ]\n";
    env.answer(raw);
    let out = env.ok(&["pr", "list", "--json", "number"]);
    assert_eq!(out, raw);
    assert_eq!(env.received(), ["pr", "list", "--json", "number"]);
}

#[test]
fn unknown_subcommands_pass_through() {
    let env = Env::new();
    env.answer("gh version 9\n");
    assert_eq!(env.ok(&["--version"]), "gh version 9\n");

    let env = Env::new();
    env.answer("x\n");
    assert_eq!(env.ok(&["secret", "list"]), "x\n");
    assert_eq!(env.received(), ["secret", "list"]);
}
