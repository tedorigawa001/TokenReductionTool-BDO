//! `bdo cc-economics` through the real binary, against a seeded history database and
//! a stand-in `ccusage`.
//!
//! The report joins two sources on the period they cover: what Claude Code spent,
//! from `ccusage`, and what bdo saved, from its own history. A period that doesn't
//! line up shows up as two half-empty rows and no savings at all, so every view is
//! checked on the same data.
//!
//! The stand-in answers like ccusage: weeks start on Sunday unless it is asked for
//! `--start-of-week monday`.
//!
//! Unix only: the stand-in is a shell script.
#![cfg(unix)]

use chrono::{Datelike, Duration, NaiveDate, Utc};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const CCUSAGE: &str = r#"#!/bin/sh
echo "$*" >> "$ROOT/ccusage.log"
[ -n "$CC_FAIL" ] && { echo "$CC_FAIL" >&2; exit 1; }
m='"inputTokens":1000,"outputTokens":100,"cacheCreationTokens":0,"cacheReadTokens":0,"totalTokens":1100,"totalCost":1.5'
case "$1" in
  daily) echo "{\"daily\":[{\"date\":\"$CC_DAY\",$m}]}" ;;
  weekly)
    week=$CC_SUNDAY
    case " $* " in *" --start-of-week monday "*) week=$CC_MONDAY ;; esac
    echo "{\"weekly\":[{\"week\":\"$week\",$m}]}" ;;
  monthly) echo "{\"monthly\":[{\"month\":\"$CC_MONTH\",$m}]}" ;;
esac
"#;

/// A Monday more than a week back, so its whole week has passed whatever today is.
fn last_monday() -> NaiveDate {
    let today = Utc::now().date_naive();
    today - Duration::days(i64::from(today.weekday().num_days_from_monday()) + 7)
}

struct Econ {
    _dir: tempfile::TempDir,
    /// bdo refuses a database path through a symlink, as macOS's temp dir is.
    root: PathBuf,
    monday: NaiveDate,
}

impl Econ {
    /// bdo saved 800 tokens on the Wednesday and 200 on the Sunday of one week.
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let env = Self {
            root: fs::canonicalize(dir.path()).expect("canonicalize"),
            _dir: dir,
            monday: last_monday(),
        };
        fs::create_dir_all(env.path("home")).expect("home");
        fs::create_dir_all(env.path("bin")).expect("bin");
        assert_ok(&env.bdo(&["gain"]));
        let db = rusqlite::Connection::open(env.path("history.db")).expect("open db");
        for (offset, saved) in [(2, 800), (6, 200)] {
            let day = env.monday + Duration::days(offset);
            db.execute(
                "INSERT INTO commands (timestamp, original_cmd, rtk_cmd, input_tokens,
                 output_tokens, saved_tokens, savings_pct) VALUES (?1, 'git status',
                 'bdo git status', ?2, 0, ?3, 80.0)",
                rusqlite::params![format!("{day}T12:00:00+00:00"), saved * 5 / 4, saved],
            )
            .expect("insert");
        }
        env
    }

    fn with_ccusage(self) -> Self {
        let path = self.path("bin/ccusage");
        fs::write(&path, CCUSAGE).expect("write ccusage");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        self
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn day(&self, offset: i64) -> String {
        (self.monday + Duration::days(offset)).to_string()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_bdo"));
        cmd.args(args)
            .env("HOME", self.path("home"))
            .env("BDO_DB_PATH", self.path("history.db"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path("bin").display()),
            )
            .env("ROOT", &self.root)
            .env("CC_DAY", self.day(2))
            .env("CC_MONDAY", self.day(0))
            .env("CC_SUNDAY", self.day(-1))
            .env("CC_MONTH", &self.day(2)[..7])
            .env_remove("CC_FAIL");
        cmd
    }

    fn bdo(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("run bdo")
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.bdo(args);
        assert_ok(&out);
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", text(&out)))
    }

    fn ccusage_calls(&self) -> String {
        fs::read_to_string(self.path("ccusage.log")).unwrap_or_default()
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn assert_ok(out: &Output) {
    assert!(out.status.success(), "{:?}\n{}", out.status, text(out));
}

fn rows(report: &Value, view: &str) -> Vec<Value> {
    report[view]
        .as_array()
        .unwrap_or_else(|| panic!("no {view} in {report}"))
        .clone()
}

/// The single row of a view, holding both sides.
fn joined(report: &Value, view: &str) -> Value {
    let rows = rows(report, view);
    assert_eq!(rows.len(), 1, "{view} should be one period: {rows:#?}");
    let row = rows[0].clone();
    assert_eq!(row["cc_cost"], 1.5, "{view}: {row}");
    assert_eq!(row["rtk_saved_tokens"], 1000, "{view}: {row}");
    row
}

#[test]
fn weekly_joins_spending_and_savings_on_the_same_week() {
    let env = Econ::new().with_ccusage();
    let report = env.json(&["cc-economics", "--weekly", "--format", "json"]);
    let row = joined(&report, "weekly");
    assert_eq!(row["label"], env.day(0), "labelled by the Monday");
    assert_eq!(row["rtk_commands"], 2);
    let savings = row["savings_weighted"].as_f64().expect("savings");
    assert!((savings - 1.0).abs() < 1e-9, "{savings}");
    assert!(
        env.ccusage_calls()
            .contains("weekly --json --since 20250101 --start-of-week monday"),
        "{}",
        env.ccusage_calls()
    );
}

#[test]
fn daily_and_monthly_join_too() {
    let env = Econ::new().with_ccusage();
    let report = env.json(&["cc-economics", "--all", "--format", "json"]);
    let daily = rows(&report, "daily");
    assert_eq!(daily.len(), 2, "{daily:#?}");
    assert_eq!(daily[0]["label"], env.day(2));
    assert_eq!(daily[0]["cc_cost"], 1.5);
    assert_eq!(daily[0]["rtk_saved_tokens"], 800);
    assert_eq!(daily[1]["label"], env.day(6));
    assert!(daily[1]["cc_cost"].is_null(), "{}", daily[1]);
    joined(&report, "weekly");

    // The Sunday can fall in the next month; the Wednesday's month holds the spending.
    let monthly = rows(&report, "monthly");
    let month = &env.day(2)[..7];
    let row = monthly
        .iter()
        .find(|r| r["label"] == month)
        .unwrap_or_else(|| panic!("{month}: {monthly:#?}"));
    assert_eq!(row["cc_cost"], 1.5);
    assert_eq!(report["totals"]["rtk_saved_tokens"], 1000);
    assert_eq!(report["totals"]["cc_cost"], 1.5);
}

#[test]
fn every_view_prints_one_weekly_row_with_savings() {
    let env = Econ::new().with_ccusage();
    let out = env.bdo(&["cc-economics", "--weekly"]);
    assert_ok(&out);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<&str> = stdout
        .lines()
        .filter(|l| l.starts_with(&env.day(-1)[..4]))
        .collect();
    assert_eq!(lines.len(), 1, "{stdout}");
    assert!(lines[0].starts_with(&env.day(0)), "{stdout}");
    assert!(lines[0].contains("$1.00"), "{stdout}");

    let out = env.bdo(&["cc-economics", "--weekly", "--format", "csv"]);
    assert_ok(&out);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    let fields: Vec<&str> = lines[1].split(',').collect();
    assert_eq!(fields.len(), lines[0].split(',').count(), "{stdout}");
    assert_eq!(fields[0], env.day(0));
    assert_eq!(fields[1], "1.5000");
    assert_eq!(fields[9], "1.0000", "weighted savings: {stdout}");
}

#[test]
fn summary_lines_up_its_figures() {
    let env = Econ::new().with_ccusage();
    let out = env.bdo(&["cc-economics"]);
    assert_ok(&out);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let column = |label: &str| {
        let line = stdout
            .lines()
            .find(|l| l.trim_start().starts_with(label))
            .unwrap_or_else(|| panic!("{label}: {stdout}"));
        line.len()
            - line[line.find(label).unwrap() + label.len()..]
                .trim_start()
                .len()
    };
    let spent = column("Spent (ccusage):");
    for label in ["Input:", "Bushido commands:", "Tokens saved:"] {
        assert_eq!(column(label), spent, "{label}\n{stdout}");
    }
}

#[test]
fn without_ccusage_the_savings_still_show() {
    let env = Econ::new();
    let out = env.bdo(&["cc-economics", "--weekly", "--format", "json"]);
    assert_ok(&out);
    assert!(text(&out).contains("ccusage not found"), "{}", text(&out));
    let report: Value = serde_json::from_slice(&out.stdout).expect("json");
    let rows = rows(&report, "weekly");
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0]["label"], env.day(0));
    assert_eq!(rows[0]["rtk_saved_tokens"], 1000);
    assert!(rows[0]["cc_cost"].is_null());
    assert!(rows[0]["savings_weighted"].is_null());
}

#[test]
fn a_failing_ccusage_is_reported_and_the_savings_still_show() {
    let env = Econ::new().with_ccusage();
    let out = env
        .command(&["cc-economics", "--daily"])
        .env("CC_FAIL", "ccusage-says-no")
        .output()
        .expect("run bdo");
    assert_ok(&out);
    let all = text(&out);
    assert!(all.contains("ccusage-says-no"), "{all}");
    assert!(all.contains("800"), "{all}");
}
