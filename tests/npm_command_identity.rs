//! npm's script-list aliases must not become script names. These process-boundary
//! probes run on Unix and native Windows without requiring npm or downloading it.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
    argv: PathBuf,
    script_marker: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = bin.join("npm");
            fs::write(
                &path,
                r#"#!/bin/sh
printf '%s\n' "$@" > "$BDO_TEST_ARGV"
if [ "$1" = run ]; then
    case "$2" in
        run-script|rum|urn|x) printf 'script ran\n' > "$BDO_TEST_SCRIPT" ;;
    esac
fi
printf 'npm probe\n'
exit "$BDO_TEST_EXIT"
"#,
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        #[cfg(windows)]
        fs::write(
            bin.join("npm.cmd"),
            concat!(
                "@echo off\r\n",
                "if not \"%~1\"==\"run\" goto log\r\n",
                "for %%s in (run-script rum urn x) do if \"%~2\"==\"%%s\" goto script\r\n",
                "goto log\r\n",
                ":script\r\n",
                "> \"%BDO_TEST_SCRIPT%\" echo script ran\r\n",
                ":log\r\n",
                "type nul > \"%BDO_TEST_ARGV%\"\r\n",
                ":args\r\n",
                "if \"%~1\"==\"\" goto done\r\n",
                ">> \"%BDO_TEST_ARGV%\" echo %~1\r\n",
                "shift\r\n",
                "goto args\r\n",
                ":done\r\n",
                "echo npm probe\r\n",
                "exit /b %BDO_TEST_EXIT%\r\n",
            ),
        )
        .unwrap();
        Self {
            argv: root.join("argv"),
            script_marker: root.join("script-ran"),
            _temp: temp,
            root,
            bin,
        }
    }

    fn bdo(&self, args: &[&str]) -> Output {
        let _ = fs::remove_file(&self.argv);
        let _ = fs::remove_file(&self.script_marker);
        self.command(args)
            .output()
            .expect("run bdo with an isolated npm probe")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bdo"));
        command
            .args(args)
            .current_dir(&self.root)
            .env("PATH", &self.bin)
            .env("PATHEXT", ".EXE;.CMD;.BAT")
            .env("HOME", &self.root)
            .env("USERPROFILE", &self.root)
            .env("APPDATA", self.root.join("config"))
            .env("LOCALAPPDATA", self.root.join("data"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("BDO_DB_PATH", self.root.join("history.db"))
            .env("BDO_TEST_ARGV", &self.argv)
            .env("BDO_TEST_SCRIPT", &self.script_marker)
            .env("BDO_TEST_EXIT", "37")
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env("BDO_TEE", "0")
            .env_remove("BDO_RAW")
            .env_remove("BDO_QUIET");
        command
    }

    fn argv(&self) -> Vec<String> {
        fs::read_to_string(&self.argv)
            .expect("npm probe was invoked")
            .lines()
            .map(str::to_string)
            .collect()
    }
}

#[test]
fn script_listing_aliases_never_execute_a_package_script() {
    let fixture = Fixture::new();
    for alias in ["run", "run-script", "rum", "urn"] {
        let output = fixture.bdo(&["npm", alias]);
        assert_eq!(output.status.code(), Some(37), "{alias}: {output:?}");
        assert!(
            !fixture.script_marker.exists(),
            "listing via {alias} must not execute a script"
        );
        assert_eq!(fixture.argv(), [alias]);
    }
}

#[test]
fn exec_alias_is_not_reinterpreted_as_a_script_name() {
    let fixture = Fixture::new();
    for alias in ["exec", "x"] {
        let output = fixture.bdo(&["npm", alias, "--help"]);
        assert_eq!(output.status.code(), Some(37), "{alias}: {output:?}");
        assert_eq!(fixture.argv(), [alias, "--help"]);
    }
}

#[test]
fn script_aliases_preserve_the_requested_script_options_and_separator() {
    let fixture = Fixture::new();
    for alias in ["run", "run-script", "rum", "urn"] {
        let args = [alias, "lint", "--", "--fix", "file with spaces.js"];
        let mut command = vec!["npm"];
        command.extend(args);
        let output = fixture.bdo(&command);
        assert_eq!(output.status.code(), Some(37), "{alias}: {output:?}");
        assert_eq!(fixture.argv(), args);

        let output = fixture.bdo(&["npm", "--silent", alias]);
        assert_eq!(output.status.code(), Some(37), "{alias}: {output:?}");
        assert_eq!(fixture.argv(), ["--silent", alias]);
        assert!(!fixture.script_marker.exists());
    }
}

#[test]
fn script_shorthands_and_explicit_runs_remain_available() {
    let fixture = Fixture::new();
    for script in ["build", "dev", "lint", "typecheck", "deploy"] {
        let output = fixture.bdo(&["npm", script, "--", "argument"]);
        assert_eq!(output.status.code(), Some(37), "{script}: {output:?}");
        assert_eq!(fixture.argv(), ["run", script, "--", "argument"]);
    }
    // These script names are legal when running them was explicitly requested.
    for script in ["run-script", "rum", "urn", "x"] {
        let output = fixture.bdo(&["npm", "run", script]);
        assert_eq!(output.status.code(), Some(37), "{script}: {output:?}");
        assert_eq!(fixture.argv(), ["run", script]);
        assert!(fixture.script_marker.exists());
    }
}

#[test]
fn an_allowed_hook_listing_stays_a_listing_when_the_rewrite_is_executed() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.root.join(".claude")).unwrap();
    let aliases = ["run", "run-script", "rum", "urn"];
    fs::write(
        fixture.root.join(".claude/settings.json"),
        serde_json::to_vec(&serde_json::json!({
            "permissions": {
                "allow": aliases.map(|alias| format!("Bash(npm {alias})"))
            }
        }))
        .unwrap(),
    )
    .unwrap();

    for alias in aliases {
        let input = serde_json::to_vec(&serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": format!("npm {alias}") },
            "cwd": fixture.root,
        }))
        .unwrap();
        let mut child = fixture
            .command(&["hook", "claude"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{alias}: {output:?}");
        let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            response.pointer("/hookSpecificOutput/permissionDecision"),
            Some(&serde_json::json!("allow"))
        );
        let rewritten = response
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|value| value.as_str())
            .unwrap();
        assert_eq!(rewritten, format!("bdo npm {alias}"));
        let args: Vec<_> = rewritten.split_whitespace().skip(1).collect();
        let output = fixture.bdo(&args);
        assert_eq!(output.status.code(), Some(37), "{alias}: {output:?}");
        assert_eq!(fixture.argv(), [alias]);
        assert!(!fixture.script_marker.exists(), "{alias} executed a script");
    }
}
