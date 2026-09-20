//! A command that feeds a pipe must reach the next program exactly as written.
//!
//! The agent never sees the left side of a pipe — only the pipeline's final
//! output — so rewriting it to `bdo …` cannot save a token. It can only change
//! what the downstream program receives: `cat f | wc -l` used to count bdo's
//! truncated view of `f`, not `f`. These tests pin the invariant end to end:
//! whatever the hook emits, running it must produce the same result as the
//! original command.

use std::fs;
use std::path::Path;
use std::process::Command;

/// Feed a Bash tool call through the Claude hook and return the command it
/// wants run — the rewrite if it produced one, else the original.
fn hooked_command(dir: &Path, cmd: &str) -> String {
    let input = format!(
        r#"{{"tool_name":"Bash","tool_input":{{"command":{}}}}}"#,
        json_str(cmd)
    );
    let out = Command::new(env!("CARGO_BIN_EXE_bdo"))
        .current_dir(dir)
        .args(["hook", "claude"])
        .env("BDO_DB_PATH", dir.join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(input.as_bytes())?;
            child.wait_with_output()
        })
        .expect("spawn bdo hook");
    let text = String::from_utf8_lossy(&out.stdout);
    // The rewritten command lives at hookSpecificOutput.updatedInput.command;
    // absent means "run it as-is".
    extract_updated_command(&text).unwrap_or_else(|| cmd.to_string())
}

fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn extract_updated_command(json: &str) -> Option<String> {
    let key = "\"command\":\"";
    let start = json.find(key)? + key.len();
    let rest = &json[start..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?),
            '"' => return Some(out),
            c => out.push(c),
        }
    }
    None
}

fn sh(dir: &Path, cmd: &str) -> String {
    let out = Command::new("sh")
        .current_dir(dir)
        .args(["-c", cmd])
        .env("BDO_DB_PATH", dir.join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1")
        .output()
        .expect("spawn sh");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A file long enough that `bdo read`'s default view would truncate it.
fn big_file(dir: &Path) -> std::path::PathBuf {
    let p = dir.join("big.rs");
    let body: String = (0..2000).map(|i| format!("fn f{i}() {{}}\n")).collect();
    fs::write(&p, body).unwrap();
    p
}

#[test]
fn piped_cat_counts_the_real_file() {
    let dir = tempfile::tempdir().unwrap();
    big_file(dir.path());
    let original = "cat big.rs | wc -l";
    let hooked = hooked_command(dir.path(), original);
    assert!(
        !hooked.contains("bdo"),
        "left side of a pipe must not be rewritten: {hooked}"
    );
    assert_eq!(sh(dir.path(), &hooked), sh(dir.path(), original));
    assert_eq!(sh(dir.path(), &hooked), "2000");
}

#[test]
fn piped_cat_hashes_the_real_file() {
    let dir = tempfile::tempdir().unwrap();
    big_file(dir.path());
    let original = "cat big.rs | shasum";
    let hooked = hooked_command(dir.path(), original);
    assert_eq!(sh(dir.path(), &hooked), sh(dir.path(), original));
}

#[test]
fn only_the_pipe_feeding_segment_is_left_alone() {
    // `git status` reaches the agent directly and is still rewritten; the
    // `cat … | wc -l` half is not.
    let dir = tempfile::tempdir().unwrap();
    big_file(dir.path());
    let hooked = hooked_command(dir.path(), "git status; cat big.rs | wc -l");
    assert!(hooked.starts_with("bdo git status"), "{hooked}");
    assert!(hooked.ends_with("cat big.rs | wc -l"), "{hooked}");
}
