//! The hook must never hand back a weaker decision than the host would reach
//! on the original command by its own rules.
//!
//! - An `ask` rule still asks after the rewrite; the host would otherwise
//!   match its rules against `bdo …` and could run it without a prompt.
//! - A bare `Bash` rule covers every command.
//! - `Bash(git push)` allows exactly `git push`, not `git push origin main`.
//! - Project settings come from the session's project directory
//!   (`CLAUDE_PROJECT_DIR`), not from the nearest `.claude/` below it.
//! - Gemini has no `ask` value, so a command under `confirmationRequired` is
//!   left as written for Gemini to confirm.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

struct Fixture {
    _temp: tempfile::TempDir,
    project: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (project, home) = (root.join("project"), root.join("home"));
        fs::create_dir(&project).unwrap();
        fs::create_dir(&home).unwrap();
        Self {
            _temp: temp,
            project,
            home,
        }
    }

    fn settings(&self, dir: &Path, permissions: Value) {
        fs::create_dir_all(dir.join(".claude")).unwrap();
        let settings = json!({ "permissions": permissions });
        fs::write(dir.join(".claude/settings.json"), settings.to_string()).unwrap();
    }

    fn run(&self, args: &[&str], cwd: &Path, project_dir: Option<&Path>, stdin: Value) -> Value {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bdo"));
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("BDO_DB_PATH", self.home.join("history.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .envs(std::env::var("LLVM_PROFILE_FILE").map(|p| ("LLVM_PROFILE_FILE", p)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = project_dir {
            command.env("CLAUDE_PROJECT_DIR", dir);
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(stdout.trim()).expect("hook must return valid JSON")
        }
    }

    fn claude(&self, command: &str, cwd: &Path, project_dir: Option<&Path>) -> Value {
        let payload = json!({
            "tool_name": "Bash",
            "tool_input": { "command": command },
            "cwd": cwd,
        });
        self.run(&["hook", "claude"], cwd, project_dir, payload)
    }
}

fn decision(answer: &Value) -> Option<&str> {
    answer
        .pointer("/hookSpecificOutput/permissionDecision")
        .and_then(Value::as_str)
}

fn rewritten(answer: &Value) -> Option<&str> {
    answer
        .pointer("/hookSpecificOutput/updatedInput/command")
        .and_then(Value::as_str)
}

#[test]
fn an_ask_rule_still_asks_after_the_rewrite() {
    let fixture = Fixture::new();
    fixture.settings(
        &fixture.project,
        json!({ "allow": ["Bash(*)"], "ask": ["Bash(git commit:*)"] }),
    );
    let answer = fixture.claude("git commit -m x", &fixture.project, None);
    assert_eq!(rewritten(&answer), Some("bdo git commit -m x"), "{answer}");
    assert_eq!(decision(&answer), Some("ask"), "{answer}");
    // Commands outside the ask rule keep their allow.
    let answer = fixture.claude("git status", &fixture.project, None);
    assert_eq!(decision(&answer), Some("allow"), "{answer}");
}

#[test]
fn claude_leaves_a_command_without_a_matching_rule_unchanged() {
    let fixture = Fixture::new();
    fixture.settings(&fixture.project, json!({ "allow": ["Bash(git status)"] }));
    let answer = fixture.claude("git status --short", &fixture.project, None);
    assert_eq!(answer, Value::Null, "{answer}");
}

#[test]
fn a_bare_bash_rule_covers_every_command() {
    for (key, expected) in [("ask", Some("ask")), ("deny", None)] {
        let fixture = Fixture::new();
        fixture.settings(
            &fixture.project,
            json!({ "allow": ["Bash(git status:*)"], key: ["Bash"] }),
        );
        let answer = fixture.claude("git status", &fixture.project, None);
        assert_ne!(decision(&answer), Some("allow"), "{key}: {answer}");
        if rewritten(&answer).is_some() {
            assert_eq!(decision(&answer), expected, "{key}: {answer}");
        }
    }

    let fixture = Fixture::new();
    fixture.settings(&fixture.project, json!({ "allow": ["Bash"] }));
    let answer = fixture.claude("git status", &fixture.project, None);
    assert_eq!(decision(&answer), Some("allow"), "{answer}");
}

#[test]
fn an_allow_rule_without_a_wildcard_allows_only_that_command() {
    let fixture = Fixture::new();
    fixture.settings(&fixture.project, json!({ "allow": ["Bash(git push)"] }));
    let answer = fixture.claude("git push", &fixture.project, None);
    assert_eq!(decision(&answer), Some("allow"), "{answer}");
    for command in ["git push origin main", "git push --force-with-lease"] {
        let answer = fixture.claude(command, &fixture.project, None);
        assert_ne!(decision(&answer), Some("allow"), "{command}: {answer}");
    }
}

#[test]
fn a_deny_or_ask_rule_without_a_wildcard_still_covers_longer_commands() {
    let fixture = Fixture::new();
    fixture.settings(
        &fixture.project,
        json!({ "allow": ["Bash(*)"], "ask": ["Bash(git push)"] }),
    );
    let answer = fixture.claude("git push origin main", &fixture.project, None);
    assert_ne!(decision(&answer), Some("allow"), "{answer}");
}

#[test]
fn settings_come_from_the_project_directory_not_a_nested_claude_directory() {
    let fixture = Fixture::new();
    fixture.settings(&fixture.project, json!({ "ask": ["Bash(git push:*)"] }));
    let sub = fixture.project.join("vendor/pkg");
    fixture.settings(&sub, json!({ "allow": ["Bash(git push:*)"] }));
    let answer = fixture.claude("git push origin main", &sub, Some(&fixture.project));
    assert_ne!(decision(&answer), Some("allow"), "{answer}");

    // The project directory's own allow rules still apply from a subdirectory.
    let fixture = Fixture::new();
    fixture.settings(&fixture.project, json!({ "allow": ["Bash(git status:*)"] }));
    let sub = fixture.project.join("src");
    fs::create_dir(&sub).unwrap();
    let answer = fixture.claude("git status", &sub, Some(&fixture.project));
    assert_eq!(decision(&answer), Some("allow"), "{answer}");
}

// Gemini's settings live in the home directory, which `dirs` takes from the OS
// on Windows rather than from HOME or USERPROFILE, so the fixture's home is
// not seen there (as in tests/hook_agents.rs).
#[cfg(unix)]
#[test]
fn gemini_leaves_commands_that_need_confirmation_as_written() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.home.join(".gemini")).unwrap();
    let settings = json!({ "tools": {
        "allowed": ["run_shell_command(git)"],
        "confirmationRequired": ["run_shell_command(git commit)"],
    }});
    fs::write(
        fixture.home.join(".gemini/settings.json"),
        settings.to_string(),
    )
    .unwrap();
    let payload = |command: &str| {
        json!({
            "tool_name": "run_shell_command",
            "tool_input": { "command": command },
            "cwd": fixture.project,
        })
    };
    let answer = fixture.run(
        &["hook", "gemini"],
        &fixture.project,
        None,
        payload("git commit -m x"),
    );
    assert_eq!(answer, json!({}), "{answer}");

    let answer = fixture.run(
        &["hook", "gemini"],
        &fixture.project,
        None,
        payload("git status"),
    );
    assert_ne!(answer, json!({}), "an allowed command is still rewritten");
}

#[test]
fn a_prefix_before_the_command_does_not_hide_it_from_deny_or_ask_rules() {
    // `FOO=1 git push`, `env git push` and `sudo git push` run `git push`;
    // the hook rewrites them, so it must judge them as `git push`.
    for prefix in ["FOO=1 ", "GIT_TRACE=0 ", "env ", "sudo ", "env FOO=1 "] {
        let fixture = Fixture::new();
        fixture.settings(
            &fixture.project,
            json!({ "allow": ["Bash(*)"], "deny": ["Bash(git push --force:*)"] }),
        );
        let command = format!("{prefix}git push --force");
        let answer = fixture.claude(&command, &fixture.project, None);
        assert_ne!(decision(&answer), Some("allow"), "{command}: {answer}");

        let fixture = Fixture::new();
        fixture.settings(
            &fixture.project,
            json!({ "allow": ["Bash(*)"], "ask": ["Bash(git push:*)"] }),
        );
        let command = format!("{prefix}git push");
        let answer = fixture.claude(&command, &fixture.project, None);
        assert_ne!(decision(&answer), Some("allow"), "{command}: {answer}");
    }
}

#[test]
fn shell_builtins_and_git_options_do_not_hide_a_command_from_deny_or_ask_rules() {
    // The rewriter also looks past `command`, `exec`, `builtin`, `noglob`,
    // `nocorrect` and git's global options, so the rules must too.
    let commands = [
        "command git push --force",
        "exec git push --force",
        "builtin git push --force",
        "noglob git push --force",
        "nocorrect git push --force",
        "FOO=1 command git push --force",
        "sudo exec git push --force",
        "git -C . push --force",
        "git --no-pager push --force",
        "git -c core.pager=cat push --force",
    ];
    for command in commands {
        let fixture = Fixture::new();
        fixture.settings(
            &fixture.project,
            json!({ "allow": ["Bash(*)"], "deny": ["Bash(git push --force:*)"] }),
        );
        let answer = fixture.claude(command, &fixture.project, None);
        assert_ne!(decision(&answer), Some("allow"), "{command}: {answer}");

        let fixture = Fixture::new();
        fixture.settings(
            &fixture.project,
            json!({ "allow": ["Bash(*)"], "ask": ["Bash(git push:*)"] }),
        );
        let answer = fixture.claude(command, &fixture.project, None);
        assert_ne!(decision(&answer), Some("allow"), "{command}: {answer}");
    }
}

// `dirs::config_dir()` ignores HOME on Windows.
#[cfg(unix)]
#[test]
fn a_configured_transparent_prefix_does_not_hide_a_command_from_deny_rules() {
    let fixture = Fixture::new();
    fixture.settings(
        &fixture.project,
        json!({ "allow": ["Bash(*)"], "deny": ["Bash(git push --force:*)"] }),
    );
    let config = if cfg!(target_os = "macos") {
        fixture
            .home
            .join("Library/Application Support/bdo/config.toml")
    } else {
        fixture.home.join(".config/bdo/config.toml")
    };
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        "[hooks]\ntransparent_prefixes = [\"direnv exec .\"]\n",
    )
    .unwrap();

    let answer = fixture.claude("direnv exec . git status", &fixture.project, None);
    assert_eq!(
        rewritten(&answer),
        Some("direnv exec . bdo git status"),
        "the prefix must be in effect: {answer}"
    );
    let answer = fixture.claude("direnv exec . git push --force", &fixture.project, None);
    assert_ne!(decision(&answer), Some("allow"), "{answer}");
}
