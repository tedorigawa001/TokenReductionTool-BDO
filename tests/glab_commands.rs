//! `bdo glab` against a stand-in `glab` that records its arguments and replays a
//! canned answer, so every subcommand can be checked without touching GitLab.
//!
//! The stand-in is a shell script, which is why this is Unix only.
#![cfg(unix)]

use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const FAKE_GLAB: &str = r#"#!/bin/sh
for arg in "$@"; do printf '%s\n' "$arg" >> "$FAKE_GLAB_ARGS"; done
[ -f "$FAKE_GLAB_STDOUT" ] && cat "$FAKE_GLAB_STDOUT"
[ -f "$FAKE_GLAB_STDERR" ] && cat "$FAKE_GLAB_STDERR" >&2
exit "${FAKE_GLAB_EXIT:-0}"
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
        let glab = bin.join("glab");
        fs::write(&glab, FAKE_GLAB).unwrap();
        fs::set_permissions(&glab, fs::Permissions::from_mode(0o755)).unwrap();
        Self { _dir: dir, root }
    }

    fn answer(&self, stdout: &str) -> &Self {
        fs::write(self.root.join("stdout"), stdout).unwrap();
        self
    }

    fn fixture(&self, name: &str) -> &Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        self.answer(&fs::read_to_string(path).unwrap())
    }

    fn run(&self, args: &[&str], exit: i32) -> Output {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new(env!("CARGO_BIN_EXE_bdo"))
            .arg("glab")
            .args(args)
            .current_dir(&self.root)
            .env("PATH", path)
            .env("FAKE_GLAB_ARGS", self.root.join("args"))
            .env("FAKE_GLAB_STDOUT", self.root.join("stdout"))
            .env("FAKE_GLAB_STDERR", self.root.join("stderr"))
            .env("FAKE_GLAB_EXIT", exit.to_string())
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
        assert!(out.status.success(), "glab {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

fn mr() -> serde_json::Value {
    json!({
        "iid": 42,
        "title": "Add the widget",
        "state": "opened",
        "author": { "username": "alice" },
        "web_url": "https://gitlab.example.com/g/p/-/merge_requests/42",
        "merge_status": "cannot_be_merged",
        "source_branch": "feature",
        "target_branch": "main",
        "labels": ["backend", "urgent"],
        "reviewers": [{ "username": "bob" }],
        "head_pipeline": { "status": "failed" },
        "description": "Fixes the widget.\n\n<!-- hidden note -->\n\n```\nkeep  this\n```\n",
    })
}

// ----------------------------------------------------------- read commands

#[test]
fn mr_list_shows_each_merge_request() {
    let env = Env::new();
    env.fixture("glab_mr_list_raw.json");
    let out = env.ok(&["mr", "list", "--assignee=@me"]);
    assert!(out.starts_with("Merge Requests\n"), "{out}");
    assert!(out.contains("[open] !314 feat(glab)"), "{out}");
    assert!(out.contains("[merged] !310 "), "{out}");
    assert!(out.contains("(alice_dev)"), "{out}");
    assert_eq!(
        env.received(),
        ["mr", "list", "-F", "json", "--assignee=@me"]
    );
}

#[test]
fn issue_list_shows_each_issue() {
    let env = Env::new();
    env.fixture("glab_issue_list_raw.json");
    let out = env.ok(&["issue", "list"]);
    assert!(out.starts_with("Issues\n"), "{out}");
    assert!(out.contains("[open] #156 Support glab CI"), "{out}");
}

#[test]
fn mr_view_keeps_what_a_reviewer_needs() {
    let env = Env::new();
    env.answer(&mr().to_string());
    let out = env.ok(&["mr", "view", "42"]);
    for want in [
        "[open] MR !42: Add the widget",
        "[conflict]",
        "feature -> main",
        "Labels: backend, urgent",
        "Reviewers: @bob",
        "Pipeline: [fail] failed",
        "merge_requests/42",
        "Fixes the widget.",
        "keep  this",
    ] {
        assert!(out.contains(want), "{want}:\n{out}");
    }
    assert!(!out.contains("hidden note"), "{out}");
    assert_eq!(env.received(), ["mr", "view", "42", "-F", "json"]);
}

#[test]
fn issue_view_keeps_its_description() {
    let env = Env::new();
    env.answer(
        &json!({
            "iid": 7, "title": "It breaks", "state": "opened",
            "author": { "username": "carol" },
            "web_url": "https://gitlab.example.com/g/p/-/issues/7",
            "description": "Steps:\n1. run it",
        })
        .to_string(),
    );
    let out = env.ok(&["issue", "view", "7"]);
    for want in [
        "[open] Issue #7: It breaks",
        "@carol",
        "issues/7",
        "1. run it",
    ] {
        assert!(out.contains(want), "{want}:\n{out}");
    }
}

/// glab takes flags anywhere, and some of them take a value that bdo doesn't know
/// about. Moving the number in front of them hands the value to glab as the number.
#[test]
fn view_keeps_the_arguments_in_the_order_given() {
    for (args, number) in [
        (["mr", "view", "-P", "5", "42"], "42"),
        (["issue", "view", "--per-page", "5", "7"], "7"),
    ] {
        let env = Env::new();
        env.answer(&mr().to_string());
        env.ok(&args);
        let got = env.received();
        assert_eq!(&got[..5], &args[..], "{got:?}");
        assert_eq!(got[4], number, "{got:?}");
        assert_eq!(&got[5..], ["-F", "json"], "{got:?}");
    }
}

#[test]
fn view_opt_outs_are_glab_own_output() {
    for args in [
        &["mr", "view", "42", "--web"][..],
        &["mr", "view", "42", "-w"],
        &["mr", "view", "42", "--comments"],
        &["issue", "view", "7", "-c"],
    ] {
        let env = Env::new();
        env.answer("as glab prints it\n");
        assert_eq!(env.ok(args), "as glab prints it\n", "{args:?}");
        assert_eq!(env.received(), args, "{args:?}");
    }
}

#[test]
fn explicit_output_is_the_bytes_glab_printed() {
    let raw = "[ {\"iid\":   7} ]\n";
    for args in [
        &["mr", "list", "-F", "json"][..],
        &["mr", "list", "--output", "json"],
        &["mr", "list", "--output=json"],
        &["mr", "list", "-Fjson"],
        &["mr", "view", "42", "--output=json"],
        &["issue", "view", "7", "-Fjson"],
    ] {
        let env = Env::new();
        env.answer(raw);
        assert_eq!(env.ok(args), raw, "{args:?}");
        assert_eq!(env.received(), args, "{args:?}");
    }
}

#[test]
fn ci_list_and_status_show_each_pipeline() {
    let env = Env::new();
    env.answer(
        &json!([
            { "id": 9, "status": "success", "ref": "main" },
            { "id": 8, "status": "failed", "ref": "feature" },
        ])
        .to_string(),
    );
    let out = env.ok(&["ci", "list"]);
    assert!(out.contains("[ok] #9 success (main)"), "{out}");
    assert!(out.contains("[fail] #8 failed (feature)"), "{out}");
    assert_eq!(env.received(), ["ci", "list", "-F", "json"]);

    let env = Env::new();
    env.answer("(failed) • 00m 31s\ttest\tunit\n(success) • 00m 12s\tbuild\tcompile\n");
    let out = env.ok(&["pipeline", "status", "--branch", "main"]);
    assert!(out.contains("[fail] (failed)"), "{out}");
    assert!(out.contains("[ok] (success)"), "{out}");
    assert_eq!(env.received(), ["ci", "status", "--branch", "main"]);
}

#[test]
fn ci_trace_drops_the_runner_and_keeps_the_job() {
    let env = Env::new();
    env.fixture("glab_ci_trace_raw.txt");
    let out = env.ok(&["ci", "trace", "123"]);
    assert!(out.contains("$ npm test"), "{out}");
    assert!(out.contains("ERROR: Job failed: exit code 1"), "{out}");
    assert!(!out.contains("runner-abc123"), "{out}");
    assert!(!out.contains("Cleaning up project directory"), "{out}");
    assert!(!out.contains('\x1b'), "{out:?}");
    assert_eq!(env.received(), ["ci", "trace", "123"]);
}

/// The job's own output can start the way the runner's lines do.
#[test]
fn ci_trace_keeps_job_output_that_looks_like_the_runner() {
    let env = Env::new();
    env.answer(
        "Running on runner-1 via host...\n\
         $ npm start\n\
         Running on http://localhost:3000\n\
         Cleaning up temp files\n\
         Cleaning up project directory and file based variables\n",
    );
    let out = env.ok(&["ci", "trace", "5"]);
    assert_eq!(
        out.trim_end(),
        "$ npm start\nRunning on http://localhost:3000\nCleaning up temp files"
    );
}

#[test]
fn release_list_and_view_drop_the_noise() {
    let env = Env::new();
    env.fixture("glab_release_list_raw.txt");
    let out = env.ok(&["release", "list"]);
    assert!(out.starts_with("Releases\n"), "{out}");
    assert!(out.contains("  v3.2.1 (about 2 days ago)"), "{out}");
    assert!(!out.contains("Showing"), "{out}");

    let env = Env::new();
    env.fixture("glab_release_view_raw.txt");
    let out = env.ok(&["release", "view", "v2.0.0"]);
    assert!(out.contains("Added widget support"), "{out}");
    assert!(out.contains("View this release on GitLab"), "{out}");
    assert!(!out.contains("toolkit-v2.0.0.zip"), "{out}");
    assert!(!out.contains("internal tracking"), "{out}");
    assert!(!out.contains("Image: logo"), "{out}");
}

#[test]
fn mr_diff_is_compacted_and_an_empty_one_says_so() {
    let env = Env::new();
    env.answer("");
    assert_eq!(env.ok(&["mr", "diff", "42"]).trim(), "No diff");

    let env = Env::new();
    env.answer(
        "diff --git a/src/x.rs b/src/x.rs\n--- a/src/x.rs\n+++ b/src/x.rs\n@@ -1 +1 @@\n-old\n+new\n",
    );
    let out = env.ok(&["mr", "diff", "42"]);
    assert!(out.contains("x.rs"), "{out}");
    assert!(out.contains("+new"), "{out}");
}

// ---------------------------------------------------------- write commands

#[test]
fn write_commands_reach_glab_exactly_and_name_the_mr() {
    for (args, said) in [
        (&["mr", "approve", "42"][..], "ok approved !42"),
        (&["mr", "note", "-m", "looks good", "42"], "ok noted !42"),
        (
            &["mr", "update", "--title", "New title", "42"],
            "ok updated !42",
        ),
        (&["mr", "update", "42", "--label", "bug"], "ok updated !42"),
        (
            &[
                "mr",
                "approve",
                "https://gitlab.example.com/g/p/-/merge_requests/9",
            ],
            "ok approved !9",
        ),
        (&["mr", "update", "feature", "--title", "x"], "ok updated"),
    ] {
        let env = Env::new();
        env.answer("whatever glab says\n");
        assert_eq!(env.ok(args).trim(), said, "{args:?}");
        assert_eq!(env.received(), args, "{args:?}");
    }
}

/// A merge can't be undone, and glab may only set it to happen once the pipeline
/// passes, so its own words are kept, as for `gh pr merge`.
#[test]
fn mr_merge_output_is_glab_own() {
    let env = Env::new();
    env.answer("Will auto-merge !42 when the pipeline succeeds\n");
    assert_eq!(
        env.ok(&["mr", "merge", "42", "--squash"]),
        "Will auto-merge !42 when the pipeline succeeds\n"
    );
    assert_eq!(env.received(), ["mr", "merge", "42", "--squash"]);
}

#[test]
fn mr_create_names_the_new_mr() {
    let env = Env::new();
    env.answer("https://gitlab.example.com/g/p/-/merge_requests/12\n");
    let out = env.ok(&["mr", "create", "--fill", "--yes"]);
    assert_eq!(
        out.trim(),
        "ok created !12 https://gitlab.example.com/g/p/-/merge_requests/12"
    );
    assert_eq!(env.received(), ["mr", "create", "--fill", "--yes"]);
}

#[test]
fn a_failure_keeps_its_exit_code_and_message() {
    for args in [
        &["mr", "view", "42"][..],
        &["mr", "merge", "42"],
        &["ci", "trace", "1"],
    ] {
        let env = Env::new();
        fs::write(env.root.join("stderr"), "glab-explains-what-went-wrong\n").unwrap();
        let out = env.run(args, 3);
        assert_eq!(out.status.code(), Some(3), "{args:?}: {out:?}");
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            all.contains("glab-explains-what-went-wrong"),
            "{args:?}: {all}"
        );
    }
}

#[test]
fn unknown_subcommands_pass_through() {
    let env = Env::new();
    env.answer("x\n");
    assert_eq!(env.ok(&["repo", "list"]), "x\n");
    assert_eq!(env.received(), ["repo", "list"]);

    let env = Env::new();
    env.answer("{\"a\": 1}\n");
    assert_eq!(env.ok(&["api", "projects"]), "{\"a\": 1}\n");
}
