//! Exercise rules export through the real CLI with isolated session history.
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

const RULES_PATH: &str = ".claude/rules/cli-corrections.md";

struct Fixture {
    _temp: tempfile::TempDir,
    project: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        fs::create_dir(&project).unwrap();
        let sessions = home.join("claude/projects/fixture");
        fs::create_dir_all(&sessions).unwrap();
        let history = [
            serde_json::json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"bad","name":"Bash","input":{"command":"git commit --ammend"}}]}}),
            serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"bad","is_error":true,"content":"error: unexpected argument '--ammend'"}]}}),
            serde_json::json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"good","name":"Bash","input":{"command":"git commit --amend"}}]}}),
            serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"good","is_error":false,"content":"commit amended"}]}}),
        ].iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
        fs::write(sessions.join("session.jsonl"), history).unwrap();
        Self {
            _temp: temp,
            project,
            home,
        }
    }

    fn export(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bdo"))
            .args(["learn", "--all", "--write-rules"])
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CLAUDE_CONFIG_DIR", self.home.join("claude"))
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("BDO_DB_PATH", self.home.join("history.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env("BDO_TEE", "0")
            .env_remove("BDO_RAW")
            .output()
            .unwrap()
    }
}

#[test]
fn learn_creates_and_updates_project_rules() {
    let fixture = Fixture::new();
    let output = fixture.export();
    assert!(output.status.success(), "{output:?}");
    let target = fixture.project.join(RULES_PATH);
    let content = fs::read_to_string(&target).unwrap();
    assert!(content.contains("Use `git commit --amend` not `git commit --ammend`"));
    fs::write(&target, "obsolete content".repeat(100)).unwrap();
    let output = fixture.export();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(&target).unwrap(), content);
}

#[cfg(unix)]
#[test]
fn learn_rejects_external_file_link_without_changing_its_target() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let victim = fixture.home.join("victim.md");
    fs::write(&victim, "outside sentinel").unwrap();
    let target = fixture.project.join(RULES_PATH);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    symlink(&victim, &target).unwrap();
    let output = fixture.export();
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Refusing project write"));
    assert_eq!(fs::read_to_string(&victim).unwrap(), "outside sentinel");
    assert!(fs::symlink_metadata(&target)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(any(unix, windows))]
#[test]
fn learn_rejects_parent_directory_links_before_any_external_write() {
    for component in [".claude", ".claude/rules"] {
        let fixture = Fixture::new();
        let outside = fixture.home.join("outside");
        fs::create_dir(&outside).unwrap();
        // Join each part, so Windows gets `\` separators: `mklink` reads a
        // `/` inside its link argument as the start of a switch.
        let link = component
            .split('/')
            .fold(fixture.project.clone(), |path, part| path.join(part));
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        // Junctions do not require Windows developer-mode symlink privileges.
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
        let output = fixture.export();
        assert!(!output.status.success(), "{component}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("Refusing project write"));
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    }
}
