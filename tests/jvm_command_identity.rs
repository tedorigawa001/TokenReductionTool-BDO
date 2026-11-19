//! Rewriting a JVM command must preserve the selected executable, not infer it
//! from files in the repository. Probes distinguish PATH tools from local files.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    work: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

fn probe(path: &Path, label: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(
            path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{label}' \"$@\" > \"$BDO_TEST_ARGV\"\nprintf 'BUILD SUCCESSFUL\\n'\nexit 37\n"
            ),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(windows)]
    fs::write(
        path,
        format!(
            "@echo off\r\n> \"%BDO_TEST_ARGV%\" echo {label}\r\n:args\r\nif \"%~1\"==\"\" goto done\r\n>> \"%BDO_TEST_ARGV%\" echo %~1\r\nshift\r\ngoto args\r\n:done\r\necho BUILD SUCCESSFUL\r\nexit /b 37\r\n"
        ),
    )
    .unwrap();
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let work = root.join("work");
        let bin = root.join("bin");
        fs::create_dir(&work).unwrap();
        fs::create_dir(&bin).unwrap();
        #[cfg(unix)]
        for name in [
            "gradle",
            "gradlew",
            "gradlew.bat",
            "mvn",
            "mvnw",
            "mvnw.cmd",
        ] {
            probe(&bin.join(name), &format!("path-{name}"));
        }
        #[cfg(windows)]
        for (file, name) in [
            ("gradle.cmd", "gradle"),
            ("gradlew.bat", "gradlew"),
            ("mvn.cmd", "mvn"),
            ("mvnw.cmd", "mvnw"),
        ] {
            probe(&bin.join(file), &format!("path-{name}"));
        }
        for name in ["gradlew", "gradlew.bat", "mvnw", "mvnw.cmd"] {
            probe(&work.join(name), &format!("local-{name}"));
        }
        Self {
            log: root.join("argv"),
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
        self.command(args, raw_env)
            .output()
            .expect("run bdo with isolated JVM probes")
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
            .expect("a JVM probe was invoked")
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
fn installed_gradle_is_not_replaced_by_a_repository_wrapper() {
    let fixture = Fixture::new();
    let control = fixture.bdo(&["proxy", "gradle", "--version"]);
    assert_eq!(control.status.code(), Some(37));
    assert_eq!(fixture.argv(), ["path-gradle", "--version"]);

    let rewritten = fixture.rewritten("gradle --version");
    let args: Vec<_> = rewritten.iter().map(String::as_str).collect();
    let output = fixture.bdo(&args);
    assert_eq!(output.status.code(), Some(37), "{output:?}");
    assert_eq!(fixture.argv(), ["path-gradle", "--version"]);
}

#[test]
fn installed_maven_is_not_replaced_by_a_repository_wrapper() {
    let fixture = Fixture::new();
    let control = fixture.bdo(&["proxy", "mvn", "--version", "test"]);
    assert_eq!(control.status.code(), Some(37));
    assert_eq!(fixture.argv(), ["path-mvn", "--version", "test"]);

    let rewritten = fixture.rewritten("mvn --version test");
    let args: Vec<_> = rewritten.iter().map(String::as_str).collect();
    let output = fixture.bdo(&args);
    assert_eq!(output.status.code(), Some(37), "{output:?}");
    assert_eq!(fixture.argv(), ["path-mvn", "--version", "test"]);
}

#[test]
fn an_allowed_hook_command_keeps_its_original_executable_when_run() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.root.join(".claude")).unwrap();
    fs::write(
        fixture.root.join(".claude/settings.json"),
        serde_json::to_vec(&serde_json::json!({
            "permissions": { "allow": ["Bash(gradle --version)", "Bash(mvn --version test)"] }
        }))
        .unwrap(),
    )
    .unwrap();
    for (original, expected) in [
        ("gradle --version", vec!["path-gradle", "--version"]),
        ("mvn --version test", vec!["path-mvn", "--version", "test"]),
    ] {
        let mut child = fixture
            .command(&["hook", "claude"], false)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = serde_json::to_vec(&serde_json::json!({
            "tool_name": "Bash", "tool_input": { "command": original }, "cwd": fixture.work,
        }))
        .unwrap();
        child.stdin.take().unwrap().write_all(&input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{original}: {output:?}");
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
        assert_eq!(output.status.code(), Some(37), "{original}: {output:?}");
        assert_eq!(fixture.argv(), expected, "{original}");
    }
}

#[test]
fn raw_rewrites_keep_the_selected_executable_without_leaking_private_options() {
    let fixture = Fixture::new();
    for (command, expected) in [
        ("gradle --version", vec!["path-gradle", "--version"]),
        ("mvn --version test", vec!["path-mvn", "--version", "test"]),
    ] {
        let rewritten = fixture.rewritten(command);
        for raw_env in [false, true] {
            let mut args: Vec<_> = rewritten.iter().map(String::as_str).collect();
            if !raw_env {
                args.insert(0, "--raw");
            }
            let output = fixture.run(&args, raw_env);
            assert_eq!(output.status.code(), Some(37), "{command}: {output:?}");
            assert_eq!(fixture.argv(), expected, "{command}: raw_env={raw_env}");
        }
    }
}

#[test]
fn raw_rewrites_of_local_wrappers_do_not_execute_path_standins() {
    let fixture = Fixture::new();
    #[cfg(unix)]
    let cases = [
        ("./gradlew build", "local-gradlew"),
        ("./mvnw test", "local-mvnw"),
    ];
    #[cfg(windows)]
    let cases = [
        ("./gradlew.bat build", "local-gradlew.bat"),
        ("./mvnw.cmd test", "local-mvnw.cmd"),
    ];
    for (command, expected) in cases {
        let rewritten = fixture.rewritten(command);
        for raw_env in [false, true] {
            let mut args: Vec<_> = rewritten.iter().map(String::as_str).collect();
            if !raw_env {
                args.insert(0, "--raw");
            }
            let output = fixture.run(&args, raw_env);
            assert_eq!(output.status.code(), Some(37), "{command}: {output:?}");
            assert_eq!(fixture.argv()[0], expected, "{command}");
            assert_eq!(fixture.argv().len(), 2, "private options reached the tool");
        }
    }
}

#[test]
fn manual_jvm_conveniences_keep_their_existing_wrapper_discovery() {
    let fixture = Fixture::new();
    #[cfg(unix)]
    let cases = [("gradlew", "gradlew", "gradle"), ("mvn", "mvnw", "mvn")];
    #[cfg(windows)]
    let cases = [
        ("gradlew", "gradlew.bat", "gradle"),
        ("mvn", "mvnw.cmd", "mvn"),
    ];
    for (subcommand, wrapper, fallback) in cases {
        let output = fixture.bdo(&[subcommand, "--version"]);
        assert_eq!(output.status.code(), Some(37));
        assert_eq!(
            fixture.argv(),
            [format!("local-{wrapper}"), "--version".into()]
        );
        fs::remove_file(fixture.work.join(wrapper)).unwrap();
        let output = fixture.bdo(&[subcommand, "--version"]);
        assert_eq!(output.status.code(), Some(37));
        assert_eq!(
            fixture.argv(),
            [format!("path-{fallback}"), "--version".into()]
        );
    }
}

#[test]
fn explicit_local_wrappers_stay_local_and_never_fall_back() {
    let fixture = Fixture::new();
    #[cfg(unix)]
    let cases = [("./gradlew build", "gradlew"), ("./mvnw test", "mvnw")];
    #[cfg(windows)]
    let cases = [
        ("./gradlew.bat build", "gradlew.bat"),
        ("./mvnw.cmd test", "mvnw.cmd"),
    ];
    for (command, wrapper) in cases {
        let rewritten = fixture.rewritten(command);
        let args: Vec<_> = rewritten.iter().map(String::as_str).collect();
        let output = fixture.bdo(&args);
        assert_eq!(output.status.code(), Some(37), "{command}: {output:?}");
        let expected = format!("local-{wrapper}");
        assert_eq!(fixture.argv()[0], expected);

        fs::remove_file(fixture.work.join(wrapper)).unwrap();
        let output = fixture.bdo(&args);
        assert_ne!(
            output.status.code(),
            Some(37),
            "{command} fell back: {output:?}"
        );
        assert!(!fixture.log.exists(), "{command} launched another program");
    }
}

#[cfg(unix)]
#[test]
fn bare_wrappers_keep_path_lookup_instead_of_becoming_local_paths() {
    let fixture = Fixture::new();
    for (command, expected) in [
        ("gradlew build", "path-gradlew"),
        ("gradlew.bat clean", "path-gradlew.bat"),
        ("mvnw test", "path-mvnw"),
        ("mvnw.cmd package", "path-mvnw.cmd"),
    ] {
        let rewritten = fixture.rewritten(command);
        let args: Vec<_> = rewritten.iter().map(String::as_str).collect();
        let output = fixture.bdo(&args);
        assert_eq!(output.status.code(), Some(37), "{command}: {output:?}");
        assert_eq!(fixture.argv()[0], expected);
    }
}

#[test]
fn filtered_quiet_verbose_and_unknown_tasks_keep_the_same_program() {
    let fixture = Fixture::new();
    for (subcommand, program, tail) in [
        ("gradlew", "gradle", vec!["build"]),
        ("gradlew", "gradle", vec!["test"]),
        ("gradlew", "gradle", vec!["lint"]),
        ("gradlew", "gradle", vec!["dependencies"]),
        ("gradlew", "gradle", vec!["build", "--info"]),
        ("gradlew", "gradle", vec!["--version"]),
        ("mvn", "mvn", vec!["compile"]),
        ("mvn", "mvn", vec!["test"]),
        ("mvn", "mvn", vec!["package"]),
        ("mvn", "mvn", vec!["-q", "test"]),
        ("mvn", "mvn", vec!["-q", "--version"]),
        ("mvn", "mvn", vec!["test", "-X"]),
        ("mvn", "mvn", vec!["--version"]),
    ] {
        let mut args = vec![subcommand, "--bdo-executable", program, "--"];
        args.extend(&tail);
        let output = fixture.bdo(&args);
        assert_eq!(output.status.code(), Some(37), "{args:?}: {output:?}");
        let mut expected = vec![format!("path-{program}")];
        expected.extend(tail.into_iter().map(str::to_string));
        assert_eq!(fixture.argv(), expected, "{args:?}");
    }
}

#[test]
fn tool_arguments_cannot_override_the_private_executable_selector() {
    let fixture = Fixture::new();
    for (subcommand, program, other) in
        [("gradlew", "gradle", "./gradlew"), ("mvn", "mvn", "./mvnw")]
    {
        let args = [
            subcommand,
            "--bdo-executable",
            program,
            "--",
            "--bdo-executable",
            other,
            "file with spaces",
        ];
        for raw_env in [false, true] {
            let output = fixture.run(&args, raw_env);
            assert_eq!(output.status.code(), Some(37), "{args:?}: {output:?}");
            assert_eq!(
                fixture.argv(),
                [
                    format!("path-{program}"),
                    "--bdo-executable".into(),
                    other.into(),
                    "file with spaces".into()
                ]
            );
        }
    }
}

#[test]
fn invalid_selectors_fail_without_launching_any_program() {
    let fixture = Fixture::new();
    for subcommand in ["gradlew", "mvn"] {
        for raw_env in [false, true] {
            let output = fixture.run(
                &[subcommand, "--bdo-executable", "sh", "--", "build"],
                raw_env,
            );
            assert!(!output.status.success());
            assert!(
                !fixture.log.exists(),
                "invalid selector launched a JVM tool"
            );
        }
    }
}
