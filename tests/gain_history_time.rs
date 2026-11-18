//! `bdo gain --history` shows when each command ran on the local clock, from
//! the UTC timestamp the tracker stored.
//!
//! Fixed-offset zones (`Etc/GMT-9` is UTC+9) keep the test free of daylight
//! saving changes. Unix only: `TZ` is how the local zone is set here.
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

fn bdo(home: &Path, tz: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bdo"))
        .args(args)
        .current_dir(home)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("TZ", tz)
        .env("BDO_DB_PATH", home.join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1")
        .envs(std::env::var("LLVM_PROFILE_FILE").map(|p| ("LLVM_PROFILE_FILE", p)))
        .output()
        .unwrap()
}

#[test]
fn history_shows_the_stored_instant_on_the_local_clock() {
    for (tz, shown) in [("Etc/GMT-9", "01-02 12:04"), ("Etc/GMT+5", "01-01 22:04")] {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        // Any tracked command creates the database with its schema.
        assert!(bdo(&home, tz, &["gain"]).status.success());

        let db = rusqlite::Connection::open(home.join("track.db")).unwrap();
        db.execute("DELETE FROM commands", []).unwrap();
        db.execute(
            "INSERT INTO commands (timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens, saved_tokens, savings_pct) \
             VALUES ('2026-01-02T03:04:05.123456789+00:00', 'git status', 'bdo git status', 100, 20, 80, 80.0)",
            [],
        )
        .unwrap();
        drop(db);

        let out = bdo(&home, tz, &["gain", "--history"]);
        assert!(out.status.success(), "{tz}: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let line = stdout
            .lines()
            .skip_while(|l| !l.contains("Recent Commands"))
            .find(|l| l.contains("bdo git status"))
            .unwrap_or_else(|| panic!("{tz}: no history line\n{stdout}"));
        assert!(line.starts_with(shown), "{tz}: {line}");
    }
}
