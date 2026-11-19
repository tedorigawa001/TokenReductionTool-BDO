//! Plain Ruby commands must not acquire Bundler because a repository contains
//! a Gemfile. External process probes keep these tests portable and offline.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    work: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    marker: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let work = root.join("work");
        let bin = root.join("bin");
        fs::create_dir(&work).unwrap();
        fs::create_dir(&bin).unwrap();
        fs::write(
            work.join("Gemfile"),
            "raise 'unexpected Gemfile evaluation'\n",
        )
        .unwrap();
        for tool in ["rubocop", "rspec", "rake", "rails", "bundle"] {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let path = bin.join(tool);
                fs::write(&path, format!(
                    "#!/bin/sh\nprintf '%s\\n' '{tool}' \"$@\" > \"$BDO_TEST_ARGV\"\nif [ '{tool}' = bundle ]; then printf 'Gemfile evaluated\\n' > \"$BDO_TEST_MARKER\"; fi\nprintf 'Ruby tool output\\n'\nexit 37\n"
                )).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            #[cfg(windows)]
            fs::write(bin.join(format!("{tool}.cmd")), format!(
                "@echo off\r\n{}> \"%BDO_TEST_ARGV%\" echo {tool}\r\n:args\r\nif \"%~1\"==\"\" goto done\r\n>> \"%BDO_TEST_ARGV%\" echo %~1\r\nshift\r\ngoto args\r\n:done\r\necho Ruby tool output\r\nexit /b 37\r\n",
                if tool == "bundle" { "> \"%BDO_TEST_MARKER%\" echo Gemfile evaluated\r\n" } else { "" }
            )).unwrap();
        }
        Self {
            log: root.join("argv"),
            marker: root.join("gemfile-evaluated"),
            _temp: temp,
            root,
            work,
            bin,
        }
    }

    fn bdo(&self, args: &[&str]) -> Output {
        self.run(args, false)
    }

    fn run(&self, args: &[&str], raw_env: bool) -> Output {
        let _ = fs::remove_file(&self.log);
        let _ = fs::remove_file(&self.marker);
        self.command(args, raw_env)
            .output()
            .expect("run bdo with isolated Ruby probes")
    }

    fn command(&self, args: &[&str], raw_env: bool) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bdo"));
        command
            .args(args)
            .current_dir(&self.work)
            .env("PATH", &self.bin)
            .env("PATHEXT", ".EXE;.CMD;.BAT")
            .env("HOME", &self.root)
            .env("USERPROFILE", &self.root)
            .env("APPDATA", self.root.join("config"))
            .env("LOCALAPPDATA", self.root.join("data"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("BDO_DB_PATH", self.root.join("history.db"))
            .env("BDO_TEST_ARGV", &self.log)
            .env("BDO_TEST_MARKER", &self.marker)
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env("BDO_TEE", "0")
            .env_remove("BDO_RAW");
        if raw_env {
            command.env("BDO_RAW", "1");
        }
        command
    }

    fn argv(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .expect("Ruby probe was invoked")
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn rewritten(&self, command: &str) -> Vec<String> {
        let output = self.bdo(&["rewrite", command]);
        assert_eq!(output.status.code(), Some(3), "{command}: {output:?}");
        let rewritten = String::from_utf8(output.stdout).unwrap();
        assert!(rewritten.starts_with("bdo "), "{rewritten:?}");
        rewritten
            .split_whitespace()
            .skip(1)
            .map(str::to_string)
            .collect()
    }
}

#[test]
fn plain_version_queries_do_not_start_bundler_or_evaluate_gemfile() {
    let fixture = Fixture::new();
    for tool in ["rubocop", "rspec", "rake"] {
        let output = fixture.bdo(&[tool, "--version"]);
        assert_eq!(output.status.code(), Some(37), "{tool}: {output:?}");
        assert!(
            !fixture.marker.exists(),
            "{tool} started Bundler for a plain version query"
        );
        assert_eq!(fixture.argv()[0], tool);
    }
}

#[test]
fn explicitly_bundled_commands_keep_bundler_and_the_requested_tool() {
    let fixture = Fixture::new();
    // Selection comes from the invocation, not from Gemfile discovery.
    fs::remove_file(fixture.work.join("Gemfile")).unwrap();
    for (original, tool) in [
        ("bundle exec rubocop --version", "rubocop"),
        ("bundle exec rspec --version", "rspec"),
        ("bundle exec rake test", "rake"),
        ("bundle exec rails test", "rails"),
    ] {
        let rewritten = fixture.rewritten(original);
        let args: Vec<_> = rewritten.iter().map(String::as_str).collect();
        let output = fixture.bdo(&args);
        assert_eq!(output.status.code(), Some(37), "{original}: {output:?}");
        assert_eq!(&fixture.argv()[..3], ["bundle", "exec", tool], "{original}");
        assert!(fixture.marker.exists(), "explicit Bundler was not invoked");
    }
}

#[test]
fn raw_rewrites_keep_explicit_bundler_and_do_not_inject_private_or_format_options() {
    let fixture = Fixture::new();
    for (original, expected) in [
        ("rubocop --version", vec!["rubocop", "--version"]),
        ("rspec --version", vec!["rspec", "--version"]),
        ("rake test", vec!["rake", "test"]),
        ("rails test", vec!["rails", "test"]),
        (
            "bundle exec rubocop --version",
            vec!["bundle", "exec", "rubocop", "--version"],
        ),
        (
            "bundle exec rspec --version",
            vec!["bundle", "exec", "rspec", "--version"],
        ),
        (
            "bundle exec rake test",
            vec!["bundle", "exec", "rake", "test"],
        ),
        (
            "bundle exec rails test",
            vec!["bundle", "exec", "rails", "test"],
        ),
    ] {
        let rewritten = fixture.rewritten(original);
        for raw_env in [false, true] {
            let mut args: Vec<_> = rewritten.iter().map(String::as_str).collect();
            if !raw_env {
                args.insert(0, "--raw");
            }
            let output = fixture.run(&args, raw_env);
            assert_eq!(output.status.code(), Some(37), "{original}: {output:?}");
            assert_eq!(fixture.argv(), expected, "{original}: raw_env={raw_env}");
        }
    }
}

#[test]
fn rewritten_plain_tools_keep_their_identity_even_when_rake_has_file_arguments() {
    let fixture = Fixture::new();
    for (original, expected) in [
        (
            "rubocop --version",
            vec!["rubocop", "--format", "json", "--version"],
        ),
        (
            "rspec --version",
            vec!["rspec", "--format", "json", "--version"],
        ),
        (
            "rake test test/example_test.rb",
            vec!["rake", "test", "test/example_test.rb"],
        ),
        ("rails test", vec!["rails", "test"]),
    ] {
        let rewritten = fixture.rewritten(original);
        let args: Vec<_> = rewritten.iter().map(String::as_str).collect();
        let output = fixture.bdo(&args);
        assert_eq!(output.status.code(), Some(37), "{original}: {output:?}");
        assert_eq!(fixture.argv(), expected, "{original}");
        assert!(!fixture.marker.exists());
    }
}

#[test]
fn a_local_rails_binstub_is_not_replaced_by_the_path_tool() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.work.join("bin")).unwrap();
    #[cfg(unix)]
    let filename = "rails";
    #[cfg(windows)]
    let filename = "rails.cmd";
    let content = fs::read_to_string(fixture.bin.join(filename))
        .unwrap()
        .replace("rails", "local-rails");
    let local = fixture.work.join("bin").join(filename);
    fs::write(&local, content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&local, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let rewritten = fixture.rewritten("bin/rails test");
    let args: Vec<_> = rewritten.iter().map(String::as_str).collect();
    for raw_env in [false, true] {
        let output = fixture.run(&args, raw_env);
        assert_eq!(output.status.code(), Some(37), "{output:?}");
        assert_eq!(fixture.argv(), ["local-rails", "test"]);
    }
    fs::remove_file(local).unwrap();
    let output = fixture.bdo(&args);
    assert_ne!(output.status.code(), Some(37));
    assert!(!fixture.log.exists(), "missing binstub fell back to PATH");
}

#[test]
fn tool_arguments_and_script_names_cannot_enable_bundler() {
    let fixture = Fixture::new();
    for (subcommand, program, expected) in [
        ("rubocop", "rubocop", vec!["rubocop", "--format", "json"]),
        ("rspec", "rspec", vec!["rspec", "--format", "json"]),
        ("rake", "rake", vec!["rake"]),
    ] {
        let mut expected = expected;
        expected.extend([
            "--bdo-executable",
            "bundle-exec-rubocop",
            "file with spaces",
        ]);
        let output = fixture.bdo(&[
            subcommand,
            "--bdo-executable",
            program,
            "--",
            "--bdo-executable",
            "bundle-exec-rubocop",
            "file with spaces",
        ]);
        assert_eq!(output.status.code(), Some(37), "{output:?}");
        assert_eq!(fixture.argv(), expected);
        assert!(!fixture.marker.exists());
        let output = fixture.bdo(&[subcommand, "bundle-exec-rubocop"]);
        assert_eq!(output.status.code(), Some(37));
        assert_eq!(fixture.argv()[0], program);
        assert!(!fixture.marker.exists());
    }
}

#[test]
fn invalid_or_cross_tool_selectors_fail_without_running_a_child() {
    let fixture = Fixture::new();
    for subcommand in ["rubocop", "rspec", "rake"] {
        for selector in ["sh", "bundle-exec-sh", "./other-tool"] {
            for raw_env in [false, true] {
                let output = fixture.run(
                    &[subcommand, "--bdo-executable", selector, "--", "--version"],
                    raw_env,
                );
                assert!(!output.status.success());
                assert!(!fixture.log.exists());
                assert!(!fixture.marker.exists());
            }
        }
    }
    let output = fixture.bdo(&[
        "rspec",
        "--bdo-executable",
        "bundle-exec-rubocop",
        "--",
        "--version",
    ]);
    assert!(!output.status.success());
    assert!(!fixture.log.exists());
}

#[test]
fn allowed_hook_commands_keep_plain_and_explicitly_bundled_contexts_separate() {
    let fixture = Fixture::new();
    let originals = [
        "rubocop --version",
        "rspec --version",
        "rake test test/example_test.rb",
        "bundle exec rubocop --version",
    ];
    fs::create_dir(fixture.root.join(".claude")).unwrap();
    fs::write(
        fixture.root.join(".claude/settings.json"),
        serde_json::to_vec(&serde_json::json!({
            "permissions": { "allow": originals.map(|cmd| format!("Bash({cmd})")) }
        }))
        .unwrap(),
    )
    .unwrap();
    for original in originals {
        let mut child = fixture
            .command(&["hook", "claude"], false)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = serde_json::to_vec(&serde_json::json!({"tool_name":"Bash", "tool_input":{"command": original}, "cwd": fixture.work})).unwrap();
        child.stdin.take().unwrap().write_all(&input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            response.pointer("/hookSpecificOutput/permissionDecision"),
            Some(&serde_json::json!("allow"))
        );
        let rewritten = response
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|v| v.as_str())
            .unwrap();
        let args: Vec<_> = rewritten.split_whitespace().skip(1).collect();
        let output = fixture.bdo(&args);
        assert_eq!(output.status.code(), Some(37), "{output:?}");
        let bundled = original.starts_with("bundle exec ");
        assert_eq!(fixture.marker.exists(), bundled, "{original}");
        assert_eq!(
            fixture.argv()[0],
            if bundled {
                "bundle"
            } else {
                original.split_whitespace().next().unwrap()
            }
        );
    }
}

#[test]
fn equals_form_selectors_preserve_explicit_bundler_in_normal_and_raw_modes() {
    let fixture = Fixture::new();
    for raw_env in [false, true] {
        let output = fixture.run(
            &[
                "rspec",
                "--bdo-executable=bundle-exec-rspec",
                "--",
                "--version",
            ],
            raw_env,
        );
        assert_eq!(output.status.code(), Some(37), "{output:?}");
        let expected = if raw_env {
            vec!["bundle", "exec", "rspec", "--version"]
        } else {
            vec!["bundle", "exec", "rspec", "--format", "json", "--version"]
        };
        assert_eq!(fixture.argv(), expected);
    }
}
