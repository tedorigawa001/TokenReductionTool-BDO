//! Integration test for `bdo test --changed` multi-language planning (#3).
//!
//! `core::testplan` is unit-tested in isolation; this exercises the real binary
//! end-to-end — change-set detection → per-language planning → the dispatch loop
//! that prints and runs each command. We assert on the `[<lang>]: <cmd>` header
//! lines, which are printed *before* each runner executes, so the test is stable
//! whether or not cargo/go/pytest are installed (a missing runner just fails the
//! command afterward; the header is already out). JS is left to the unit tests
//! so the integration run never reaches for `npx vitest` over the network.

use std::fs;
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .expect("spawn git")
        .success();
    assert!(ok, "git {args:?} failed");
}

fn run_test_changed(dir: &Path) -> String {
    let bdo = env!("CARGO_BIN_EXE_bdo");
    let out = Command::new(bdo)
        .current_dir(dir)
        .args(["test", "--changed"])
        .env("BDO_DB_PATH", dir.join("track.db")) // keep the real tracking DB clean
        .env("BDO_TELEMETRY_DISABLED", "1")
        .output()
        .expect("spawn bdo");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn test_changed_plans_each_language() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q"]);
    git(p, &["config", "user.email", "t@t"]);
    git(p, &["config", "user.name", "t"]);

    // One changed source file per supported (offline) language. The tempdir is
    // not a cargo/go project, so the runners fail fast — but each plan header is
    // printed first, which is what we assert.
    fs::create_dir_all(p.join("src/core")).unwrap();
    fs::create_dir_all(p.join("cmd/app")).unwrap();
    fs::create_dir_all(p.join("pkg")).unwrap();
    fs::write(p.join("src/core/widget.rs"), "fn main() {}\n").unwrap();
    fs::write(p.join("cmd/app/main.go"), "package main\n").unwrap();
    fs::write(p.join("pkg/svc.py"), "def f():\n    pass\n").unwrap();

    let out = run_test_changed(p);

    // Rust: inline-module stem filter (paths/stems are shell-quoted).
    assert!(
        out.contains("bdo test --changed [rust]: cargo test -- 'widget'"),
        "missing rust plan header: {out}"
    );
    // Go: the changed file's parent package.
    assert!(
        out.contains("bdo test --changed [go]: go test './cmd/app'"),
        "missing go plan header: {out}"
    );
    // Python: a non-test source file maps to a `-k` stem filter.
    assert!(
        out.contains("bdo test --changed [python]: pytest -k 'svc'"),
        "missing python plan header: {out}"
    );
}

#[test]
fn test_changed_reports_nothing_when_no_targets() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q"]);
    git(p, &["config", "user.email", "t@t"]);
    git(p, &["config", "user.name", "t"]);

    // A doc-only change has no test target in any language.
    fs::write(p.join("README.md"), "just docs\n").unwrap();

    let out = run_test_changed(p);
    assert!(
        out.contains("no test targets in the change set"),
        "expected the empty-plan message: {out}"
    );
}
