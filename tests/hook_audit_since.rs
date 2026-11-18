//! `bdo hook-audit --since N` counts the hooks that ran in the last N days,
//! wherever the machine's clock is set.
//!
//! The hook writes the log and `hook-audit` reads it, so both run through the
//! real binary here. Fixed-offset zones (`Etc/GMT-9` is UTC+9) keep the tests
//! free of daylight saving changes. `HOME` points at a temporary directory,
//! which is why this is Unix only.
#![cfg(unix)]

use jiff::{SignedDuration, Timestamp};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

/// Ahead of UTC and behind it, where a local clock read as UTC moves an entry
/// into the window or out of it.
const ZONES: [(&str, i32); 2] = [("Etc/GMT-9", 9), ("Etc/GMT+5", -5)];

struct Env {
    _dir: tempfile::TempDir,
    home: PathBuf,
    tz: &'static str,
}

impl Env {
    fn new(tz: &'static str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        Self {
            _dir: dir,
            home,
            tz,
        }
    }

    fn log(&self) -> PathBuf {
        self.home.join("hook-audit.log")
    }

    fn bdo(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_bdo"));
        cmd.args(args)
            .current_dir(&self.home)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("TZ", self.tz)
            .env("BDO_AUDIT_DIR", &self.home)
            .env("BDO_HOOK_AUDIT", "1")
            .env("BDO_DB_PATH", self.home.join("track.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .envs(std::env::var("LLVM_PROFILE_FILE").map(|p| ("LLVM_PROFILE_FILE", p)));
        cmd
    }

    fn hook(&self, command: &str) -> Output {
        let payload =
            serde_json::json!({ "tool_name": "Bash", "tool_input": { "command": command } });
        let mut child = self
            .bdo(&["hook", "claude"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn audit(&self, since_days: &str) -> String {
        let out = self
            .bdo(&["hook-audit", "--since", since_days])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

fn total(report: &str) -> Option<usize> {
    report
        .lines()
        .find_map(|l| l.strip_prefix("Total invocations:"))
        .map(|n| n.trim().parse().unwrap())
}

#[test]
fn a_logged_hook_says_when_it_ran_in_any_timezone() {
    for (tz, _) in ZONES {
        let env = Env::new(tz);
        let before = Timestamp::now();
        let out = env.hook("git status");
        assert!(out.status.success(), "{tz}: {out:?}");

        let log = fs::read_to_string(env.log()).unwrap();
        let stamp = log.split(" | ").next().unwrap();
        let at: Timestamp = stamp
            .parse()
            .unwrap_or_else(|e| panic!("{tz}: `{stamp}` names no instant: {e}"));
        let off = at.duration_since(before).as_secs();
        assert!(
            (-1..=60).contains(&off),
            "{tz}: `{stamp}` is {off}s from now"
        );
    }
}

#[test]
fn only_hooks_from_the_window_are_counted_in_any_timezone() {
    for (tz, hours) in ZONES {
        let env = Env::new(tz);
        let now = Timestamp::now();
        let utc = |ago: i64| (now - SignedDuration::from_hours(ago)).strftime("%Y-%m-%dT%H:%M:%SZ");
        // How the hook wrote it before: the local clock, with no offset.
        let local = |ago: i64| {
            // A fixed offset's wall clock is UTC shifted by that offset.
            (now - SignedDuration::from_hours(ago) + SignedDuration::from_hours(i64::from(hours)))
                .strftime("%Y-%m-%dT%H:%M:%S")
        };
        let lines = [
            format!("{} | rewrite | git status | bdo git status", utc(1)),
            format!("{} | rewrite | git log | bdo git log", utc(30)),
            format!("{} | rewrite | git diff | bdo git diff", local(1)),
            format!("{} | rewrite | git show | bdo git show", local(20)),
            format!("{} | rewrite | ls | bdo ls", local(30)),
        ];
        fs::write(env.log(), lines.join("\n") + "\n").unwrap();

        let report = env.audit("1");
        assert_eq!(total(&report), Some(3), "{tz}:\n{report}");
        assert_eq!(total(&env.audit("0")), Some(5), "{tz}");
    }
}
