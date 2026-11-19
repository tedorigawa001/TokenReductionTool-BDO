//! Files a repository controls, read without being asked to: the hook's
//! project settings on every command, and the manifests and sources that
//! `deps`, `map --changed`, `test-changed` and `format` inspect.
//!
//! A FIFO must not block them, a link must not pull in a file from outside
//! the repository, and an oversized file is skipped rather than read. A
//! settings file that is a link to a regular file is a legitimate setup and
//! still applies. Unix only: FIFOs and these links are made with Unix tools.
#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Fixture {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    outside: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (repo, home, outside) = (root.join("repo"), root.join("home"), root.join("outside"));
        for dir in [&repo, &home, &outside] {
            fs::create_dir(dir).unwrap();
        }
        let fixture = Self {
            _temp: temp,
            repo,
            home,
            outside,
        };
        fixture.git(&["init", "--template=", "-q"]);
        fixture
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.repo)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.home.join("no-git-config"))
            .env("BDO_DB_PATH", self.home.join("history.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env("BDO_TEE", "0")
            .envs(std::env::var("LLVM_PROFILE_FILE").map(|p| ("LLVM_PROFILE_FILE", p)));
        command
    }

    fn git(&self, args: &[&str]) {
        let output = self.command("git").args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
    }

    fn fifo(&self, relative: &str) {
        let path = self.repo.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let output = Command::new("mkfifo").arg(&path).output().unwrap();
        assert!(output.status.success(), "{output:?}");
    }

    /// A file outside the repository, and a link to it at `relative`.
    fn link_outside(&self, relative: &str, content: &str) {
        let name = Path::new(relative).file_name().unwrap();
        let target = self.outside.join(name);
        fs::write(&target, content).unwrap();
        let link = self.repo.join(relative);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&target, link).unwrap();
    }

    /// Run bdo, failing the test if it is still running after 5 seconds.
    fn bdo(&self, args: &[&str], stdin: &[u8]) -> Output {
        let mut child = self
            .command(env!("CARGO_BIN_EXE_bdo"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child.stdin.take().unwrap().write_all(stdin);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                return child.wait_with_output().unwrap();
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!("{args:?} blocked on a repository file: {output:?}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn hook(&self, command: &str) -> serde_json::Value {
        let payload = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": command },
            "cwd": self.repo,
        });
        let output = self.bdo(&["hook", "claude"], payload.to_string().as_bytes());
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str(stdout.trim()).unwrap_or(serde_json::Value::Null)
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn decision(answer: &serde_json::Value) -> Option<&str> {
    answer
        .pointer("/hookSpecificOutput/permissionDecision")
        .and_then(|v| v.as_str())
}

#[test]
fn hook_settings_that_are_fifos_do_not_stall_the_hook() {
    for name in ["settings.json", "settings.local.json"] {
        let fixture = Fixture::new();
        fixture.fifo(&format!(".claude/{name}"));
        let answer = fixture.hook("git status");
        assert_eq!(
            answer
                .pointer("/hookSpecificOutput/updatedInput/command")
                .and_then(|v| v.as_str()),
            Some("bdo git status"),
            "{name}: {answer}"
        );
    }
}

#[test]
fn hook_settings_reached_through_a_link_still_apply() {
    let fixture = Fixture::new();
    let allow = serde_json::json!({ "permissions": { "allow": ["Bash(git status)"] } });
    fixture.link_outside(".claude/settings.json", &allow.to_string());
    assert_eq!(decision(&fixture.hook("git status")), Some("allow"));
}

#[test]
fn oversized_hook_settings_are_skipped_in_full() {
    let fixture = Fixture::new();
    let allow = serde_json::json!({
        "permissions": { "allow": ["Bash(git status)"] },
        "padding": "x".repeat(1024 * 1024),
    });
    fs::create_dir(fixture.repo.join(".claude")).unwrap();
    fs::write(
        fixture.repo.join(".claude/settings.json"),
        allow.to_string(),
    )
    .unwrap();
    assert_ne!(decision(&fixture.hook("git status")), Some("allow"));
}

#[test]
fn deps_skips_manifests_that_are_fifos_or_links_out_of_the_repository() {
    let fixture = Fixture::new();
    fixture.fifo("package.json");
    fixture.link_outside(
        "Cargo.toml",
        "[package]\nname = \"x\"\n[dependencies]\nleaked-secret-dep = \"1\"\n",
    );
    fs::write(
        fixture.repo.join("go.mod"),
        "module example.com/x\n\nrequire example.com/kept v1.0.0\n",
    )
    .unwrap();

    let output = fixture.bdo(&["deps"], b"");
    let out = stdout(&output);
    assert!(!out.contains("leaked-secret-dep"), "{out}");
    assert!(out.contains("example.com/kept"), "{out}");
}

#[test]
fn map_changed_does_not_follow_links_out_of_the_repository() {
    let fixture = Fixture::new();
    fixture.link_outside("leak.rs", "pub fn leaked_secret_fn() {}\n");
    fs::write(fixture.repo.join("kept.rs"), "pub fn kept_fn() {}\n").unwrap();

    let output = fixture.bdo(&["map", "--changed"], b"");
    let out = stdout(&output);
    assert!(!out.contains("leaked_secret_fn"), "{out}");
    assert!(out.contains("kept_fn"), "{out}");
}
