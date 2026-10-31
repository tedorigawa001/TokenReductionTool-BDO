//! `bdo grep` must never let ripgrep run a program.
//!
//! bdo translates grep to rg and forwards extra arguments. rg's `--pre <cmd>`
//! runs `<cmd>` on every searched file (and `--hostname-bin` runs a command),
//! so a hook-rewritten, auto-allowed `grep -rn x . --pre rm` deleted the
//! files it searched. Native grep has no such options — it just fails.
//!
//! The PATH holds a copy of the real rg and nothing else (no grep), so every
//! case reaches bdo's own grep module — including `--raw`, which otherwise
//! hands `grep` to the system grep. The "program" is `rm`: if any case lets
//! rg run it, the canary file is gone.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    dir: tempfile::TempDir,
    bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let rg = which::which("rg").expect(
            "grep_exec_flags tests require ripgrep on PATH; install with cargo install ripgrep --locked --version 15.1.0",
        );
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        fs::copy(rg, bin.join("rg")).unwrap();
        // `rm` resolves to the real one: that is what rg would run.
        Self { dir, bin }
    }

    fn work(&self) -> &Path {
        self.dir.path()
    }

    fn canary(&self) -> PathBuf {
        let path = self.work().join("canary.txt");
        fs::write(&path, "needle --pre\n").unwrap();
        path
    }

    fn bdo(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bdo"))
            .current_dir(self.work())
            .args(args)
            .env("PATH", &self.bin)
            .env("BDO_DB_PATH", self.work().join("track.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env_remove("BDO_RAW")
            .env_remove("RIPGREP_CONFIG_PATH")
            .output()
            .expect("run bdo")
    }
}

#[test]
fn command_running_rg_options_are_refused_and_nothing_runs() {
    let f = Fixture::new();
    for args in [
        &["grep", "needle", "canary.txt", "--pre", "/bin/rm"][..],
        &["grep", "needle", "canary.txt", "--pre=/bin/rm"][..],
        &["grep", "needle", ".", "--pre-glob", "*", "--pre", "/bin/rm"][..],
        &["grep", "needle", ".", "--hostname-bin=/bin/rm"][..],
        &["--raw", "grep", "needle", "canary.txt", "--pre", "/bin/rm"][..],
    ] {
        let canary = f.canary();
        let out = f.bdo(args);
        assert!(canary.exists(), "{args:?} let rg run `rm`");
        assert!(!out.status.success(), "{args:?} should fail");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("refusing ripgrep option"),
            "{args:?}: {stderr}"
        );
    }
}

#[test]
fn a_pattern_that_looks_like_the_option_is_still_searched() {
    let f = Fixture::new();
    let canary = f.canary();
    for args in [
        &["grep", "--", "--pre", "canary.txt"][..],
        &["--raw", "grep", "--", "--pre", "canary.txt"][..],
    ] {
        let out = f.bdo(args);
        assert!(canary.exists());
        assert!(out.status.success(), "{args:?}: {:?}", out);
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("needle --pre"),
            "{args:?}"
        );
    }
}
