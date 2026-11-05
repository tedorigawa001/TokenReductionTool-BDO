//! `bdo init` for every agent, through the real binary, in a temporary home.
//!
//! This writes to files people keep by hand, so most of this checks what must hold
//! for every agent: it starts from nothing, a second run changes nothing, a dry run
//! writes nothing, and uninstalling leaves the user's own text as it was.
//!
//! `HOME` points at a temporary directory, which is why this is Unix only: on
//! Windows the home directory comes from a system call.
#![cfg(unix)]

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        fs::create_dir_all(root.join("proj")).unwrap();
        Self { _dir: dir, root }
    }

    /// `path` is under the temporary root: `home/…` or `proj/…`.
    fn write(&self, path: &str, content: &str) -> &Self {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
        self
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    fn json(&self, path: &str) -> Value {
        serde_json::from_str(&self.read(path)).unwrap()
    }

    fn exists(&self, path: &str) -> bool {
        self.root.join(path).symlink_metadata().is_ok()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bdo"));
        command
            .args(args)
            .current_dir(self.root.join("proj"))
            .stdin(Stdio::null())
            .env("HOME", self.root.join("home"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env("BDO_DB_PATH", self.root.join("track.db"));
        for variable in [
            "CLAUDE_CONFIG_DIR",
            "CODEX_HOME",
            "HERMES_HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
        ] {
            command.env_remove(variable);
        }
        command
    }

    fn bdo(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    /// Everything on disk, apart from what bdo keeps for itself whatever is run.
    fn snapshot(&self) -> BTreeMap<String, String> {
        let mut found = BTreeMap::new();
        for top in ["home", "proj"] {
            collect(&self.root, &self.root.join(top), &mut found);
        }
        found
    }
}

fn collect(root: &Path, dir: &Path, found: &mut BTreeMap<String, String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        // bdo's own state and the user-level filters template, on macOS and Linux.
        if ["/Library", "/.config/bdo", "/.local"]
            .iter()
            .any(|own| name.contains(own))
        {
            continue;
        }
        if path.is_dir() && !path.is_symlink() {
            found.insert(format!("{name}/"), String::new());
            collect(root, &path, found);
        } else {
            found.insert(name, fs::read_to_string(&path).unwrap_or_default());
        }
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[track_caller]
fn assert_ok(out: &Output) {
    assert!(out.status.success(), "{}", text(out));
}

#[track_caller]
fn assert_failed(out: &Output) {
    assert_eq!(out.status.code(), Some(1), "{}", text(out));
}

/// Install and uninstall arguments of each agent that can be uninstalled.
const AGENTS: &[(&str, &[&str], &[&str])] = &[
    (
        "claude",
        &["init", "-g", "--auto-patch"],
        &["init", "-g", "--uninstall"],
    ),
    (
        "claude hook-only",
        &["init", "-g", "--hook-only", "--auto-patch"],
        &["init", "-g", "--uninstall"],
    ),
    (
        "claude no-patch",
        &["init", "-g", "--no-patch"],
        &["init", "-g", "--uninstall"],
    ),
    (
        "claude-md",
        &["init", "-g", "--claude-md"],
        &["init", "-g", "--uninstall"],
    ),
    (
        "opencode",
        &["init", "-g", "--opencode", "--auto-patch"],
        &["init", "-g", "--opencode", "--uninstall"],
    ),
    (
        "codex",
        &["init", "--codex", "-g"],
        &["init", "--codex", "-g", "--uninstall"],
    ),
    (
        "gemini",
        &["init", "--gemini", "-g", "--auto-patch"],
        &["init", "--gemini", "-g", "--uninstall"],
    ),
    (
        "gemini hook-only",
        &["init", "--gemini", "-g", "--hook-only", "--auto-patch"],
        &["init", "--gemini", "-g", "--uninstall"],
    ),
    (
        "copilot",
        &["init", "--copilot"],
        &["init", "--copilot", "--uninstall"],
    ),
    (
        "copilot global",
        &["init", "--copilot", "-g"],
        &["init", "--copilot", "-g", "--uninstall"],
    ),
    (
        "windsurf",
        &["init", "-g", "--agent", "windsurf"],
        &["init", "-g", "--agent", "windsurf", "--uninstall"],
    ),
    (
        "cline",
        &["init", "--agent", "cline"],
        &["init", "--agent", "cline", "--uninstall"],
    ),
    (
        "kilocode",
        &["init", "--agent", "kilocode"],
        &["init", "--agent", "kilocode", "--uninstall"],
    ),
    (
        "antigravity",
        &["init", "--agent", "antigravity"],
        &["init", "--agent", "antigravity", "--uninstall"],
    ),
    (
        "hermes",
        &["init", "--agent", "hermes"],
        &["init", "--agent", "hermes", "--uninstall"],
    ),
    (
        "pi",
        &["init", "--agent", "pi"],
        &["init", "--agent", "pi", "--uninstall"],
    ),
    (
        "pi global",
        &["init", "--agent", "pi", "-g"],
        &["init", "--agent", "pi", "-g", "--uninstall"],
    ),
];

/// What people already have before they install bdo.
const USER_FILES: &[(&str, &str)] = &[
    (
        "home/.claude/CLAUDE.md",
        "# My rules\r\n\r\nBe kind 日本語.\r\n",
    ),
    ("home/.codex/AGENTS.md", "# Codex rules\nkeep me\n"),
    ("home/.gemini/GEMINI.md", "# Gemini rules\nkeep me\n"),
    (
        "home/.copilot/copilot-instructions.md",
        "# My copilot\nkeep me\n",
    ),
    (
        "home/.hermes/config.yaml",
        "# my config\nmodel: x\nplugins:\n  enabled:\n    - other\n",
    ),
    ("proj/CLAUDE.md", "# Project rules\nkeep me\n"),
    ("proj/.windsurfrules", "# Windsurf rules\nkeep me\n"),
    ("proj/.clinerules", "# Cline rules\nkeep me\n"),
    (
        "proj/.github/copilot-instructions.md",
        "# Copilot rules\nkeep me\n",
    ),
    ("proj/.kilocode/rules/mine.md", "keep me\n"),
];

fn user_json_files() -> [(&'static str, Value); 3] {
    [
        (
            "home/.claude/settings.json",
            json!({
                "theme": "dark",
                "permissions": {"allow": ["Bash(ls:*)"]},
                "hooks": {
                    "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "mine.sh"}]}],
                    "Stop": [{"hooks": [{"type": "command", "command": "say done"}]}],
                },
            }),
        ),
        (
            "home/.gemini/settings.json",
            json!({
                "theme": "x",
                "hooks": {"BeforeTool": [{"matcher": "other", "hooks": [{"type": "command", "command": "/opt/bdo-audit/check.sh"}]}]},
            }),
        ),
        (
            "home/.cursor/hooks.json",
            json!({"version": 1, "hooks": {"preToolUse": [{"command": "mine.sh", "matcher": "Shell"}]}}),
        ),
    ]
}

fn env_with_user_files() -> Env {
    let env = Env::new();
    for (path, content) in USER_FILES {
        env.write(path, content);
    }
    for (path, value) in user_json_files() {
        env.write(path, &serde_json::to_string_pretty(&value).unwrap());
    }
    env
}

/// `true` for files that may differ only in formatting, or that bdo leaves on purpose.
fn same_for_the_user(path: &str, before: &str, after: &str) -> bool {
    if before == after {
        return true;
    }
    path.ends_with(".json")
        && serde_json::from_str::<Value>(before).ok() == serde_json::from_str::<Value>(after).ok()
}

/// Left by design: backups of the file bdo changed, and a hook list it emptied is
/// the user's to keep. Directories bdo created are checked per file underneath.
fn left_on_purpose(path: &str) -> bool {
    path.ends_with(".bak") || path.ends_with('/')
}

#[test]
fn every_agent_installs_on_a_machine_where_it_has_never_run() {
    for (name, install, _) in AGENTS {
        let env = Env::new();
        assert_ok(&env.bdo(install));
        assert!(!env.snapshot().is_empty(), "{name} wrote nothing");
    }
}

#[test]
fn a_second_install_changes_nothing() {
    for (name, install, _) in AGENTS {
        for env in [Env::new(), env_with_user_files()] {
            assert_ok(&env.bdo(install));
            let first = env.snapshot();
            assert_ok(&env.bdo(install));
            assert_eq!(first, env.snapshot(), "{name}");
        }
    }
}

#[test]
fn a_dry_run_writes_nothing() {
    for (name, install, uninstall) in AGENTS {
        let dry = |args: &[&'static str]| -> Vec<&'static str> { [args, &["--dry-run"]].concat() };
        let env = env_with_user_files();
        let before = env.snapshot();

        assert_ok(&env.bdo(&dry(install)));
        assert_eq!(before, env.snapshot(), "{name}: install --dry-run");
        assert_ok(&env.bdo(&dry(uninstall)));
        assert_eq!(
            before,
            env.snapshot(),
            "{name}: uninstall --dry-run, not installed"
        );

        assert_ok(&env.bdo(install));
        let installed = env.snapshot();
        assert_ok(&env.bdo(&dry(uninstall)));
        assert_eq!(
            installed,
            env.snapshot(),
            "{name}: uninstall --dry-run, installed"
        );
    }
}

#[test]
fn installing_keeps_every_line_the_user_wrote() {
    for (name, install, _) in AGENTS {
        let env = env_with_user_files();
        assert_ok(&env.bdo(install));
        for (path, content) in USER_FILES {
            let now = env.read(path);
            for line in content.lines() {
                assert!(
                    now.contains(line),
                    "{name} lost {line:?} from {path}:\n{now}"
                );
            }
        }
        for (path, value) in user_json_files() {
            let now = env.json(path);
            assert_eq!(now["theme"], value["theme"], "{name}: {path}");
            let hooks = serde_json::to_string(&now["hooks"]).unwrap();
            for hook in ["mine.sh", "say done", "/opt/bdo-audit/check.sh"] {
                let had = serde_json::to_string(&value["hooks"])
                    .unwrap()
                    .contains(hook);
                assert_eq!(hooks.contains(hook), had, "{name} on {hook} in {path}");
            }
        }
    }
}

/// `-g --auto-patch` on Cursor also sets up Claude Code, so it has its own test.
#[test]
fn uninstalling_restores_what_the_user_had() {
    for (name, install, uninstall) in AGENTS {
        let env = env_with_user_files();
        let before = env.snapshot();
        assert_ok(&env.bdo(install));
        let out = env.bdo(uninstall);
        assert_ok(&out);
        let after = env.snapshot();

        for (path, content) in &before {
            if path.ends_with('/') {
                continue;
            }
            let now = after
                .get(path)
                .unwrap_or_else(|| panic!("{name} deleted {path}"));
            assert!(
                same_for_the_user(path, content, now),
                "{name} changed {path}\nbefore: {content:?}\nafter: {now:?}\n{}",
                text(&out)
            );
        }
        for path in after.keys() {
            assert!(
                before.contains_key(path) || left_on_purpose(path),
                "{name} left {path} behind\n{}",
                text(&out)
            );
        }
    }
}

#[test]
fn uninstalling_from_nothing_leaves_nothing() {
    for (name, install, uninstall) in AGENTS {
        let env = Env::new();
        assert_ok(&env.bdo(install));
        assert_ok(&env.bdo(uninstall));
        // Hermes' config.yaml and the hosts' settings.json are the hosts' own files
        // by then. No rules or instructions file may stay.
        for path in env.snapshot().keys() {
            let rules = [
                "AGENTS.md",
                "CLAUDE.md",
                "GEMINI.md",
                "copilot-instructions.md",
                "rules",
            ];
            assert!(
                !rules.iter().any(|file| path.ends_with(file)),
                "{name} left {path} behind"
            );
            assert!(!path.contains("sha256"), "{name} left {path} behind");
        }
        // A second uninstall has nothing to do, and says so without failing.
        assert_ok(&env.bdo(uninstall));
    }
}

/// `-g` used to fall through to the full uninstall, removing Claude Code's setup when
/// another agent was named.
#[test]
fn uninstalling_one_agent_leaves_the_others_alone() {
    let others: &[&[&str]] = &[
        &["init", "-g", "--agent", "windsurf", "--uninstall"],
        &["init", "-g", "--agent", "cline", "--uninstall"],
        &["init", "-g", "--agent", "kilocode", "--uninstall"],
        &["init", "-g", "--agent", "antigravity", "--uninstall"],
        &["init", "-g", "--opencode", "--uninstall"],
        &["init", "--agent", "windsurf", "--uninstall"],
        &["init", "--gemini", "-g", "--uninstall"],
        &["init", "--codex", "-g", "--uninstall"],
        &["init", "-g", "--agent", "cursor", "--uninstall"],
    ];
    for args in others {
        let env = Env::new();
        assert_ok(&env.bdo(&["init", "-g", "--auto-patch"]));
        let before = env.snapshot();
        assert_ok(&env.bdo(args));
        assert_eq!(before, env.snapshot(), "{args:?}");
    }
}

#[test]
fn cursor_uninstall_removes_its_hook_and_says_claude_stays() {
    let env = Env::new();
    env.write(
        "home/.cursor/hooks.json",
        &json!({"version": 1, "hooks": {"preToolUse": [{"command": "mine.sh"}]}}).to_string(),
    );
    assert_ok(&env.bdo(&["init", "-g", "--agent", "cursor", "--auto-patch"]));
    assert!(env
        .read("home/.cursor/hooks.json")
        .contains("bdo hook cursor"));
    assert!(env.exists("home/.claude/Bushido.md"));

    let out = env.bdo(&["init", "-g", "--agent", "cursor", "--uninstall"]);
    assert_ok(&out);
    assert!(
        text(&out).contains("bdo init -g --uninstall"),
        "{}",
        text(&out)
    );
    let cursor = env.read("home/.cursor/hooks.json");
    assert!(cursor.contains("mine.sh"), "{cursor}");
    assert!(!cursor.contains("bdo hook"), "{cursor}");
    assert!(env.exists("home/.claude/Bushido.md"));

    assert_ok(&env.bdo(&["init", "-g", "--uninstall"]));
    assert!(!env.exists("home/.claude/Bushido.md"));
}

/// This file holds people's own instructions to Gemini. bdo used to replace it on
/// install and delete it on uninstall, even under `--hook-only`.
#[test]
fn gemini_md_is_never_replaced_or_deleted() {
    let mine = "# Gemini rules\nkeep me\n";
    let env = Env::new();
    env.write("home/.gemini/GEMINI.md", mine);

    assert_ok(&env.bdo(&["init", "--gemini", "-g", "--hook-only", "--auto-patch"]));
    assert_eq!(env.read("home/.gemini/GEMINI.md"), mine);
    assert_ok(&env.bdo(&["init", "--gemini", "-g", "--uninstall"]));
    assert_eq!(env.read("home/.gemini/GEMINI.md"), mine);

    assert_ok(&env.bdo(&["init", "--gemini", "-g", "--auto-patch"]));
    let installed = env.read("home/.gemini/GEMINI.md");
    assert!(installed.starts_with(mine.trim()), "{installed}");
    assert!(installed.contains("bdo gain"), "{installed}");
    assert_ok(&env.bdo(&["init", "--gemini", "-g", "--uninstall"]));
    assert_eq!(env.read("home/.gemini/GEMINI.md"), mine);
}

/// A file an earlier version wrote whole is bdo's own, so it is not kept as if
/// the user had written it.
#[test]
fn gemini_md_written_by_an_earlier_version_moves_to_a_block() {
    let slim = include_str!("../hooks/claude/bdo-awareness.md");
    let env = Env::new();
    env.write("home/.gemini/GEMINI.md", slim);
    assert_ok(&env.bdo(&["init", "--gemini", "-g", "--auto-patch"]));
    let upgraded = env.read("home/.gemini/GEMINI.md");
    assert!(upgraded.starts_with("<!-- bdo-instructions"), "{upgraded}");
    assert_eq!(
        upgraded.matches("Rust Token Killer").count(),
        1,
        "{upgraded}"
    );

    env.write("home/.gemini/GEMINI.md", slim);
    assert_ok(&env.bdo(&["init", "--gemini", "-g", "--uninstall"]));
    assert!(!env.exists("home/.gemini/GEMINI.md"));
}

/// A hook of the user's whose name has "bdo" in it is not bdo's.
#[test]
fn gemini_keeps_hooks_that_merely_mention_bdo() {
    let env = Env::new();
    let settings = json!({"hooks": {"BeforeTool": [
        {"matcher": "x", "hooks": [{"type": "command", "command": "/opt/bdo-audit/check.sh"}]},
    ]}});
    env.write("home/.gemini/settings.json", &settings.to_string());

    assert_ok(&env.bdo(&["init", "--gemini", "-g", "--auto-patch"]));
    let installed = env.read("home/.gemini/settings.json");
    assert!(
        installed.contains("bdo-hook-gemini.sh"),
        "not installed: {installed}"
    );

    assert_ok(&env.bdo(&["init", "--gemini", "-g", "--uninstall"]));
    assert_eq!(env.json("home/.gemini/settings.json"), settings);
}

/// The other half of an entry is the user's.
#[test]
fn a_hook_sharing_an_entry_with_bdo_stays() {
    let shared = |ours: &str, key: &str| {
        json!({"hooks": {key: [{"matcher": "Bash", "hooks": [
            {"type": "command", "command": "mine.sh"},
            {"type": "command", "command": ours},
        ]}]}})
    };
    let only_mine = |key: &str| json!({"hooks": {key: [{"matcher": "Bash", "hooks": [{"type": "command", "command": "mine.sh"}]}]}});

    let env = Env::new();
    env.write(
        "home/.claude/settings.json",
        &shared("bdo hook claude", "PreToolUse").to_string(),
    );
    assert_ok(&env.bdo(&["init", "-g", "--uninstall"]));
    assert_eq!(
        env.json("home/.claude/settings.json"),
        only_mine("PreToolUse")
    );

    let hook = env.root.join("home/.gemini/hooks/bdo-hook-gemini.sh");
    env.write(
        "home/.gemini/settings.json",
        &shared(hook.to_str().unwrap(), "BeforeTool").to_string(),
    );
    assert_ok(&env.bdo(&["init", "--gemini", "-g", "--uninstall"]));
    assert_eq!(
        env.json("home/.gemini/settings.json"),
        only_mine("BeforeTool")
    );
}

/// A settings file bdo cannot read is the user's to fix. bdo used to replace Gemini's.
#[test]
fn settings_it_cannot_read_are_left_alone() {
    for (path, install) in [
        (
            "home/.claude/settings.json",
            &["init", "-g", "--auto-patch"][..],
        ),
        (
            "home/.claude/settings.json",
            &["init", "-g", "--hook-only", "--auto-patch"],
        ),
        ("home/.claude/settings.json", &["init", "-g", "--no-patch"]),
        (
            "home/.gemini/settings.json",
            &["init", "--gemini", "-g", "--auto-patch"],
        ),
    ] {
        for content in ["{ \"theme\": \"dark\", // comment\n}\n", "{ broken"] {
            let env = Env::new();
            env.write(path, content);
            let out = env.bdo(install);
            assert_failed(&out);
            assert!(text(&out).contains("parse"), "{}", text(&out));
            assert_eq!(env.read(path), content, "{install:?}");
        }
    }
}

/// Failing after CLAUDE.md has changed left a half-done install, reported as done.
#[test]
fn a_settings_problem_stops_claude_setup_before_it_changes_anything() {
    for content in [
        "{ broken",
        "[]",
        "[1, 2]",
        "{\"hooks\": \"x\"}",
        "{\"hooks\": {\"PreToolUse\": {}}}",
    ] {
        let env = Env::new();
        env.write("home/.claude/settings.json", content);
        env.write("home/.claude/CLAUDE.md", "# Mine\n");
        let before = env.snapshot();
        let out = env.bdo(&["init", "-g", "--auto-patch"]);
        assert_failed(&out);
        assert!(!text(&out).contains("set up"), "{content}: {}", text(&out));
        assert_eq!(before, env.snapshot(), "{content}");
    }
}

#[test]
fn a_hook_it_did_not_add_is_not_reported_as_registered() {
    let env = Env::new();
    let out = env.bdo(&["init", "-g", "--no-patch"]);
    assert_ok(&out);
    assert!(!text(&out).contains("hook registered"), "{}", text(&out));
    assert!(text(&out).contains("MANUAL STEP"), "{}", text(&out));
    assert!(!env.exists("home/.claude/settings.json"));
}

/// Dotfile managers keep these as links. Replacing a link would cut the file off
/// from the repository it is kept in.
#[test]
fn symlinked_config_files_stay_links() {
    let env = Env::new();
    env.write("home/dotfiles/settings.json", "{\"theme\": \"x\"}\n");
    env.write("home/dotfiles/CLAUDE.md", "# Mine\n");
    fs::create_dir_all(env.root.join("home/.claude")).unwrap();
    for name in ["settings.json", "CLAUDE.md"] {
        symlink(
            env.root.join("home/dotfiles").join(name),
            env.root.join("home/.claude").join(name),
        )
        .unwrap();
    }
    let is_link = |name: &str| {
        env.root
            .join("home/.claude")
            .join(name)
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    };

    assert_ok(&env.bdo(&["init", "-g", "--auto-patch"]));
    assert!(is_link("settings.json") && is_link("CLAUDE.md"));
    assert!(env
        .read("home/dotfiles/settings.json")
        .contains("bdo hook claude"));
    assert!(env.read("home/dotfiles/CLAUDE.md").contains("@Bushido.md"));

    assert_ok(&env.bdo(&["init", "-g", "--uninstall"]));
    assert!(is_link("settings.json") && is_link("CLAUDE.md"));
    assert_eq!(env.read("home/dotfiles/CLAUDE.md"), "# Mine\n");
}

/// Line endings and the final newline are the user's. Only bdo's lines go.
#[test]
fn removing_the_reference_keeps_every_other_byte() {
    let env = Env::new();
    env.write(
        "home/.claude/CLAUDE.md",
        "# Mine\r\nline two\r\n\r\n@Bushido.md\r\n\r\nafter\r\n",
    );
    assert_ok(&env.bdo(&["init", "-g", "--uninstall"]));
    assert_eq!(
        env.read("home/.claude/CLAUDE.md"),
        "# Mine\r\nline two\r\n\r\nafter\r\n"
    );
}

/// A word in the user's rules is not an install, and a rules file it cannot read as
/// text is not an empty one.
#[test]
fn rules_files_are_read_with_care() {
    for (path, install) in [
        ("proj/.clinerules", &["init", "--agent", "cline"][..]),
        (
            "proj/.windsurfrules",
            &["init", "-g", "--agent", "windsurf"],
        ),
    ] {
        let env = Env::new();
        env.write(path, "Never touch abdomen.txt\n");
        assert_ok(&env.bdo(install));
        let installed = env.read(path);
        assert!(
            installed.starts_with("Never touch abdomen.txt"),
            "{installed}"
        );
        assert!(
            installed.contains("# Bushido - Rust Token Killer"),
            "{installed}"
        );

        let env = Env::new();
        let shift_jis: &[u8] = b"# \x83\x8b\x81[\x83\x8b\n";
        fs::write(env.root.join(path), shift_jis).unwrap();
        assert_failed(&env.bdo(install));
        assert_eq!(fs::read(env.root.join(path)).unwrap(), shift_jis);
    }
}

#[test]
fn an_edited_copy_of_the_rules_is_left_for_the_user() {
    let env = Env::new();
    assert_ok(&env.bdo(&["init", "--agent", "cline"]));
    let edited = env.read("proj/.clinerules") + "\nmy addition inside\n";
    let edited = edited.replace("Token-optimized", "Edited");
    env.write("proj/.clinerules", &edited);
    let out = env.bdo(&["init", "--agent", "cline", "--uninstall"]);
    assert_ok(&out);
    assert!(text(&out).contains("by hand"), "{}", text(&out));
    assert_eq!(env.read("proj/.clinerules"), edited);
}

#[test]
fn an_unterminated_block_is_refused_and_kept() {
    let content = "# Mine\n<!-- bdo-instructions v1 -->\nunterminated\nmy text after\n";
    let env = Env::new();
    env.write("proj/CLAUDE.md", content);
    let out = env.bdo(&["init", "--claude-md"]);
    assert_failed(&out);
    assert!(
        text(&out).contains("without closing marker"),
        "{}",
        text(&out)
    );
    assert_eq!(env.read("proj/CLAUDE.md"), content);
}

/// Local installs cannot be undone yet, and must say so rather than appear to work.
#[test]
fn local_uninstall_says_what_to_do_instead() {
    for (install, uninstall, file) in [
        (&["init"][..], &["init", "--uninstall"][..], "CLAUDE.md"),
        (
            &["init", "--claude-md"],
            &["init", "--uninstall"],
            "CLAUDE.md",
        ),
        (
            &["init", "--codex"],
            &["init", "--codex", "--uninstall"],
            "AGENTS.md",
        ),
    ] {
        let env = Env::new();
        assert_ok(&env.bdo(install));
        let before = env.snapshot();
        let out = env.bdo(uninstall);
        assert_failed(&out);
        assert!(text(&out).contains(file), "{}", text(&out));
        assert_eq!(before, env.snapshot());
    }
}

#[test]
fn modes_that_are_global_only_refuse_local_runs() {
    for (args, says) in [
        (&["init", "--gemini"][..], "global-only"),
        (&["init", "--agent", "windsurf"], "global-only"),
        (&["init", "--agent", "cursor"], "global-only"),
        (&["init", "--opencode"], "global-only"),
        (&["init", "--codex", "--claude-md"], "cannot be combined"),
        (
            &["init", "--codex", "-g", "--auto-patch"],
            "cannot be combined",
        ),
    ] {
        let env = Env::new();
        let out = env.bdo(args);
        assert_failed(&out);
        assert!(text(&out).contains(says), "{args:?}: {}", text(&out));
        assert!(env.snapshot().is_empty(), "{args:?}");
    }
}

#[test]
fn config_directory_variables_are_honoured() {
    let env = Env::new();
    let alt = env.root.join("alt-claude");
    let out = env
        .command(&["init", "-g", "--auto-patch"])
        .env("CLAUDE_CONFIG_DIR", &alt)
        .output()
        .unwrap();
    assert_ok(&out);
    assert!(alt.join("settings.json").exists());
    assert!(!env.exists("home/.claude"));

    let codex = env.root.join("alt-codex");
    let out = env
        .command(&["init", "--codex", "-g"])
        .env("CODEX_HOME", &codex)
        .output()
        .unwrap();
    assert_ok(&out);
    assert!(codex.join("AGENTS.md").exists());
    assert!(!env.exists("home/.codex"));
}

#[test]
fn a_read_only_config_directory_is_a_clear_error() {
    let env = Env::new();
    env.write("home/.claude/settings.json", "{}");
    let dir = env.root.join("home/.claude");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    if fs::write(dir.join("probe"), "").is_ok() {
        // Running as root: permissions do not apply.
        let _ = fs::remove_file(dir.join("probe"));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        return;
    }
    let out = env.bdo(&["init", "-g", "--auto-patch"]);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_failed(&out);
    assert!(
        text(&out).contains("Failed to write Bushido.md"),
        "{}",
        text(&out)
    );
    assert!(!text(&out).contains("panicked"), "{}", text(&out));
}

#[test]
fn show_reports_what_is_installed() {
    let env = Env::new();
    let out = env.bdo(&["init", "--show"]);
    assert_ok(&out);
    assert!(
        text(&out).contains("[--] settings.json: not found"),
        "{}",
        text(&out)
    );

    assert_ok(&env.bdo(&["init", "-g", "--auto-patch"]));
    let shown = text(&env.bdo(&["init", "--show"]));
    assert!(!shown.contains("[--] settings.json: not found"), "{shown}");
    assert!(!shown.contains("[--] Bushido.md: not found"), "{shown}");
    assert!(shown.contains("[ok]"), "{shown}");

    assert_ok(&env.bdo(&["init", "-g", "--uninstall"]));
    let shown = text(&env.bdo(&["init", "--show"]));
    assert!(shown.contains("[--] Bushido.md: not found"), "{shown}");
    assert_ok(&env.bdo(&["init", "--show", "--codex"]));
}
