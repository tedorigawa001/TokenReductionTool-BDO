//! Security regressions for the real stale/review file-reading paths.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Fixture {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let home = temp.path().join("home");
        fs::create_dir(&repo).unwrap();
        fs::create_dir(&home).unwrap();
        let fixture = Self {
            _temp: temp,
            repo,
            home,
        };
        fixture.git(&["init", "--template=", "-q"]);
        fixture
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.home.join("no-git-config"))
            .env("BDO_DB_PATH", self.home.join("history.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env("BDO_TEE", "0")
            .env_remove("BDO_RAW");
        command
    }

    fn git(&self, args: &[&str]) {
        let output = self.command("git").args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
    }

    fn run(&self, command: &str) -> Output {
        self.run_at(command, &self.repo)
    }

    fn run_at(&self, command: &str, cwd: &Path) -> Output {
        self.run_args_at(&[command], cwd)
    }

    fn run_args_at(&self, args: &[&str], cwd: &Path) -> Output {
        let mut child = self
            .command(env!("CARGO_BIN_EXE_bdo"))
            .args(args)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                return child.wait_with_output().unwrap();
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!("{args:?} blocked while scanning an unsafe file: {output:?}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn track(&self, path: &str, content: &str) {
        fs::write(self.repo.join(path), content).unwrap();
        self.git(&["add", "--", path]);
    }
}

#[cfg(unix)]
#[test]
fn stale_and_review_skip_tracked_fifo_without_opening_it() {
    for command in ["stale", "review"] {
        let fixture = Fixture::new();
        fixture.track("pipe.txt", "ordinary file\n");
        fs::remove_file(fixture.repo.join("pipe.txt")).unwrap();
        let output = Command::new("mkfifo")
            .arg(fixture.repo.join("pipe.txt"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let output = fixture.run(command);
        assert!(output.status.success(), "{command}: {output:?}");
    }
}

#[test]
fn normal_markers_and_markdown_drift_are_still_detected() {
    let fixture = Fixture::new();
    fixture.track(
        "doc.md",
        "ordinary line\ncargo install bdo\nSee `bdo removedcommand`.\n",
    );
    let output = fixture.run("stale");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stdout.contains("STALE MARKERS (1)"), "{stdout}");
    assert!(stdout.contains("doc.md:2"), "{stdout}");
    assert!(stdout.contains("DOC↔IMPL DRIFT (1)"), "{stdout}");
    let output = fixture.run("review");
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("doc.md:2"));
}

#[test]
fn oversized_regular_files_are_skipped_in_full() {
    let fixture = Fixture::new();
    let content = format!("cargo install bdo\n{}", "x".repeat(1024 * 1024));
    fixture.track("large.txt", &content);
    for command in ["stale", "review"] {
        let output = fixture.run(command);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{command}: {output:?}");
        assert!(stdout.contains("STALE MARKERS (0)"), "{stdout}");
        assert!(
            stdout.contains("1 file(s) skipped for content scanning"),
            "{stdout}"
        );
    }
}

#[cfg(unix)]
#[test]
fn file_links_to_devices_or_external_text_are_not_followed() {
    let fixture = Fixture::new();
    let victim = fixture.home.join("outside.txt");
    fs::write(&victim, "cargo install bdo\n").unwrap();
    for (name, target) in [
        ("text.txt", victim.as_path()),
        ("zero.txt", Path::new("/dev/zero")),
        ("dangling.txt", fixture.home.join("missing").as_path()),
    ] {
        std::os::unix::fs::symlink(target, fixture.repo.join(name)).unwrap();
        fixture.git(&["add", "--", name]);
    }
    for command in ["stale", "review"] {
        let output = fixture.run(command);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{command}: {output:?}");
        assert!(stdout.contains("STALE MARKERS (0)"), "{stdout}");
        assert!(
            stdout.contains("3 file(s) skipped for content scanning"),
            "{stdout}"
        );
    }
}

#[cfg(unix)]
#[test]
fn ignore_file_fifo_and_device_link_cannot_block_either_command() {
    for device_link in [false, true] {
        let fixture = Fixture::new();
        fixture.track("doc.md", "cargo install bdo\n");
        let ignore = fixture.repo.join(".bdostaleignore");
        if device_link {
            std::os::unix::fs::symlink("/dev/zero", &ignore).unwrap();
        } else {
            assert!(Command::new("mkfifo")
                .arg(&ignore)
                .status()
                .unwrap()
                .success());
        }
        for command in ["stale", "review"] {
            let output = fixture.run(command);
            assert_eq!(
                output.status.code(),
                Some(if command == "stale" { 1 } else { 0 }),
                "{output:?}"
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("doc.md:1"));
        }
    }
}

#[test]
fn both_audits_read_repo_relative_paths_from_a_subdirectory() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.repo.join("sub")).unwrap();
    fixture.track("root.md", "cargo install bdo\n");
    for command in ["stale", "review"] {
        let output = fixture.run_at(command, &fixture.repo.join("sub"));
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("root.md:1"),
            "{output:?}"
        );
    }
}

#[cfg(any(unix, windows))]
#[test]
fn parent_directory_links_and_windows_junctions_are_skipped() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.repo.join("parent")).unwrap();
    fixture.track("parent/doc.md", "ordinary content\n");
    fs::remove_dir_all(fixture.repo.join("parent")).unwrap();
    let outside = fixture.home.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("doc.md"), "cargo install bdo\n").unwrap();
    let link = fixture.repo.join("parent");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    #[cfg(windows)]
    {
        let output = Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    for command in ["stale", "review"] {
        let output = fixture.run(command);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{command}: {output:?}");
        assert!(stdout.contains("STALE MARKERS (0)"), "{stdout}");
        assert!(stdout.contains("skipped for content scanning"), "{stdout}");
    }
}

#[test]
fn directories_replacing_tracked_files_are_skipped() {
    let fixture = Fixture::new();
    fixture.track("directory.txt", "old\n");
    fs::remove_file(fixture.repo.join("directory.txt")).unwrap();
    fs::create_dir(fixture.repo.join("directory.txt")).unwrap();
    for command in ["stale", "review"] {
        let output = fixture.run(command);
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("STALE MARKERS (0)"));
    }
}

#[cfg(unix)]
#[test]
fn tracked_unix_sockets_are_skipped() {
    let fixture = Fixture::new();
    fixture.track("socket.txt", "old\n");
    fs::remove_file(fixture.repo.join("socket.txt")).unwrap();
    // sockaddr_un has a short address limit. Bind through a relative path,
    // without changing process-global cwd or leaving the isolated fixture.
    let cwd = std::env::current_dir().unwrap();
    let common = cwd
        .ancestors()
        .find(|base| fixture.repo.starts_with(base))
        .unwrap();
    let mut relative = PathBuf::new();
    for _ in cwd.strip_prefix(common).unwrap().components() {
        relative.push("..");
    }
    relative.push(fixture.repo.strip_prefix(common).unwrap());
    relative.push("socket.txt");
    let _listener = std::os::unix::net::UnixListener::bind(relative).unwrap();
    for command in ["stale", "review"] {
        let output = fixture.run(command);
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("STALE MARKERS (0)"));
    }
}

#[test]
fn review_against_uses_root_relative_paths_even_with_relative_diff_configuration() {
    let fixture = Fixture::new();
    fixture.git(&[
        "-c",
        "user.email=t@example.invalid",
        "-c",
        "user.name=test",
        "commit",
        "--allow-empty",
        "-qm",
        "baseline",
    ]);
    fixture.git(&["config", "diff.relative", "true"]);
    fs::create_dir(fixture.repo.join("sub")).unwrap();
    fixture.track("sub/doc.md", "cargo install bdo\n");
    fs::write(fixture.repo.join("sub/untracked.md"), "cargo install bdo\n").unwrap();
    let output = fixture.run_args_at(&["review", "--against", "HEAD"], &fixture.repo.join("sub"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{output:?}");
    assert!(stdout.contains("sub/doc.md:1"), "{stdout}");
    assert!(stdout.contains("sub/untracked.md:1"), "{stdout}");
}
