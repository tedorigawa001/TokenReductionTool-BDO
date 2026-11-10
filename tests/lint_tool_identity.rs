//! A lint operand must never become a different executable. Probes live at
//! the process boundary so these checks run on Unix and native Windows.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        for tool in [
            "eslint", "biome", "ruff", "pylint", "mypy", "flake8", "sh", "node", "npx",
        ] {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let path = bin.join(tool);
                fs::write(
                    &path,
                    format!(
                        "#!/bin/sh\nprintf '%s\\n' '{tool}' \"$@\" > \"$BDO_TEST_ARGV\"\nprintf '[]\\n'\nexit \"$BDO_TEST_EXIT\"\n"
                    ),
                )
                .unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            #[cfg(windows)]
            fs::write(
                bin.join(format!("{tool}.cmd")),
                format!(
                    "@echo off\r\n> \"%BDO_TEST_ARGV%\" echo {tool}\r\n:args\r\nif \"%~1\"==\"\" goto done\r\n>> \"%BDO_TEST_ARGV%\" echo %~1\r\nshift\r\ngoto args\r\n:done\r\necho []\r\nexit /b %BDO_TEST_EXIT%\r\n"
                ),
            )
            .unwrap();
        }
        // These are data operands. None is an executable or linter config.
        for name in [
            "sh", "node", "ruff", "biome", "eslint", "payload", "src", "npx", "exec",
        ] {
            fs::write(root.join(name), "lint input, not executable code\n").unwrap();
        }
        let log = root.join("argv");
        Self {
            _temp: temp,
            root,
            bin,
            log,
        }
    }

    fn bdo(&self, args: &[&str]) -> Output {
        self.run(args, false, 37)
    }

    fn run(&self, args: &[&str], raw_env: bool, exit: i32) -> Output {
        let _ = fs::remove_file(&self.log);
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
            .env("BDO_TEST_ARGV", &self.log)
            .env("BDO_TEST_EXIT", exit.to_string())
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env("BDO_TEE", "0")
            .env_remove("BDO_RAW");
        if raw_env {
            command.env("BDO_RAW", "1");
        }
        command.output().expect("run bdo with isolated tool probes")
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
fn npx_eslint_keeps_its_tool_when_the_first_file_names_an_executable() {
    let fixture = Fixture::new();
    // A conventional path remains the legitimate control for the same route.
    let control = fixture.bdo(&["npx", "eslint", "./src"]);
    assert_eq!(control.status.code(), Some(37));
    assert_eq!(fixture.argv(), ["eslint", "-f", "json", "./src"]);

    for operand in ["sh", "node", "ruff", "biome", "eslint", "npx", "exec"] {
        let output = fixture.bdo(&["npx", "eslint", operand, "payload"]);
        assert_eq!(output.status.code(), Some(37), "{operand}: {output:?}");
        assert_eq!(fixture.argv(), ["eslint", "-f", "json", operand, "payload"]);
    }
}

#[test]
fn rewritten_eslint_keeps_its_tool_and_file_operands() {
    let fixture = Fixture::new();
    for prefix in [
        "eslint",
        "npx eslint",
        "npm exec eslint",
        "npm x eslint",
        "pnpm exec eslint",
        "pnpm dlx eslint",
        "pnpx eslint",
    ] {
        for operand in ["sh", "ruff"] {
            let command = format!("{prefix} {operand} payload");
            let output = fixture.bdo(&["rewrite", &command]);
            // No allow rules: exit 3 requests confirmation, but still rewrites.
            assert_eq!(output.status.code(), Some(3), "{command}: {output:?}");
            let rewritten = String::from_utf8(output.stdout).unwrap();
            assert_eq!(
                rewritten,
                format!("bdo lint eslint {operand} payload"),
                "{command}"
            );

            let args: Vec<_> = rewritten.split_whitespace().skip(1).collect();
            let output = fixture.bdo(&args);
            assert_eq!(output.status.code(), Some(37), "{command}: {output:?}");
            assert_eq!(fixture.argv(), ["eslint", "-f", "json", operand, "payload"]);
        }
    }
}

/// These run the project's script named `eslint`, which may be any command, so the
/// script runs rather than ESLint itself.
#[test]
fn script_runs_named_eslint_are_left_to_the_package_manager() {
    let fixture = Fixture::new();
    for (command, expected) in [
        ("npm run eslint sh payload", "bdo npm run eslint sh payload"),
        (
            "npm run-script eslint sh payload",
            "bdo npm run-script eslint sh payload",
        ),
        ("npm rum eslint sh payload", "bdo npm rum eslint sh payload"),
        ("npm urn eslint sh payload", "bdo npm urn eslint sh payload"),
        (
            "pnpm run eslint sh payload",
            "bdo pnpm run eslint sh payload",
        ),
        (
            "pnpm run-script eslint sh payload",
            "bdo pnpm run-script eslint sh payload",
        ),
        // pnpm runs a script of that name first; npm has no such command.
        ("pnpm eslint sh payload", ""),
        ("npm eslint sh payload", ""),
    ] {
        let output = fixture.bdo(&["rewrite", command]);
        let rewritten = String::from_utf8_lossy(&output.stdout);
        assert_eq!(rewritten.trim_end(), expected, "{command}: {output:?}");
    }
}

#[test]
fn default_lint_keeps_dotless_file_operands_as_data() {
    let fixture = Fixture::new();
    for operand in ["sh", "node", "src", "payload"] {
        let output = fixture.bdo(&["lint", operand, "payload"]);
        assert_eq!(output.status.code(), Some(37), "{operand}: {output:?}");
        assert_eq!(fixture.argv(), ["eslint", "-f", "json", operand, "payload"]);
    }
}

#[test]
fn fixed_eslint_keeps_option_separators_and_format_behavior() {
    let fixture = Fixture::new();
    let output = fixture.bdo(&["npx", "eslint", "--", "ruff", "payload"]);
    assert_eq!(output.status.code(), Some(37));
    assert_eq!(
        fixture.argv(),
        ["eslint", "-f", "json", "--", "ruff", "payload"]
    );

    let args = ["lint", "eslint", "--format", "stylish", "sh", "payload"];
    let output = fixture.bdo(&args);
    assert_eq!(output.status.code(), Some(37));
    assert_eq!(fixture.argv(), ["eslint", "-f", "json", "sh", "payload"]);
    for raw_env in [false, true] {
        let mut raw_args = args.to_vec();
        if !raw_env {
            raw_args.insert(0, "--raw");
        }
        let output = fixture.run(&raw_args, raw_env, 37);
        assert_eq!(output.status.code(), Some(37));
        assert_eq!(
            fixture.argv(),
            ["eslint", "--format", "stylish", "sh", "payload"]
        );
    }
}

#[test]
fn native_raw_npx_keeps_the_explicit_tool_and_operands() {
    let fixture = Fixture::new();
    for raw_env in [false, true] {
        let mut args = vec!["npx", "eslint", "sh", "payload"];
        if !raw_env {
            args.insert(0, "--raw");
        }
        let output = fixture.run(&args, raw_env, 37);
        assert_eq!(output.status.code(), Some(37));
        assert_eq!(fixture.argv(), ["npx", "eslint", "sh", "payload"]);
    }
}

#[test]
fn supported_explicit_linters_and_default_eslint_remain_available() {
    let fixture = Fixture::new();
    let cases: &[(&[&str], &[&str])] = &[
        (&["lint"], &["eslint", "-f", "json", "."]),
        (&["lint", "eslint", "src"], &["eslint", "-f", "json", "src"]),
        (
            &["lint", "biome", "check", "src"],
            &["biome", "check", "src"],
        ),
        (
            &["lint", "ruff", "check", "src"],
            &["ruff", "check", "--output-format=json", "src"],
        ),
        (
            &["lint", "pylint", "src"],
            &["pylint", "--output-format=json2", "src"],
        ),
        (&["lint", "mypy", "src"], &["mypy", "src"]),
        (&["lint", "flake8", "src"], &["flake8", "src"]),
        (
            &["lint", "npx", "eslint", "sh"],
            &["eslint", "-f", "json", "sh"],
        ),
    ];
    for (args, expected) in cases {
        let output = fixture.run(args, false, 0);
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        assert_eq!(fixture.argv(), *expected, "{args:?}");
    }
}
