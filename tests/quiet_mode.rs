//! `BDO_QUIET=1`: bdo's own advisory notes on stderr go away; output and
//! errors do not.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn bdo(dir: &Path, args: &[&str], quiet: Option<&str>) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_bdo"));
    c.current_dir(dir)
        .args(args)
        .env("BDO_DB_PATH", dir.join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1")
        .env_remove("BDO_RAW")
        .env_remove("BDO_QUIET");
    if let Some(v) = quiet {
        c.env("BDO_QUIET", v);
    }
    c.output().expect("spawn bdo")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A file long enough that `bdo read`'s default view truncates it.
fn big_file(dir: &Path) {
    let body: String = (0..2000).map(|i| format!("fn f{i}() {{}}\n")).collect();
    fs::write(dir.join("big.rs"), body).unwrap();
}

#[test]
fn quiet_drops_the_reduced_view_hint_but_not_the_output() {
    let dir = tempfile::tempdir().unwrap();
    big_file(dir.path());

    let loud = bdo(dir.path(), &["read", "big.rs"], None);
    assert!(stderr(&loud).contains("reduced view"), "{}", stderr(&loud));

    for v in ["1", "true", "YES", "on"] {
        let quiet = bdo(dir.path(), &["read", "big.rs"], Some(v));
        assert_eq!(stderr(&quiet), "", "BDO_QUIET={v}");
        assert_eq!(quiet.stdout, loud.stdout, "BDO_QUIET={v} changed stdout");
    }
}

#[test]
fn falsy_quiet_values_keep_the_hint() {
    let dir = tempfile::tempdir().unwrap();
    big_file(dir.path());
    for v in ["0", "false", ""] {
        let out = bdo(dir.path(), &["read", "big.rs"], Some(v));
        assert!(stderr(&out).contains("reduced view"), "BDO_QUIET={v:?}");
    }
}

#[test]
fn quiet_drops_the_raw_has_no_effect_note() {
    let dir = tempfile::tempdir().unwrap();
    let loud = bdo(dir.path(), &["--raw", "gain"], None);
    assert!(stderr(&loud).contains("--raw has no effect"));
    let quiet = bdo(dir.path(), &["--raw", "gain"], Some("1"));
    assert!(!stderr(&quiet).contains("--raw has no effect"));
}

#[test]
fn quiet_never_hides_errors() {
    let dir = tempfile::tempdir().unwrap();
    let out = bdo(dir.path(), &["read", "does-not-exist.rs"], Some("1"));
    assert!(!out.status.success());
    assert!(!stderr(&out).is_empty(), "errors must still reach stderr");
}

// The daily hook nag reads ~/.claude and writes its rate-limit marker under
// the data dir, so give the child a throwaway HOME with Claude Code "installed"
// but no hook. dirs:: honours HOME on unix only.
#[cfg(unix)]
#[test]
fn quiet_drops_the_hook_nag_without_consuming_it() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::write(dir.path().join("a.txt"), "hi\n").unwrap();

    let run = |quiet: Option<&str>| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_bdo"));
        c.current_dir(dir.path())
            .args(["read", "a.txt"])
            .env("HOME", &home)
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("XDG_DATA_HOME")
            .env("BDO_DB_PATH", dir.path().join("track.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env_remove("BDO_QUIET");
        if let Some(v) = quiet {
            c.env("BDO_QUIET", v);
        }
        c.output().expect("spawn bdo")
    };

    let quiet = run(Some("1"));
    assert!(!stderr(&quiet).contains("hook"), "{}", stderr(&quiet));
    // Suppressed, not spent: the next unquiet run still gets the once-a-day nag.
    let loud = run(None);
    assert!(
        stderr(&loud).contains("No hook installed"),
        "{}",
        stderr(&loud)
    );
}
