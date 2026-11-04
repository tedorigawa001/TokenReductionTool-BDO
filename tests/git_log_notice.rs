//! `bdo git log` caps its output, and must say so: native `git log` prints the
//! whole history, so ten silent lines read as "this repository has ten commits".
//! Runs real git in a temporary repository.

use std::path::Path;
use std::process::{Command, Output};

const NOTICE: &str = "use -n N for more";

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
        .args(args)
        .output()
        .expect("run git");
    assert!(status.status.success(), "git {args:?}: {status:?}");
}

fn repo_with_commits(n: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    for i in 1..=n {
        git(
            dir.path(),
            &[
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                &format!("commit {i}"),
            ],
        );
    }
    dir
}

fn log(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bdo"))
        .current_dir(dir)
        .args(["git", "log"])
        .args(args)
        .env("BDO_DB_PATH", dir.join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1")
        .env_remove("BDO_RAW")
        .output()
        .expect("run bdo")
}

fn stdout(out: &Output) -> String {
    assert!(out.status.success(), "{out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn commit_lines(s: &str) -> usize {
    s.lines().filter(|l| l.contains("commit ")).count()
}

#[test]
fn a_capped_log_says_so() {
    let dir = repo_with_commits(12);
    let s = stdout(&log(dir.path(), &[]));
    assert_eq!(commit_lines(&s), 10, "{s}");
    assert!(s.contains(NOTICE), "{s}");
}

#[test]
fn a_log_that_shows_everything_says_nothing() {
    // The boundary: exactly the cap, which the extra commit requested must not
    // mistake for more.
    for n in [3, 10] {
        let dir = repo_with_commits(n);
        let s = stdout(&log(dir.path(), &[]));
        assert_eq!(commit_lines(&s), n, "{s}");
        assert!(!s.contains(NOTICE), "{n} commits: {s}");
    }
}

#[test]
fn a_user_format_is_capped_and_says_so_too() {
    let dir = repo_with_commits(52);
    let s = stdout(&log(dir.path(), &["--oneline"]));
    assert_eq!(commit_lines(&s), 50, "{s}");
    assert!(s.contains(NOTICE), "{s}");
}

#[test]
fn an_explicit_limit_is_exactly_what_was_asked_for() {
    let dir = repo_with_commits(12);
    for args in [
        &["-11"][..],
        &["-n", "11"][..],
        &["-n11"][..],
        &["--max-count=11"][..],
    ] {
        let s = stdout(&log(dir.path(), args));
        assert_eq!(commit_lines(&s), 11, "{args:?}: {s}");
        assert!(!s.contains(NOTICE), "{args:?}: {s}");
    }
}

#[test]
fn a_failure_keeps_its_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let out = log(dir.path(), &[]);
    assert!(!out.status.success());
}
