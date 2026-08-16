//! Integration test for `bdo review` honoring `.bdostaleignore` (#1).
//!
//! `review::run` does live git + stdout, so the wiring (an ignored file is no
//! longer flagged for residue, yet still listed as changed) is exercised here
//! against the real binary in a throwaway git repo.

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

#[test]
fn review_respects_bdostaleignore() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q"]);
    git(p, &["config", "user.email", "t@t"]);
    git(p, &["config", "user.name", "t"]);

    // A doc that legitimately mentions a high-signal stale marker
    // (`cargo install bdo` — the wrong-crate-name install line).
    fs::write(p.join("CHANGELOG.md"), "history note: cargo install bdo\n").unwrap();

    let bdo = env!("CARGO_BIN_EXE_bdo");
    let db = p.join("track.db");
    let run = || {
        let out = Command::new(bdo)
            .current_dir(p)
            .arg("review")
            .env("BDO_DB_PATH", &db) // keep the real tracking DB clean
            .env("BDO_TELEMETRY_DISABLED", "1")
            .output()
            .expect("spawn bdo");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    // Without an ignore file, the marker in CHANGELOG.md is flagged.
    let before = run();
    assert!(
        before.contains("CHANGELOG.md"),
        "changed list (before): {before}"
    );
    assert!(
        before.contains("STALE MARKERS (1)"),
        "marker must be flagged before ignore: {before}"
    );

    // `.bdostaleignore` listing CHANGELOG.md suppresses the residue flag, but the
    // file is still reported in the CHANGED list (detection isn't hidden).
    fs::write(p.join(".bdostaleignore"), "CHANGELOG.md\n").unwrap();
    let after = run();
    assert!(
        after.contains("CHANGELOG.md"),
        "still listed as changed (after): {after}"
    );
    assert!(
        after.contains("STALE MARKERS (0)"),
        "marker must be suppressed after ignore: {after}"
    );
}
