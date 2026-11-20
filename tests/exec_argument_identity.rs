//! bdo must run what it was asked to run, with the arguments it was given.
//!
//! - `bdo summary` and `bdo run` with several arguments pass them as they are.
//!   Joining them for `sh -c` turned a quoted file name such as
//!   `a$(touch x).txt` back into shell syntax. A single argument is still a
//!   shell command string, as before.
//! - When mypy is not installed, the `python3 -m mypy` fallback must not put
//!   the working directory on the module path: a repository's `mypy.py`
//!   would run instead.
//! - When tsc, next or prisma is not installed, the npx fallback must not
//!   download a package of that name; `npx --no-install` runs only what is
//!   already installed. `bdo test --changed` builds its npx command the same way.
//!
//! Unix only: the stand-in tools are shell scripts.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _temp: tempfile::TempDir,
    work: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (work, home, bin) = (root.join("work"), root.join("home"), root.join("bin"));
        for dir in [&work, &home, &bin] {
            fs::create_dir(dir).unwrap();
        }
        Self {
            _temp: temp,
            work,
            home,
            bin,
            log: root.join("argv"),
        }
    }

    /// A stand-in that records its arguments, one per line.
    fn recorder(&self, name: &str) {
        let path = self.bin.join(name);
        fs::write(
            &path,
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$BDO_TEST_ARGV\"\n",
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn link(&self, name: &str, target: &Path) {
        std::os::unix::fs::symlink(target, self.bin.join(name)).unwrap();
    }

    fn bdo(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bdo"))
            .args(args)
            .current_dir(&self.work)
            .env_clear()
            // Only the stand-ins and the basic system tools: no real mypy,
            // tsc, next, prisma or npx.
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("HOME", &self.home)
            .env("BDO_DB_PATH", self.home.join("history.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env("BDO_TEE", "0")
            .env("BDO_TEST_ARGV", &self.log)
            .envs(std::env::var("LLVM_PROFILE_FILE").map(|p| ("LLVM_PROFILE_FILE", p)))
            .output()
            .unwrap()
    }

    fn argv(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

#[test]
fn summary_and_run_pass_several_arguments_as_they_are() {
    let fixture = Fixture::new();
    let name = "a$(touch injected).txt";
    fs::write(fixture.work.join(name), "").unwrap();

    let output = fixture.bdo(&["summary", "ls", "-l", name]);
    assert!(output.status.success(), "{output:?}");
    let output = fixture.bdo(&["run", "ls", "b;touch injected", name]);
    assert!(
        !output.status.success(),
        "ls of a missing file fails: {output:?}"
    );
    assert!(!fixture.work.join("injected").exists());

    fixture.recorder("tool");
    let output = fixture.bdo(&["run", "tool", "a b", "$HOME", "*"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fixture.argv(), ["a b", "$HOME", "*"]);
}

#[test]
fn summary_and_run_still_take_one_argument_as_a_shell_command() {
    let fixture = Fixture::new();
    let output = fixture.bdo(&["summary", "echo one; echo two"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("one") && stdout.contains("two"), "{stdout}");

    fixture.bdo(&["run", "touch first && touch second"]);
    fixture.bdo(&["run", "-c", "touch third"]);
    for name in ["first", "second", "third"] {
        assert!(fixture.work.join(name).exists(), "{name}");
    }
}

#[test]
fn the_mypy_fallback_does_not_import_from_the_working_directory() {
    let Some(python) = [
        "/usr/bin/python3",
        "/usr/local/bin/python3",
        "/opt/homebrew/bin/python3",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.exists()) else {
        eprintln!("skipped: no python3");
        return;
    };
    let fixture = Fixture::new();
    fixture.link("python3", &python);
    fs::write(
        fixture.work.join("mypy.py"),
        "open('repository-mypy-ran', 'w').write('x')\n",
    )
    .unwrap();
    fixture.bdo(&["mypy", "src"]);
    assert!(!fixture.work.join("repository-mypy-ran").exists());
}

#[test]
fn npx_fallbacks_never_install_a_package() {
    let fixture = Fixture::new();
    fixture.recorder("npx");
    for (args, tool) in [
        (vec!["tsc"], "tsc"),
        (vec!["next", "build"], "next"),
        (vec!["prisma", "generate"], "prisma"),
    ] {
        fixture.bdo(&args);
        let argv = fixture.argv();
        assert_eq!(
            argv.get(..3),
            Some(&["--no-install".to_string(), "--".into(), tool.into()][..]),
            "{args:?}: {argv:?}"
        );
    }
}

#[test]
fn test_changed_runs_installed_js_runners_only() {
    let fixture = Fixture::new();
    fixture.recorder("npx");
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&fixture.work)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", fixture.home.join("no-git-config"))
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "--template="]);
    fs::write(fixture.work.join("app.ts"), "export const a = 1;\n").unwrap();
    git(&["add", "app.ts"]);
    fixture.bdo(&["test", "--changed"]);
    let argv = fixture.argv();
    assert_eq!(
        argv.get(..2),
        Some(&["--no-install".to_string(), "--".into()][..]),
        "{argv:?}"
    );
    assert_eq!(argv.get(2).map(String::as_str), Some("vitest"), "{argv:?}");
}
