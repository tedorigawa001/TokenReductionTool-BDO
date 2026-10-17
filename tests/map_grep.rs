//! `bdo map --grep <PATTERN>`: the repo map narrowed to matching signatures.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn bdo(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bdo"))
        .current_dir(dir)
        .args(args)
        .env("BDO_DB_PATH", dir.join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1")
        .env_remove("BDO_RAW")
        .output()
        .expect("spawn bdo")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn tree(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/tracker.rs"),
        "pub struct Tracker {\n    n: u32,\n}\n\npub fn open_tracker() -> Tracker {\n    Tracker { n: 0 }\n}\n\npub fn unrelated() -> u32 {\n    1\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/parse.rs"),
        "pub fn parse_args(a: &str) -> Vec<String> {\n    vec![a.into()]\n}\n",
    )
    .unwrap();
}

#[test]
fn grep_keeps_only_matching_signatures_and_files() {
    let dir = tempfile::tempdir().unwrap();
    tree(dir.path());
    let out = bdo(dir.path(), &["map", "src", "--grep", "Tracker"]);
    assert!(out.status.success());
    let s = stdout(&out);
    assert!(s.contains("tracker.rs"), "{s}");
    assert!(s.contains("pub struct Tracker"), "{s}");
    assert!(s.contains("pub fn open_tracker() -> Tracker"), "{s}");
    assert!(!s.contains("unrelated"), "non-matching signature kept: {s}");
    assert!(!s.contains("parse.rs"), "file without a match kept: {s}");
    assert!(s.contains("2 of 4 signatures match /Tracker/"), "{s}");
}

#[test]
fn grep_is_a_regex_with_inline_flags() {
    let dir = tempfile::tempdir().unwrap();
    tree(dir.path());
    let s = stdout(&bdo(dir.path(), &["map", "src", "--grep", "(?i)PARSE_"]));
    assert!(s.contains("pub fn parse_args"), "{s}");
    assert!(!s.contains("Tracker"), "{s}");
}

#[test]
fn grep_with_no_match_says_so() {
    let dir = tempfile::tempdir().unwrap();
    tree(dir.path());
    let out = bdo(dir.path(), &["map", "src", "--grep", "nothing_here"]);
    assert!(out.status.success());
    assert!(
        stdout(&out).contains("No signatures match /nothing_here/ (4 searched)"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn invalid_grep_pattern_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    tree(dir.path());
    let out = bdo(dir.path(), &["map", "src", "--grep", "("]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid pattern"));
}

#[test]
fn mapping_a_single_file_labels_it() {
    let dir = tempfile::tempdir().unwrap();
    tree(dir.path());
    let s = stdout(&bdo(dir.path(), &["map", "src/parse.rs"]));
    let first = s.lines().next().unwrap_or_default();
    assert_eq!(first, "src/parse.rs", "{s}");
}
