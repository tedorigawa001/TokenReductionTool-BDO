//! Exercise the no-grep fallback with a real rg on Linux, macOS and Windows.
//! Do not gate this module on cfg(unix): native Windows is the regression case.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    dir: tempfile::TempDir,
    bin: PathBuf,
    rg: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let installed_rg = which::which("rg")
            .expect("raw_grep tests require ripgrep on PATH; install with cargo install ripgrep --locked --version 15.1.0");
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let rg = bin.join(format!("rg{}", std::env::consts::EXE_SUFFIX));
        // Copy the native executable: no Unix shell scripts, symlinks or chmod
        // are needed on Windows. The child's PATH contains rg and nothing else.
        fs::copy(installed_rg, &rg).unwrap();
        assert!(which::which_in("grep", Some(&bin), dir.path()).is_err());
        let body: String = (1..=300)
            .map(|n| format!("needle {n} {}\n", "x".repeat(150)))
            .collect();
        fs::write(dir.path().join("input with spaces.rs"), body).unwrap();
        Self { dir, bin, rg }
    }

    fn bdo(&self, args: &[&str], raw_env: bool) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_bdo"));
        cmd.current_dir(self.dir.path())
            .args(args)
            .env("PATH", &self.bin)
            .env("BDO_DB_PATH", self.dir.path().join("track.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env("BDO_QUIET", "1")
            .env_remove("RIPGREP_CONFIG_PATH")
            .env_remove("BDO_RAW");
        if raw_env {
            cmd.env("BDO_RAW", "1");
        }
        cmd.output().expect("run bdo grep with isolated PATH")
    }

    fn expected(&self, pattern: &str) -> Output {
        Command::new(&self.rg)
            .current_dir(self.dir.path())
            .args([
                "--no-heading",
                "--no-ignore-vcs",
                "-nH",
                pattern,
                "input with spaces.rs",
            ])
            .env_remove("RIPGREP_CONFIG_PATH")
            .output()
            .expect("run reference rg")
    }
}

fn assert_same_output(actual: &Output, expected: &Output) {
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stderr, expected.stderr);
    assert!(
        actual.stdout == expected.stdout,
        "stdout changed: got {} bytes / {} lines, expected {} bytes / {} lines",
        actual.stdout.len(),
        String::from_utf8_lossy(&actual.stdout).lines().count(),
        expected.stdout.len(),
        String::from_utf8_lossy(&expected.stdout).lines().count(),
    );
}

#[test]
fn raw_grep_without_native_grep_keeps_every_match_in_full() {
    let fixture = Fixture::new();
    let expected = fixture.expected("needle");
    assert!(expected.status.success());
    assert_eq!(
        String::from_utf8_lossy(&expected.stdout).lines().count(),
        300
    );
    let actual = fixture.bdo(&["--raw", "grep", "needle", "input with spaces.rs"], false);
    assert_same_output(&actual, &expected);
}

#[test]
fn raw_grep_flag_and_environment_ignore_reduction_options() {
    let fixture = Fixture::new();
    let expected = fixture.expected("needle");
    for raw_env in [false, true] {
        let mut args = vec!["grep", "needle", "input with spaces.rs"];
        if !raw_env {
            args.insert(0, "--raw");
        }
        args.extend(["--max", "5", "--max-len", "10", "--context-only", "--all"]);
        assert_same_output(&fixture.bdo(&args, raw_env), &expected);
    }
}

#[test]
fn raw_grep_keeps_no_match_and_regex_error_output_and_exit_codes() {
    let fixture = Fixture::new();
    for (pattern, code) in [("absent", 1), ("[", 2)] {
        let expected = fixture.expected(pattern);
        assert_eq!(expected.status.code(), Some(code));
        let actual = fixture.bdo(&["--raw", "grep", pattern, "input with spaces.rs"], false);
        assert_same_output(&actual, &expected);
    }
}

#[test]
fn normal_grep_still_reduces_the_same_search() {
    let fixture = Fixture::new();
    let normal = fixture.bdo(&["grep", "needle", "input with spaces.rs"], false);
    assert!(normal.status.success());
    let output = String::from_utf8_lossy(&normal.stdout);
    assert!(output.contains("more"), "normal mode must retain its cap");
    assert!(
        !output.contains(&"x".repeat(150)),
        "normal mode must still shorten long lines"
    );
}
