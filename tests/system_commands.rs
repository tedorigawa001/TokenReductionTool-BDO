//! `bdo ls`, `deps`, `summary`, `gain` and `stale` through the real binary.
//!
//! These filters decide what an agent gets to see, so the failures that matter are
//! the quiet ones: a file left out, a failure reported as a success. Unix only:
//! the commands under test are `ls` and `sh`.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Dir {
    _tmp: tempfile::TempDir,
    path: PathBuf,
    db: PathBuf,
}

impl Dir {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        // The tracking database refuses a path through a symlink, as on macOS.
        let root = tmp.path().canonicalize().unwrap();
        let path = root.join("work");
        fs::create_dir(&path).unwrap();
        Self {
            _tmp: tmp,
            path,
            db: root.join("track.db"),
        }
    }

    fn file(&self, name: &str, content: &str) -> &Self {
        let path = self.path.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
        self
    }

    fn dir(&self, name: &str) -> &Self {
        fs::create_dir_all(self.path.join(name)).unwrap();
        self
    }

    fn run(&self, args: &[&str]) -> Output {
        run_in(&self.path, &self.db, args)
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// `git` in this directory, with no dependence on the machine's config.
    fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    fn committed_repo(&self) -> &Self {
        self.git(&["init", "-q"]);
        self.git(&["add", "-A"]);
        self.git(&[
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "x",
        ]);
        self
    }
}

fn run_in(cwd: &Path, db: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bdo"))
        .args(args)
        .current_dir(cwd)
        // `gain` reads the agent's session history under HOME: never the real one.
        .env("HOME", db.parent().unwrap())
        .env("BDO_DB_PATH", db)
        .env("BDO_TELEMETRY_DISABLED", "1")
        .output()
        .unwrap()
}

fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

// ---- ls ----

#[test]
fn ls_keeps_files_whose_names_end_in_a_dot() {
    let d = Dir::new();
    d.file("notes.", "").file("etc...", "");
    assert_eq!(lines(&d.ok(&["ls"])), ["etc...  0B", "notes.  0B"]);
}

#[test]
fn ls_empty_directory_says_so() {
    assert_eq!(Dir::new().ok(&["ls"]), "(empty)\n");
}

#[test]
fn ls_directory_flag_lists_the_directory_itself() {
    let d = Dir::new();
    d.dir("sub");
    assert_eq!(d.ok(&["ls", "-d", "."]), "./\n");
    assert_eq!(d.ok(&["ls", "-d", "sub"]), "sub/\n");
}

#[test]
fn ls_says_which_directory_each_entry_is_in() {
    let d = Dir::new();
    d.file("a/in_a.txt", "").file("b/in_b.txt", "").dir("c");
    assert_eq!(
        d.ok(&["ls", "a", "b", "c"]),
        "a:\nin_a.txt  0B\n\nb:\nin_b.txt  0B\n\nc:\n(empty)\n"
    );
}

#[test]
fn ls_recursive_keeps_the_structure() {
    let d = Dir::new();
    d.file("top.txt", "").file("sub/deep/x.txt", "");
    let out = d.ok(&["ls", "-R"]);
    let deep = out
        .find("sub/deep:\nx.txt  0B")
        .unwrap_or_else(|| panic!("{out}"));
    assert!(out.find("top.txt").unwrap() < deep, "{out}");
}

#[test]
fn ls_says_what_it_hid_and_how_to_see_it() {
    let d = Dir::new();
    d.dir("target").dir("src").file(".DS_Store", "");
    let out = d.ok(&["ls"]);
    assert_eq!(
        lines(&out),
        ["src/", "[hidden: .DS_Store target/ — use -a to show]"]
    );
    for flag in ["-a", "-A", "--all"] {
        let out = d.ok(&["ls", flag]);
        assert!(out.contains("target/"), "{flag}: {out}");
        assert!(!out.contains("hidden"), "{flag}: {out}");
    }
}

/// Noise names are about directories. A file called `build` is the user's.
#[test]
fn ls_does_not_hide_a_file_named_like_a_noise_directory() {
    let d = Dir::new();
    d.file("build", "").file("env", "").file("target", "");
    assert_eq!(
        lines(&d.ok(&["ls"])),
        ["build  0B", "env  0B", "target  0B"]
    );
}

#[test]
fn ls_recursive_does_not_list_inside_what_it_hid() {
    let d = Dir::new();
    d.file("src/a.rs", "").file("target/debug/huge.bin", "");
    let out = d.ok(&["ls", "-R"]);
    assert!(out.contains("[hidden: target/"), "{out}");
    assert!(!out.contains("huge.bin"), "{out}");
    assert!(out.contains("a.rs"), "{out}");
    // Asking for that directory by name still shows it.
    assert!(d.ok(&["ls", "-R", "target"]).contains("huge.bin"));
}

#[test]
fn ls_classify_flags_do_not_double_the_slash() {
    let d = Dir::new();
    d.dir("sub").dir("target");
    for flag in ["-F", "-p"] {
        assert_eq!(
            lines(&d.ok(&["ls", flag])),
            ["sub/", "[hidden: target/ — use -a to show]"],
            "{flag}"
        );
    }
}

/// These add a column the compact form would drop, and the listing would then
/// report directories as files.
#[test]
fn ls_flags_that_add_columns_are_passed_through() {
    let d = Dir::new();
    d.dir("sub");
    for flag in ["-i", "-s"] {
        let out = d.ok(&["ls", flag]);
        assert!(out.contains("drwx"), "{flag}: {out}");
    }
}

#[test]
fn ls_keeps_non_ascii_names_and_long_format_permissions() {
    let d = Dir::new();
    d.file("日本語.txt", "abc");
    assert_eq!(d.ok(&["ls"]), "日本語.txt  3B\n");
    assert!(d.ok(&["ls", "-l"]).ends_with("日本語.txt  3B\n"));
}

#[test]
fn ls_failure_keeps_the_exit_code_and_the_message() {
    let d = Dir::new();
    let out = d.run(&["ls", "nonexistent"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("nonexistent"));
}

// ---- deps ----

#[test]
fn deps_cargo_counts_every_way_of_declaring_a_dependency() {
    let d = Dir::new();
    d.file(
        "Cargo.toml",
        r#"
[package]
name = "x"
[dependencies]
plain = "1"
inline = { version = "2", features = ["std"] }
local = { path = "../local" }
fromgit = { git = "https://example.com/x" }
shared = { workspace = true }
dotted.workspace = true
[dependencies.tablestyle]
version = "3"
[target.'cfg(unix)'.dependencies]
libc = "0.2"
[build-dependencies]
cc = "1"
[dev-dependencies]
tempfile = "3"
"#,
    );
    let out = d.ok(&["deps"]);
    assert!(out.contains("Dependencies (8):"), "{out}");
    for line in [
        "plain (1)",
        "inline (2)",
        "local (path)",
        "fromgit (git)",
        "shared (workspace)",
        "dotted (workspace)",
        "tablestyle (3)",
        "libc (0.2)",
        "Build (1):",
        "cc (1)",
        "Dev (1):",
        "tempfile (3)",
    ] {
        assert!(out.contains(line), "missing {line}: {out}");
    }
}

#[test]
fn deps_requirements_keeps_every_form_of_requirement() {
    let d = Dir::new();
    d.file(
        "requirements.txt",
        "# header\nflask==2.0\nrequests[security]>=2\nzope.interface==5\nnumpy >= 1.2\n\
         black==22 # why\n-r other.txt\n--index-url https://example.com/simple\n",
    );
    assert_eq!(
        lines(&d.ok(&["deps"])),
        [
            "Python (requirements.txt):",
            "  Packages (6):",
            "    flask==2.0",
            "    requests[security]>=2",
            "    zope.interface==5",
            "    numpy >= 1.2",
            "    black==22",
            "    -r other.txt",
        ]
    );
}

#[test]
fn deps_requirements_does_not_print_credentials() {
    let d = Dir::new();
    let secret = format!("ghp_{}", "a1B2".repeat(9));
    d.file(
        "requirements.txt",
        &format!(
            "--index-url https://user:{secret}@example.com/simple\n\
             git+https://{secret}@github.com/o/r.git\n"
        ),
    );
    let out = d.ok(&["deps"]);
    assert!(!out.contains(&secret), "{out}");
    assert!(out.contains("Packages (1):"), "{out}");
}

#[test]
fn deps_pyproject_reads_the_dependencies_and_nothing_else() {
    // An inline array used to swallow the rest of the file as dependencies.
    let d = Dir::new();
    d.file(
        "pyproject.toml",
        r#"
[project]
name = "x"
dependencies = ["a", "b>=1"]

[project.optional-dependencies]
dev = ["pytest"]

[tool.ruff]
line-length = 100
"#,
    );
    assert_eq!(
        lines(&d.ok(&["deps"])),
        [
            "Python (pyproject.toml):",
            "  Dependencies (2):",
            "    a",
            "    b>=1",
            "  Optional/dev (1):",
            "    pytest",
        ]
    );
}

#[test]
fn deps_pyproject_multi_line_arrays_are_not_cut_up() {
    let d = Dir::new();
    d.file(
        "pyproject.toml",
        "[project]\ndependencies = [\n  \"a>=1\",  # why\n  \"b[x]; python_version<'3.9'\",\n]\n",
    );
    let out = d.ok(&["deps"]);
    assert!(out.contains("    a>=1\n"), "{out}");
    assert!(out.contains("    b[x]; python_version<'3.9'\n"), "{out}");
}

#[test]
fn deps_pyproject_reads_poetry() {
    let d = Dir::new();
    d.file(
        "pyproject.toml",
        "[tool.poetry.dependencies]\npython = \"^3.8\"\nflask = \"^2\"\n\n\
         [tool.poetry.group.test.dependencies]\npytest = \"^7\"\n\n[tool.other]\nkey = \"value\"\n",
    );
    assert_eq!(
        lines(&d.ok(&["deps"])),
        [
            "Python (pyproject.toml):",
            "  Dependencies (2):",
            "    flask (^2)",
            "    python (^3.8)",
            "  Optional/dev (1):",
            "    pytest (^7)",
        ]
    );
}

#[test]
fn deps_go_mod_marks_indirect_and_ignores_other_blocks() {
    let d = Dir::new();
    d.file(
        "go.mod",
        "module example.com/x\n\ngo 1.21\n\nrequire github.com/a/b v1.0.0\nrequire (\n\
         \tgithub.com/c/d v1.2.3\n\tgithub.com/e/f v1.0.0 // indirect\n)\n\
         replace github.com/a/b => ../b\nexclude (\n\tgithub.com/g/h v1.0.0\n)\n",
    );
    let out = d.ok(&["deps"]);
    assert!(out.contains("example.com/x (go 1.21)"), "{out}");
    assert!(out.contains("Dependencies (3):"), "{out}");
    assert!(out.contains("github.com/e/f v1.0.0 (indirect)"), "{out}");
    assert!(!out.contains("github.com/g/h"), "{out}");
}

#[test]
fn deps_package_json_includes_peer_and_optional() {
    let d = Dir::new();
    d.file(
        "package.json",
        r#"{"name":"x","version":"1","dependencies":{"a":"1"},
            "peerDependencies":{"react":"18"},"optionalDependencies":{"o":"2"},
            "devDependencies":{"t":"3"}}"#,
    );
    assert_eq!(
        lines(&d.ok(&["deps"])),
        [
            "Node.js (package.json):",
            "  x @ 1",
            "  Dependencies (1):",
            "    a (1)",
            "  Peer (1):",
            "    react (18)",
            "  Optional (1):",
            "    o (2)",
            "  Dev Dependencies (1):",
            "    t (3)",
        ]
    );
}

#[test]
fn deps_one_broken_manifest_does_not_hide_the_others() {
    let d = Dir::new();
    d.file("package.json", "{ not json")
        .file("requirements.txt", "x==1\n");
    let out = d.ok(&["deps"]);
    assert!(out.contains("could not parse"), "{out}");
    assert!(out.contains("x==1"), "{out}");
}

#[test]
fn deps_paths() {
    let d = Dir::new();
    d.file("requirements.txt", "x==1\n");
    // A file means the directory it is in, including a bare file name.
    assert!(d.ok(&["deps", "requirements.txt"]).contains("x==1"));
    assert_eq!(d.ok(&["deps", "."]).lines().count(), 3);

    let empty = Dir::new();
    assert_eq!(empty.ok(&["deps"]), "No dependency files found in .\n");

    let missing = d.run(&["deps", "nope"]);
    assert!(!missing.status.success(), "{missing:?}");
    assert!(String::from_utf8_lossy(&missing.stderr).contains("nope does not exist"));
}

#[test]
fn deps_caps_long_lists_and_says_how_many_more() {
    let d = Dir::new();
    let many: String = (0..200).map(|i| format!("pkg{i}==1\n")).collect();
    d.file("requirements.txt", &many);
    let out = d.ok(&["deps"]);
    assert!(out.contains("Packages (200):"), "{out}");
    assert!(out.contains(" more\n"), "{out}");
    assert!(out.lines().count() < 200, "{out}");
}

// ---- summary ----

fn summary(command: &str) -> (String, i32) {
    let d = Dir::new();
    let out = d.run(&["summary", command]);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap(),
    )
}

/// `cargo test` prints one totals line per binary. The last one used to win, so a
/// failure followed by empty doc tests read as nothing having failed.
#[test]
fn summary_adds_up_the_totals_of_every_test_binary() {
    let (out, code) = summary(
        "echo 'test result: FAILED. 3 passed; 2 failed; 1 ignored';\
         echo 'test result: ok. 4 passed; 0 failed; 0 ignored';\
         echo 'test result: ok. 0 passed; 0 failed; 0 ignored';\
         echo test; exit 101",
    );
    assert_eq!(code, 101);
    assert!(out.contains("[FAIL] Command"), "{out}");
    assert!(out.contains("[ok] 7 passed"), "{out}");
    assert!(out.contains("[FAIL] 2 failed"), "{out}");
    assert!(out.contains("skip 1 skipped"), "{out}");
}

#[test]
fn summary_lists_a_line_saying_ten_failed() {
    // `0 failed` must not match the end of `10 failed`.
    let (out, _) = summary("echo run test; echo 'tests: 10 failed, 1 passed'; exit 1");
    assert!(out.contains("[FAIL] 10 failed"), "{out}");
    assert!(out.contains("• tests: 10 failed, 1 passed"), "{out}");
}

#[test]
fn summary_does_not_count_passes_from_words_containing_ok() {
    let (out, _) =
        summary("pytest_stand_in=1; echo 'look at this book'; echo 'test a ... ok'; echo test");
    assert!(out.contains("[ok] 1 passed"), "{out}");
}

#[test]
fn summary_does_not_take_latest_for_a_test_run() {
    let (out, _) = summary("echo hello # latest");
    assert!(!out.contains("passed"), "{out}");
    assert!(out.contains("hello"), "{out}");
}

/// Tests that never ran, such as a compile error: the reason has to be visible.
#[test]
fn summary_of_a_test_run_with_no_results_shows_the_output() {
    let (out, code) = summary("echo 'could not find the crate'; true test; exit 2");
    assert_eq!(code, 2);
    assert!(out.contains("could not find the crate"), "{out}");
    assert!(!out.contains("0 passed"), "{out}");
}

#[test]
fn summary_does_not_call_a_failed_build_successful() {
    let (out, code) = summary("echo build; echo 'linker gave up'; exit 3");
    assert_eq!(code, 3);
    assert!(out.contains("[FAIL] Command"), "{out}");
    assert!(!out.contains("successful"), "{out}");
    assert!(out.contains("linker gave up"), "{out}");
    let (out, _) = summary("echo build");
    assert!(out.contains("[ok] Build successful"), "{out}");
}

#[test]
fn summary_counts_ten_errors_as_errors() {
    let (out, _) = summary("echo compile; echo 'finished with 10 errors'; exit 1");
    assert!(out.contains("[error] 1 errors"), "{out}");
    assert!(out.contains("finished with 10 errors"), "{out}");
    let (out, _) = summary("echo compile; echo 'finished with 0 errors'");
    assert!(out.contains("[ok] Build successful"), "{out}");
}

#[test]
fn summary_of_a_log_quotes_the_errors() {
    let (out, code) =
        summary("printf 'error: something specific broke\\nwarn: careful\\n[info] x\\n'; exit 1");
    assert_eq!(code, 1);
    assert!(out.contains("[error] 1 errors"), "{out}");
    assert!(out.contains("• error: something specific broke"), "{out}");
    assert!(out.contains("• warn: careful"), "{out}");
}

#[test]
fn summary_quotes_a_few_lines_and_says_how_many_more() {
    let (out, _) = summary("for i in 1 2 3 4 5 6 7 8; do echo \"error: e$i\"; done");
    assert!(out.contains("• error: e5"), "{out}");
    assert!(!out.contains("• error: e6"), "{out}");
    assert!(out.contains("... +3 more"), "{out}");
}

#[test]
fn summary_generic_does_not_drop_the_middle_of_a_short_output() {
    let words = "a b c d e f g h i j k";
    let (out, _) = summary(&format!(
        "for i in 1 2 3 4 5 6 7 8; do echo \"{words} $i\"; done"
    ));
    for i in 1..=8 {
        assert!(out.contains(&format!("{words} {i}")), "{i}: {out}");
    }
    assert!(!out.contains(" lines\n   a b"), "{out}");

    // A longer one says how much of it is left out.
    let (out, _) = summary(&format!(
        "for i in 1 2 3 4 5 6 7 8 9 10 11 12; do echo \"{words} $i\"; done"
    ));
    assert!(out.contains("... +4 lines"), "{out}");
    assert!(out.contains(&format!("{words} 5")), "{out}");
    assert!(!out.contains(&format!("{words} 6")), "{out}");
    assert!(out.contains(&format!("{words} 12")), "{out}");
}

#[test]
fn summary_counts_lines_of_output_exactly() {
    assert!(summary("echo one").0.contains("   1 lines of output"));
    assert!(summary("echo one; echo two >&2")
        .0
        .contains("   2 lines of output"));
    let (out, _) = summary("true");
    assert!(out.contains("   0 lines of output"), "{out}");
    assert!(out.contains("(no output)"), "{out}");
}

#[test]
fn summary_of_json_ignores_stderr_after_it() {
    let (out, _) = summary("echo '{\"a\":1}'; echo noise >&2");
    assert!(out.contains("Object with 1 keys"), "{out}");
    let (out, _) = summary("echo '[1,2,3]'");
    assert!(out.contains("Array with 3 items"), "{out}");
    let (out, _) = summary("echo '{oops'");
    assert!(out.contains("(Invalid JSON)"), "{out}");
}

#[test]
fn summary_lists_and_non_ascii_output_survive() {
    let (out, _) = summary("echo one; echo two");
    assert!(out.contains("List (2 items):"), "{out}");
    let long = "日本語の出力".repeat(30);
    let (out, code) = summary(&format!("echo {long}"));
    assert_eq!(code, 0);
    assert!(out.contains("日本語の出力"), "{out}");
}

// ---- gain ----

fn gain_with_history() -> Dir {
    let d = Dir::new();
    d.file("a.txt", "one\ntwo\n");
    d.ok(&["ls"]);
    d.ok(&["read", "a.txt"]);
    d
}

#[test]
fn gain_with_no_data_says_so() {
    let d = Dir::new();
    assert!(d.ok(&["gain"]).contains("No tracking data yet."));
}

#[test]
fn gain_history_survives_a_non_ascii_command() {
    // A byte offset 22 into this name falls inside a character, which panicked.
    let d = Dir::new();
    let name = "日本語のとても長いディレクトリ名前です";
    d.dir(name);
    d.ok(&["ls", name]);
    let out = d.ok(&["gain", "--history"]);
    assert!(out.contains("Recent Commands"), "{out}");
    assert!(out.contains("bdo ls -la 日本語"), "{out}");
}

#[test]
fn gain_views_show_the_same_totals() {
    let d = gain_with_history();
    let json: serde_json::Value =
        serde_json::from_str(&d.ok(&["gain", "--format", "json", "--all"])).unwrap();
    assert_eq!(json["summary"]["total_commands"], 2);
    assert_eq!(json["daily"][0]["commands"], 2);
    assert_eq!(json["weekly"][0]["commands"], 2);
    assert_eq!(json["monthly"][0]["commands"], 2);

    let text = d.ok(&["gain"]);
    assert!(text.contains("Total commands:    2"), "{text}");

    for (flag, title) in [
        ("--daily", "Daily Breakdown (1 day)"),
        ("--weekly", "Weekly Breakdown (1 week)"),
        ("--monthly", "Monthly Breakdown (1 month)"),
    ] {
        let out = d.ok(&["gain", flag]);
        assert!(lines(&out).contains(&title), "{flag}: {out}");
    }
    let out = d.ok(&["gain", "--graph"]);
    assert!(out.contains("Daily Savings"), "{out}");
    assert!(d.ok(&["gain", "--by-agent"]).contains("By Agent"));
}

#[test]
fn gain_csv_without_a_period_is_not_empty() {
    let d = gain_with_history();
    let out = d.ok(&["gain", "--format", "csv"]);
    let rows = lines(&out);
    assert_eq!(rows.len(), 2, "{out}");
    assert!(rows[0].starts_with("commands,input_tokens,"), "{out}");
    assert!(rows[1].starts_with("2,"), "{out}");
    let all = d.ok(&["gain", "--format", "csv", "--all"]);
    for title in ["# Daily Data", "# Weekly Data", "# Monthly Data"] {
        assert!(all.contains(title), "{title}: {all}");
    }
}

#[test]
fn gain_refuses_values_it_does_not_know() {
    let d = gain_with_history();
    for args in [
        &["gain", "--format", "nonsense"][..],
        &["gain", "--quota", "--tier", "nope"][..],
    ] {
        let out = d.run(args);
        assert!(!out.status.success(), "{args:?}: {out:?}");
    }
    for tier in ["pro", "5x", "20x"] {
        let out = d.ok(&["gain", "--quota", "--tier", tier]);
        assert!(out.contains("Monthly Quota Analysis"), "{tier}: {out}");
    }
}

#[test]
fn gain_project_scope_only_counts_this_project() {
    let d = Dir::new();
    d.dir("one").dir("two");
    let (one, two) = (d.path.join("one"), d.path.join("two"));
    assert!(run_in(&one, &d.db, &["ls"]).status.success());
    assert!(run_in(&two, &d.db, &["ls"]).status.success());
    assert!(run_in(&two, &d.db, &["ls"]).status.success());

    let count = |cwd: &Path, scope: &[&str]| -> serde_json::Value {
        let mut args = vec!["gain", "--format", "json"];
        args.extend(scope);
        let out = run_in(cwd, &d.db, &args);
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        json["summary"]["total_commands"].clone()
    };
    assert_eq!(count(&one, &["--project"]), 1);
    assert_eq!(count(&two, &["--project"]), 2);
    assert_eq!(count(&one, &[]), 3);
}

#[test]
fn gain_reset_needs_confirmation() {
    let d = gain_with_history();
    // With no terminal to answer on, the answer is no.
    assert!(d.ok(&["gain", "--reset"]).contains("Aborted."));
    assert!(d.ok(&["gain"]).contains("Total commands:    2"));

    d.ok(&["gain", "--reset", "--yes"]);
    assert!(d.ok(&["gain"]).contains("No tracking data yet."));
}

// ---- stale ----

fn stale_marker() -> String {
    // Assembled so that `bdo stale` does not flag this test file.
    format!("{}-rewrite", "rtk")
}

#[test]
fn stale_clean_repo_exits_zero() {
    let d = Dir::new();
    d.file("a.txt", "fine\n").committed_repo();
    let out = d.ok(&["stale"]);
    assert!(out.contains("scanned 1 tracked files"), "{out}");
    assert!(out.contains("✓ clean"), "{out}");
}

#[test]
fn stale_reports_each_kind_of_residue_and_exits_nonzero() {
    let d = Dir::new();
    d.file("a.pyc", "x")
        .file("日本語.pyc", "x")
        .file("old.txt", &format!("line\n{}\n", stale_marker()))
        .file("doc.md", "run `bdo nonsense-cmd --x`\n")
        .committed_repo();
    let out = d.run(&["stale"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("a.pyc  [python bytecode]"), "{text}");
    assert!(text.contains("日本語.pyc  [python bytecode]"), "{text}");
    assert!(text.contains("old.txt:2"), "{text}");
    assert!(text.contains("doc.md:1"), "{text}");
    assert!(text.contains("nonsense-cmd"), "{text}");
    assert!(text.contains("4 residue item(s) found"), "{text}");
}

#[test]
fn stale_does_not_flag_a_real_command_in_a_doc() {
    let d = Dir::new();
    d.file("doc.md", "run `bdo git status`\n").committed_repo();
    assert!(d.ok(&["stale"]).contains("✓ clean"));
}

#[test]
fn stale_honours_the_ignore_file() {
    let d = Dir::new();
    d.file("CHANGELOG.md", &format!("{}\n", stale_marker()))
        .file(".bdostaleignore", "CHANGELOG.md\n")
        .committed_repo();
    let out = d.ok(&["stale"]);
    assert!(out.contains("1 ignored via .bdostaleignore"), "{out}");
    assert!(out.contains("✓ clean"), "{out}");
}

#[test]
fn stale_scope_is_relative_to_where_it_runs() {
    let d = Dir::new();
    d.file("sub/in.pyc", "x")
        .file("out.pyc", "x")
        .committed_repo();
    let sub = d.path.join("sub");

    let scoped = d.run(&["stale", "sub"]);
    let text = String::from_utf8_lossy(&scoped.stdout);
    assert!(text.contains("scanned 1 tracked files"), "{text}");
    assert!(text.contains("sub/in.pyc"), "{text}");
    assert!(!text.contains("out.pyc"), "{text}");

    // Without a path it is the whole repository, from anywhere inside it.
    let whole = run_in(&sub, &d.db, &["stale"]);
    assert!(String::from_utf8_lossy(&whole.stdout).contains("scanned 2 tracked files"));
    let here = run_in(&sub, &d.db, &["stale", "."]);
    assert!(String::from_utf8_lossy(&here.stdout).contains("scanned 1 tracked files"));
}

#[test]
fn stale_refuses_what_it_cannot_scan() {
    let d = Dir::new();
    assert!(!d.run(&["stale"]).status.success(), "not a repository");
    d.file("a.txt", "x\n").committed_repo();
    let outside = d.run(&["stale", "/"]);
    assert!(!outside.status.success());
    assert!(String::from_utf8_lossy(&outside.stderr).contains("not inside the repository"));
}
