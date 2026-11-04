//! `bdo env` output goes to an LLM, so a secret must not reach it in clear,
//! whatever the variable is called. All values are fake.

use std::process::Command;

fn bdo_env(vars: &[(&str, &str)], args: &[&str]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bdo"));
    cmd.current_dir(dir.path())
        .arg("env")
        .args(args)
        .env_clear()
        .env("BDO_DB_PATH", dir.path().join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1");
    // Windows cannot start a process without these.
    for key in ["SystemRoot", "SYSTEMROOT", "TEMP", "TMP"] {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
    cmd.envs(vars.iter().copied());
    cmd.envs(std::env::var("LLVM_PROFILE_FILE").map(|p| ("LLVM_PROFILE_FILE", p)));
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const SECRET_VARS: &[(&str, &str)] = &[
    ("DATABASE_URL", "postgres://user:fakevalue1@host/db"),
    ("REDIS_URL", "redis://:fakevalue2@host:6379"),
    ("SENTRY_DSN", "https://fakevalue3@sentry.io/1"),
    ("SESSION_COOKIE", "fakevalue4"),
    ("DB_PASSWORD", "fakevalue5"),
    ("MYSQL_PWD", "fakevalue6"),
    ("MY_API_KEY", "fakevalue7"),
    ("GH_TOKEN", "fakevalue8"),
    ("AWS_SECRET_ACCESS_KEY", "fakevalue9"),
];

#[test]
fn no_secret_reaches_the_output() {
    // `-f _` selects them all, past the "relevant variables" filter.
    let out = bdo_env(SECRET_VARS, &["-f", "_"]);
    for (key, _) in SECRET_VARS {
        assert!(out.contains(key), "{key} not listed: {out}");
    }
    assert!(!out.contains("fakevalue"), "{out}");
}

#[test]
fn a_known_token_format_is_redacted_under_any_name() {
    let token = format!("ghp_{}", "a".repeat(36));
    let out = bdo_env(&[("X_THING", &token)], &["-f", "X_THING"]);
    assert!(out.contains("X_THING"), "{out}");
    assert!(!out.contains(&token), "{out}");
}

#[test]
fn a_long_url_is_redacted_before_being_cut() {
    let url = format!("postgres://user:fakevalue1@host/{}", "d".repeat(200));
    let out = bdo_env(&[("DATABASE_URL", &url)], &["-f", "DATABASE"]);
    assert!(!out.contains("fakevalue"), "{out}");
    assert!(out.contains("chars)"), "{out}");
}

#[test]
fn ordinary_values_stay_readable() {
    let out = bdo_env(
        &[
            ("APP_MODE", "production"),
            ("SITE_URL", "https://example.com/a"),
        ],
        &["-f", "_"],
    );
    assert!(out.contains("APP_MODE=production"), "{out}");
    assert!(out.contains("SITE_URL=https://example.com/a"), "{out}");
}

#[test]
fn show_all_is_the_explicit_way_to_see_values() {
    let out = bdo_env(
        &[("DB_PASSWORD", "fakevalue5")],
        &["-f", "PASSWORD", "--show-all"],
    );
    assert!(out.contains("DB_PASSWORD=fakevalue5"), "{out}");
}
