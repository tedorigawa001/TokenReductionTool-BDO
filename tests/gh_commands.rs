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

// ---- gh run view, from what real gh prints ----

const FAILED_RUN: &str = r#"
X feature CI o/r#51 · 417
Triggered via pull_request about 7 hours ago

JOBS
✓ Rust formatting in 14s (ID 1)
✓ fail-fast in 14s (ID 4)
X Tests in 2m7s (ID 2)
  ✓ Set up job
  X Run tests
- Skipped job (ID 3)

ANNOTATIONS
- "The label will migrate"
Rust formatting: .github#1

X Process completed with exit code 1.
Tests: .github#2

To see what failed, try: gh run view 417 --log-failed
View this run on GitHub: https://github.com/o/r/actions/runs/417
"#;

/// It used to print only the hint, since gh's mark for a failed job is `X`.
#[test]
fn run_view_names_the_job_and_step_that_failed() {
    let env = Env::new();
    env.answer(FAILED_RUN);
    let s = env.ok(&["run", "view", "417"]);
    assert!(s.starts_with("Workflow Run #417\n"), "{s}");
    assert!(s.contains("X feature CI"), "the outcome: {s}");
    assert!(s.contains("X Tests in 2m7s"), "the job: {s}");
    assert!(s.contains("X Run tests"), "the step: {s}");
    assert!(s.contains("Process completed with exit code 1."), "{s}");
    assert!(s.contains("Tests: .github#2"), "{s}");
    assert!(s.contains("--log-failed"), "{s}");
    assert!(s.contains("Jobs: 2 passed, 1 skipped"), "{s}");
}

#[test]
fn run_view_leaves_out_what_needs_no_attention() {
    let env = Env::new();
    env.answer(FAILED_RUN);
    let s = env.ok(&["run", "view", "417"]);
    for gone in [
        "Rust formatting",
        "Set up job",
        "Skipped job",
        "will migrate",
    ] {
        assert!(!s.contains(gone), "{gone}: {s}");
    }
}

/// A job's name decides nothing: `fail-fast` passed above, and this one did not.
#[test]
fn run_view_goes_by_the_mark_not_the_name() {
    let env = Env::new();
    env.answer("✓ main CI · 1\n\nJOBS\nX success-report in 1s (ID 1)\n* still-running (ID 2)\n");
    let s = env.ok(&["run", "view", "1"]);
    assert!(s.contains("X success-report"), "{s}");
    assert!(s.contains("* still-running"), "{s}");
}

#[test]
fn run_view_of_a_passing_run_says_so() {
    let env = Env::new();
    env.answer("✓ main CI · 1\nTriggered via push\n\nJOBS\n✓ a in 1s (ID 1)\n✓ b in 1s (ID 2)\n");
    let s = env.ok(&["run", "view", "1"]);
    assert!(s.contains("✓ main CI"), "{s}");
    assert!(s.contains("Jobs: 2 passed, 0 skipped"), "{s}");
}

#[test]
fn run_view_output_flags_are_gh_own_output() {
    for flag in ["--log", "--log-failed"] {
        let env = Env::new();
        env.answer("the whole log\n");
        assert_eq!(env.ok(&["run", "view", "1", flag]), "the whole log\n");
    }
}

// ---- gh pr checks ----

/// Only the state column counts. These names and this URL all contain a state.
#[test]
fn pr_checks_reads_the_state_column_only() {
    let env = Env::new();
    env.answer(
        "bypass-check\tfail\t1m\thttps://example.com/compass/1\n\
         fail-fast\tpass\t1m\thttps://example.com/2\n\
         pending-report\tpass\t1m\thttps://example.com/3\n\
         deploy\tpending\t0\thttps://example.com/4\n\
         cleanup\tskipping\t0\thttps://example.com/5\n\
         flaky\tcancel\t1m\thttps://example.com/6\n",
    );
    let s = env.ok(&["pr", "checks", "7"]);
    assert!(s.contains("Passed: 2"), "{s}");
    assert!(s.contains("Failed: 2"), "{s}");
    assert!(s.contains("Pending: 1"), "{s}");
    assert!(s.contains("Skipped: 1"), "{s}");
    assert!(s.contains("bypass-check"), "{s}");
    assert!(s.contains("flaky"), "{s}");
    assert!(!s.contains("fail-fast"), "{s}");
}

// ---- gh pr view and pr status ----

#[test]
fn pr_view_counts_every_way_a_check_can_end() {
    let env = Env::new();
    env.answer(
        &json!({
            "number": 7, "title": "t", "state": "OPEN", "author": author("a"),
            "url": "https://example.com/7", "mergeable": "MERGEABLE", "body": "",
            "statusCheckRollup": [
                { "conclusion": "SUCCESS" },
                { "state": "SUCCESS" },
                { "conclusion": "FAILURE" },
                { "conclusion": "TIMED_OUT" },
                { "conclusion": "CANCELLED" },
                { "state": "ERROR" },
                { "conclusion": "SKIPPED" },
                { "conclusion": "NEUTRAL" },
                { "status": "IN_PROGRESS", "conclusion": "" },
                { "state": "PENDING" },
            ],
        })
        .to_string(),
    );
    let s = env.ok(&["pr", "view", "7"]);
    assert!(
        s.contains("Checks: 2/10 passed (2 pending, 2 skipped)"),
        "{s}"
    );
    assert!(s.contains("4 checks failed"), "{s}");
}

#[test]
fn pr_status_shows_all_three_groups() {
    let env = Env::new();
    let pr = |number: i64, title: &str| json!({ "number": number, "title": title });
    env.answer(
        &json!({
            "currentBranch": pr(1, "on this branch"),
            "createdBy": [pr(2, "mine")],
            "needsReview": [pr(3, "please look")],
        })
        .to_string(),
    );
    let s = env.ok(&["pr", "status"]);
    assert!(s.contains("Current Branch"), "{s}");
    assert!(s.contains("on this branch"), "{s}");
    assert!(s.contains("Your PRs (1):"), "{s}");
    assert!(s.contains("mine"), "{s}");
    assert!(s.contains("Requesting your review (1):"), "{s}");
    assert!(s.contains("please look"), "{s}");
}

// ---- the rest ----

#[test]
fn repo_view_keeps_the_identity_and_the_counts() {
    let env = Env::new();
    env.answer(
        &json!({
            "name": "r", "owner": { "login": "o" }, "description": "what it is",
            "url": "https://example.com/o/r", "stargazerCount": 3, "forkCount": 2,
            "isPrivate": true,
        })
        .to_string(),
    );
    let s = env.ok(&["repo", "view", "o/r"]);
    for wanted in [
        "o/r",
        "[private]",
        "what it is",
        "3 stars | 2 forks",
        "https://example.com/o/r",
    ] {
        assert!(s.contains(wanted), "{wanted}: {s}");
    }
    assert_eq!(env.received()[..3], ["repo", "view", "o/r"]);
}

#[test]
fn repo_subcommands_other_than_view_pass_through() {
    let env = Env::new();
    env.answer("cloned\n");
    assert_eq!(env.ok(&["repo", "clone", "o/r"]), "cloned\n");
    assert_eq!(env.received(), ["repo", "clone", "o/r"]);
}

#[test]
fn pr_diff_compacts_and_its_opt_outs_do_not() {
    let diff = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n";
    let env = Env::new();
    env.answer(diff);
    let s = env.ok(&["pr", "diff", "7"]);
    assert!(s.contains("+new") && s.contains("-old"), "{s}");

    let env = Env::new();
    env.answer(diff);
    assert_eq!(env.ok(&["pr", "diff", "7", "--no-compact"]), diff);
    assert!(!env.received().contains(&"--no-compact".to_string()));

    let env = Env::new();
    env.answer("f\n");
    assert_eq!(env.ok(&["pr", "diff", "7", "--name-only"]), "f\n");

    let env = Env::new();
    env.answer("");
    assert!(env.ok(&["pr", "diff", "7"]).contains("No diff"));
}

#[test]
fn api_output_is_never_summarised() {
    let env = Env::new();
    let body = "{\"a\":[1,2,3]}\n";
    env.answer(body);
    assert_eq!(env.ok(&["api", "repos/o/r"]), body);
}

#[test]
fn pr_merge_output_is_gh_own() {
    let env = Env::new();
    env.answer("Merged pull request #7\n");
    assert_eq!(
        env.ok(&["pr", "merge", "7", "--squash"]),
        "Merged pull request #7\n"
    );
    assert_eq!(env.received(), ["pr", "merge", "7", "--squash"]);
}
