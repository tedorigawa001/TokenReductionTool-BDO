//! `bdo gain` keeps `bdo proxy` apart from the commands bdo filters, and does
//! not count binary output from `bdo proxy` as tokens.
//!
//! Unix only: the binary output comes from `printf`.
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

fn bdo(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bdo"))
        .args(args)
        .current_dir(home)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("BDO_DB_PATH", home.join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1")
        .envs(std::env::var("LLVM_PROFILE_FILE").map(|p| ("LLVM_PROFILE_FILE", p)))
        .output()
        .unwrap()
}

fn fresh_db(home: &Path) -> rusqlite::Connection {
    // Any tracked command creates the database with its schema.
    assert!(bdo(home, &["gain"]).status.success());
    let db = rusqlite::Connection::open(home.join("track.db")).unwrap();
    db.execute("DELETE FROM commands", []).unwrap();
    db
}

fn insert(db: &rusqlite::Connection, rtk_cmd: &str, input: i64, output: i64) {
    db.execute(
        "INSERT INTO commands (timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens, saved_tokens, savings_pct) \
         VALUES ('2026-01-02T03:04:05+00:00', ?1, ?2, ?3, ?4, ?3 - ?4, 0.0)",
        rusqlite::params![rtk_cmd.trim_start_matches("bdo "), rtk_cmd, input, output],
    )
    .unwrap();
}

fn summary(home: &Path) -> serde_json::Value {
    let out = bdo(home, &["gain", "-f", "json"]);
    assert!(out.status.success(), "{out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    json["summary"].clone()
}

#[test]
fn gain_reports_savings_without_bdo_proxy_alongside_the_total() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let db = fresh_db(&home);
    // Filtered: 1000 in, 200 out. Proxy: 1000 in, 1000 out.
    insert(&db, "bdo git status", 1000, 200);
    insert(&db, "bdo proxy git show HEAD:a.txt", 600, 600);
    insert(&db, "bdo proxy cat b.txt", 400, 400);
    drop(db);

    let s = summary(&home);
    assert_eq!(s["avg_savings_pct"].as_f64().unwrap().round(), 40.0, "{s}");
    assert_eq!(s["proxy_commands"], 2, "{s}");
    assert_eq!(s["proxy_input"], 1000, "{s}");
    assert_eq!(
        s["filtered_savings_pct"].as_f64().unwrap().round(),
        80.0,
        "{s}"
    );

    let out = bdo(&home, &["gain"]);
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .find(|l| l.starts_with("Without proxy:"))
        .unwrap_or_else(|| panic!("no proxy line\n{text}"));
    assert!(line.contains("80.0%"), "{line}");
    assert!(line.contains("2 bdo proxy commands"), "{line}");
}

#[test]
fn gain_has_no_proxy_line_when_nothing_ran_through_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let db = fresh_db(&home);
    insert(&db, "bdo git status", 1000, 200);
    drop(db);

    let s = summary(&home);
    assert_eq!(s["proxy_commands"], 0, "{s}");
    assert_eq!(
        s["filtered_savings_pct"].as_f64().unwrap().round(),
        80.0,
        "{s}"
    );
    let out = bdo(&home, &["gain"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains("Without proxy:"), "{text}");
}

#[test]
fn binary_output_through_proxy_is_passed_on_but_not_counted_as_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    drop(fresh_db(&home));

    let binary = format!("{}\\000{}", "a".repeat(4000), "b".repeat(4000));
    let out = bdo(&home, &["proxy", "printf", &binary]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        out.stdout.len(),
        8001,
        "proxy must pass binary output on unchanged"
    );
    assert_eq!(out.stdout[4000], 0);

    let text = "c".repeat(4000);
    assert!(bdo(&home, &["proxy", "printf", &text]).status.success());

    let s = summary(&home);
    assert_eq!(s["total_commands"], 2, "both runs are recorded: {s}");
    assert_eq!(s["total_input"], 1000, "only the text run counts: {s}");
}

#[test]
fn proxy_excludes_nul_free_binary_stdout_but_counts_text_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let out = bdo(
        &home,
        &[
            "proxy",
            "sh",
            "-c",
            r#"printf '\377\376\375\374\373\372\371\370'; printf 'warning!\n' >&2; exit 7"#,
        ],
    );
    assert_eq!(out.status.code(), Some(7), "{out:?}");
    assert_eq!(out.stdout, [0xff, 0xfe, 0xfd, 0xfc, 0xfb, 0xfa, 0xf9, 0xf8]);
    assert_eq!(out.stderr, b"warning!\n");

    let s = summary(&home);
    assert_eq!(s["proxy_commands"], 1, "{s}");
    assert_eq!(s["proxy_input"], 3, "only the text diagnostic counts: {s}");
    assert_eq!(s["total_output"], 3, "{s}");
    assert_eq!(s["total_saved"], 0, "{s}");
}

#[test]
fn proxy_excludes_binary_stderr_but_counts_text_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let out = bdo(
        &home,
        &[
            "proxy",
            "sh",
            "-c",
            r#"printf 'stdout!\n'; printf 'abcd\000efgh' >&2"#,
        ],
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(out.stdout, b"stdout!\n");
    assert_eq!(out.stderr, b"abcd\0efgh");
    let s = summary(&home);
    assert_eq!(s["proxy_commands"], 1, "{s}");
    assert_eq!(s["proxy_input"], 2, "only text stdout counts: {s}");
    assert_eq!(s["total_output"], 2, "{s}");
}

#[test]
fn proxy_checks_for_binary_bytes_beyond_the_tracking_sample() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let out = bdo(
        &home,
        &["proxy", "sh", "-c", r#"printf '%1048576s\377' ''"#],
    );
    assert!(out.status.success(), "{}", out.status);
    assert_eq!(out.stdout.len(), 1_048_577);
    assert_eq!(out.stdout.last(), Some(&0xff));
    let s = summary(&home);
    assert_eq!(s["proxy_commands"], 1, "{s}");
    assert_eq!(s["proxy_input"], 0, "{s}");
}

#[test]
fn proxy_counts_utf8_text_even_if_the_tracking_sample_splits_a_character() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let out = bdo(
        &home,
        &["proxy", "sh", "-c", r#"printf '%1048575s\303\251' ''"#],
    );
    assert!(out.status.success(), "{}", out.status);
    assert_eq!(out.stdout.len(), 1_048_577);
    assert!(out.stdout.ends_with("é".as_bytes()));
    let s = summary(&home);
    assert_eq!(s["proxy_commands"], 1, "{s}");
    assert_eq!(s["proxy_input"], 262_144, "{s}");
}
