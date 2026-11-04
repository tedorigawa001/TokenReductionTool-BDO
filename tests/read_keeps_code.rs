//! `bdo read` may drop comments, never code.
//!
//! `cat` is rewritten to `bdo read`, so its output is what an agent believes the
//! file contains. A `/*` inside a string used to swallow every line up to the
//! next `*/`, or to the end of the file if there was none.

use std::fs;
use std::process::Command;

fn read(name: &str, source: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(name);
    fs::write(&file, source).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bdo"))
        .args(["read", file.to_str().unwrap()])
        .env("BDO_DB_PATH", dir.path().join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1")
        .env_remove("BDO_RAW")
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_glob_in_a_string_does_not_swallow_the_code_after_it() {
    let out = read(
        "a.ts",
        "const glob = \"src/*.ts\";\nconst b = 2;\nfunction keep() {}\n/* gone */\nconst c = x /* inline */ + y;\n",
    );
    assert_eq!(
        out,
        "const glob = \"src/*.ts\";\nconst b = 2;\nfunction keep() {}\nconst c = x /* inline */ + y;\n"
    );
}

#[test]
fn a_python_string_assignment_is_not_a_comment() {
    let source = "SQL = \"\"\"\nselect 1\n# not a comment\n\n\n\nselect 2\n\"\"\"\nafter = 1";
    assert_eq!(read("a.py", source), source);
}

#[test]
fn an_unknown_format_keeps_its_comment_like_lines() {
    let source = "first\n// a log line\n/* never closed\nlast";
    assert_eq!(read("run.output", source), source);
}

#[test]
fn comments_are_still_removed() {
    assert_eq!(
        read("a.rs", "// gone\nfn main() {}\n/* gone */\nfn other() {}\n"),
        "fn main() {}\nfn other() {}\n"
    );
}

/// `head -N` prints this much without bdo, so it must not count as a saving.
#[test]
fn a_line_window_is_not_recorded_as_a_saving() {
    let dir = tempfile::tempdir().unwrap();
    // Canonical: bdo refuses a tracking database behind a symlink, which is what
    // the temp directory is on macOS.
    let root = dir.path().canonicalize().unwrap();
    let file = root.join("big.csv");
    fs::write(&file, "a,b,c\n".repeat(5000)).unwrap();
    let db = root.join("track.db");
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bdo"))
            .arg("read")
            .args(args)
            .arg(&file)
            .env("BDO_DB_PATH", &db)
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env_remove("BDO_RAW")
            .output()
            .unwrap();
        assert!(out.status.success());
    };
    run(&["-m", "3"]);
    run(&["--tail-lines", "3"]);

    let conn = rusqlite::Connection::open(&db).unwrap();
    let rows: Vec<(String, i64)> = conn
        .prepare("SELECT original_cmd, saved_tokens FROM commands ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].0.starts_with("head "), "{:?}", rows[0]);
    assert!(rows[1].0.starts_with("tail "), "{:?}", rows[1]);
    assert_eq!((rows[0].1, rows[1].1), (0, 0));
}
