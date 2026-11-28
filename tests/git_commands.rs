//! `bdo git` through real git in temporary repositories.
//!
//! Two things have to hold whatever the filter does to the text: the command does
//! what git would have done, and the output still contains what the reader needs,
//! or says that it does not.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
    work: PathBuf,
}

fn raw_git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git")
}

/// Runs git for test setup, which has to work.
fn setup(dir: &Path, args: &[&str]) -> String {
    let out = raw_git(dir, args);
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_in(dir: &Path) {
    setup(dir, &["init", "-q", "-b", "main"]);
    setup(dir, &["config", "user.email", "t@example.com"]);
    setup(dir, &["config", "user.name", "t"]);
    setup(dir, &["config", "commit.gpgsign", "false"]);
    setup(dir, &["config", "core.autocrlf", "false"]);
}

impl Repo {
    /// A repository with one commit holding `f.txt`.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let work = root.join("work");
        fs::create_dir_all(&work).unwrap();
        init_in(&work);
        let repo = Self {
            _dir: dir,
            root,
            work,
        };
        repo.commit_file("f.txt", "one\n", "first commit");
        repo
    }

    /// `self` with a bare `origin` that has its history.
    fn with_origin() -> (Self, PathBuf) {
        let repo = Self::new();
        let origin = repo.root.join("origin.git");
        setup(
            &repo.root,
            &["init", "-q", "--bare", "-b", "main", "origin.git"],
        );
        setup(
            &repo.work,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        setup(&repo.work, &["push", "-q", "-u", "origin", "main"]);
        (repo, origin)
    }

    fn write(&self, name: &str, content: &str) {
        let path = self.work.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.work.join(name)).unwrap()
    }

    fn commit_file(&self, name: &str, content: &str, message: &str) {
        self.write(name, content);
        setup(&self.work, &["add", "--", name]);
        setup(&self.work, &["commit", "-q", "-m", message]);
    }

    fn git(&self, args: &[&str]) -> String {
        setup(&self.work, args)
    }

    fn bdo_in(&self, dir: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bdo"))
            .current_dir(dir)
            .arg("git")
            .args(args)
            .env("BDO_DB_PATH", self.root.join("track.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env_remove("BDO_RAW")
            .output()
            .expect("run bdo")
    }

    fn bdo(&self, args: &[&str]) -> Output {
        self.bdo_in(&self.work, args)
    }

    /// stdout of a run that has to succeed.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.bdo(args);
        assert!(out.status.success(), "bdo git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

fn all_output(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The same code as real git, for a command that fails.
fn assert_same_failure(repo: &Repo, args: &[&str]) {
    let real = raw_git(&repo.work, args);
    assert!(!real.status.success(), "git {args:?} was meant to fail");
    let out = repo.bdo(args);
    assert_eq!(out.status.code(), real.status.code(), "{args:?}: {out:?}");
    assert!(
        !all_output(&out).trim().is_empty(),
        "{args:?}: no explanation"
    );
}

// ---------------------------------------------------------------- status

#[test]
fn status_reports_every_kind_of_change() {
    let repo = Repo::new();
    repo.commit_file("deleted.txt", "x\n", "add");
    repo.commit_file("old-name.txt", "x\n", "add");
    repo.commit_file("staged.txt", "x\n", "add");
    repo.write("f.txt", "changed\n");
    fs::remove_file(repo.work.join("deleted.txt")).unwrap();
    repo.git(&["mv", "old-name.txt", "new-name.txt"]);
    repo.write("staged.txt", "changed\n");
    repo.git(&["add", "staged.txt"]);
    repo.write("untracked.txt", "x\n");

    let s = repo.ok(&["status"]);
    for name in [
        "f.txt",
        "deleted.txt",
        "new-name.txt",
        "staged.txt",
        "untracked.txt",
    ] {
        assert!(s.contains(name), "{name} missing: {s}");
    }
    assert!(s.contains("main"), "branch missing: {s}");
}

#[test]
fn status_keeps_awkward_file_names_readable() {
    let repo = Repo::new();
    repo.write("with space.txt", "x\n");
    repo.write("日本語.txt", "x\n");
    let s = repo.ok(&["status"]);
    assert!(s.contains("with space.txt"), "{s}");
    assert!(s.contains("日本語.txt"), "{s}");
}

#[test]
fn a_users_quotepath_setting_still_wins() {
    let repo = Repo::new();
    repo.write("日本語.txt", "x\n");
    let s = repo.ok(&["-c", "core.quotepath=true", "status"]);
    assert!(!s.contains("日本語.txt"), "{s}");
}

#[test]
fn status_says_when_the_tree_is_clean() {
    let s = Repo::new().ok(&["status"]);
    assert!(s.to_lowercase().contains("clean"), "{s}");
}

#[test]
fn status_reports_a_conflict() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "-b", "other"]);
    repo.commit_file("f.txt", "other\n", "other change");
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_file("f.txt", "main\n", "main change");
    assert!(!raw_git(&repo.work, &["merge", "other"]).status.success());
    let s = repo.ok(&["status"]);
    assert!(s.contains("f.txt"), "{s}");
}

#[test]
fn status_with_many_files_names_them_or_says_how_many_are_left() {
    let repo = Repo::new();
    for i in 0..200 {
        repo.write(&format!("file{i:03}.txt"), "x\n");
    }
    let s = repo.ok(&["status"]);
    let named = (0..200)
        .filter(|i| s.contains(&format!("file{i:03}.txt")))
        .count();
    assert!(
        named == 200 || s.contains("more") || s.contains("200"),
        "{named} named, nothing said about the rest: {s}"
    );
}

// ------------------------------------------------------------------ diff

#[test]
fn diff_shows_the_changed_lines() {
    let repo = Repo::new();
    repo.write("f.txt", "two\n");
    let s = repo.ok(&["diff"]);
    assert!(s.contains("f.txt"), "{s}");
    assert!(s.contains("-one"), "{s}");
    assert!(s.contains("+two"), "{s}");
}

#[test]
fn diff_is_empty_without_changes() {
    assert_eq!(Repo::new().ok(&["diff"]).trim(), "");
}

#[test]
fn diff_cached_covers_the_index_only() {
    let repo = Repo::new();
    repo.write("f.txt", "staged\n");
    repo.git(&["add", "f.txt"]);
    assert_eq!(repo.ok(&["diff"]).trim(), "");
    assert!(repo.ok(&["diff", "--cached"]).contains("+staged"));
}

#[test]
fn diff_quiet_and_exit_code_keep_their_meaning() {
    let repo = Repo::new();
    assert!(repo.bdo(&["diff", "--quiet"]).status.success());
    repo.write("f.txt", "two\n");
    assert_eq!(repo.bdo(&["diff", "--quiet"]).status.code(), Some(1));
    assert_eq!(repo.bdo(&["diff", "--exit-code"]).status.code(), Some(1));
}

#[test]
fn diff_of_many_files_names_them_all() {
    let repo = Repo::new();
    for i in 0..50 {
        repo.commit_file(&format!("file{i:02}.txt"), "a\n", "add");
    }
    for i in 0..50 {
        repo.write(&format!("file{i:02}.txt"), "b\n");
    }
    let s = repo.ok(&["diff"]);
    let named = (0..50)
        .filter(|i| s.contains(&format!("file{i:02}.txt")))
        .count();
    assert!(
        named == 50 || s.contains("more") || s.contains("--no-compact"),
        "{named} of 50 named, nothing said about the rest: {s}"
    );
}

#[test]
fn a_cut_diff_says_how_to_see_the_rest() {
    let repo = Repo::new();
    let lines = |word: &str| -> String { (0..500).map(|i| format!("{word} {i}\n")).collect() };
    repo.commit_file("big.txt", &lines("old"), "big");
    repo.write("big.txt", &lines("new"));
    let s = repo.ok(&["diff"]);
    let shown = (0..500)
        .filter(|i| s.contains(&format!("+new {i}\n")))
        .count();
    assert!(
        shown == 500 || s.contains("--no-compact"),
        "{shown} of 500 lines, no way to see the rest: {s}"
    );
    // ...and that way works.
    let full = repo.ok(&["diff", "--no-compact"]);
    assert!(full.contains("+new 499"), "{full}");
}

#[test]
fn diff_stat_passes_through() {
    let repo = Repo::new();
    repo.write("f.txt", "two\n");
    let s = repo.ok(&["diff", "--stat"]);
    assert!(s.contains("f.txt"), "{s}");
    assert!(s.contains("1 file changed"), "{s}");
}

// ------------------------------------------------------------------ show

#[test]
fn show_gives_the_commit_and_its_change() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "two\n", "second commit");
    let s = repo.ok(&["show"]);
    assert!(s.contains("second commit"), "{s}");
    assert!(s.contains("+two"), "{s}");
}

#[test]
fn show_combines_file_statistics_and_patch_headers() {
    let repo = Repo::new();
    repo.commit_file("changes with spaces.txt", "old\nstays\n", "add fixture");
    repo.commit_file(
        "changes with spaces.txt",
        "new\nstays\nextra\n",
        "edit fixture",
    );

    let output = repo.ok(&["show"]);
    assert_eq!(
        output.matches("changes with spaces.txt").count(),
        1,
        "{output}"
    );
    assert!(
        output.contains("changes with spaces.txt  +2 -1"),
        "{output}"
    );
    assert!(output.contains("1 file changed"), "{output}");
    assert!(output.contains("+new"), "{output}");
    assert!(output.contains("-old"), "{output}");
}

#[test]
fn show_keeps_rename_paths_and_their_exact_counts_in_one_header() {
    let repo = Repo::new();
    repo.git(&["config", "diff.renames", "true"]);
    let original = "unchanged content\n".repeat(20);
    repo.write("old name.txt", &format!("{original}old value\n"));
    repo.write("before.txt", "same content\n");
    repo.git(&["add", "--", "old name.txt", "before.txt"]);
    repo.git(&["commit", "-q", "-m", "add original files"]);
    repo.git(&["mv", "old name.txt", "new name.txt"]);
    repo.git(&["mv", "before.txt", "after.txt"]);
    repo.write("new name.txt", &format!("{original}new value\n"));
    repo.git(&["add", "--", "new name.txt"]);
    repo.git(&["commit", "-q", "-m", "rename files"]);

    let output = repo.ok(&["show", "-M"]);
    assert!(
        output.contains("old name.txt => new name.txt  +1 -1"),
        "{output}"
    );
    assert!(
        output.contains("before.txt => after.txt  +0 -0"),
        "{output}"
    );
    for path in ["old name.txt", "new name.txt", "before.txt", "after.txt"] {
        assert_eq!(output.matches(path).count(), 1, "{output}");
    }
}

#[test]
fn show_keeps_exact_counts_for_files_beyond_patch_limits() {
    let repo = Repo::new();
    for index in 0..7 {
        let name = format!("part-{index}.txt");
        repo.write(&name, &"old line\n".repeat(140));
        repo.git(&["add", "--", &name]);
    }
    repo.git(&["commit", "-q", "-m", "add files"]);
    for index in 0..7 {
        let name = format!("part-{index}.txt");
        repo.write(
            &name,
            &format!("last marker {index}\n{}", "new line\n".repeat(159)),
        );
        repo.git(&["add", "--", &name]);
    }
    repo.git(&["commit", "-q", "-m", "rewrite files"]);

    let output = repo.ok(&["show"]);
    assert!(output.contains("more changes truncated"), "{output}");
    assert!(!output.contains("last marker 6"), "{output}");
    let commit = repo.git(&["rev-parse", "HEAD"]);
    assert!(
        output.contains(&format!("[full diff: bdo --raw git show {commit}]")),
        "{output}"
    );
    for index in 0..7 {
        let name = format!("part-{index}.txt");
        assert!(output.contains(&format!("{name}  +160 -140")), "{output}");
        assert_eq!(output.matches(&name).count(), 1, "{output}");
    }
}

#[test]
fn show_keeps_git_statistics_when_patch_headers_cannot_be_matched() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "two\n", "edit fixture");
    let output = repo.ok(&["show", "--no-prefix"]);
    assert!(output.contains("f.txt | 2 +-"), "{output}");
    assert!(output.contains("1 file changed"), "{output}");
    assert!(output.contains("+two"), "{output}");
}

#[test]
fn show_with_unified_context_renders_each_change_once() {
    let repo = Repo::new();
    repo.commit_file(
        "f.txt",
        "far before\nnear before\nold value\nnear after\nfar after\n",
        "set context",
    );
    repo.write(
        "f.txt",
        "far before\nnear before\nnew value\nnear after\nfar after\n",
    );
    repo.write("other.txt", "not selected\n");
    repo.git(&["add", "--", "f.txt", "other.txt"]);
    repo.git(&["commit", "-q", "-m", "change one value"]);

    for context in ["-U1", "--unified=1"] {
        let output = repo.ok(&["show", context, "HEAD", "--", "f.txt"]);
        assert_eq!(output.matches("+new value").count(), 1, "{output}");
        assert_eq!(output.matches("-old value").count(), 1, "{output}");
        assert!(output.contains("@@ -2,3 +2,3 @@"), "{output}");
        assert!(
            output.lines().any(|line| line.trim() == "near before"),
            "{output}"
        );
        assert!(output.contains("near after"), "{output}");
        assert!(output.contains("change one value"), "{output}");
        assert!(output.contains("1 file changed"), "{output}");
        assert!(!output.contains("far after"), "{output}");
        assert!(!output.contains("other.txt"), "{output}");
        assert!(!output.contains("unchanged lines omitted"), "{output}");
    }
}

#[test]
fn show_default_keeps_only_context_next_to_hunk_edges() {
    let repo = Repo::new();
    repo.git(&["config", "diff.context", "3"]);
    let name = "f.txt";
    repo.commit_file(
        name,
        "before 0\nbefore 1\nbefore 2\nold value\nafter 0\nafter 1\nafter 2\n",
        "setup",
    );
    repo.commit_file(
        name,
        "before 0\nbefore 1\nbefore 2\nnew value\nafter 0\nafter 1\nafter 2\n",
        "edit",
    );
    let output = repo.ok(&["show"]);
    let lines: Vec<_> = output.lines().map(str::trim).collect();
    for line in ["before 2", "-old value", "+new value", "after 0"] {
        assert!(lines.contains(&line), "{line}: {output}");
    }
    for line in ["before 0", "before 1", "after 1", "after 2"] {
        assert!(!lines.contains(&line), "{line}: {output}");
    }
    assert!(
        output.contains("4 unchanged lines omitted at hunk edges"),
        "{output}"
    );
    let commit = repo.git(&["rev-parse", "HEAD"]);
    assert!(
        output.contains(&format!("[full diff: bdo --raw git show {commit}]")),
        "{output}"
    );

    let explicit = repo.ok(&["show", "-U3"]);
    for line in ["before 0", "before 1", "after 1", "after 2"] {
        assert!(
            explicit.lines().any(|item| item.trim() == line),
            "{explicit}"
        );
    }
}

#[test]
fn show_omits_long_context_but_keeps_changes_and_a_pinned_full_diff_hint() {
    let repo = Repo::new();
    let name = "file with spaces.txt";
    let shared: String = (0..10).map(|n| format!("context {n}\n")).collect();
    repo.commit_file(
        name,
        &format!("old start\n{shared}old end\n"),
        "add context fixture",
    );
    repo.commit_file(
        name,
        &format!("new start\n{shared}new end\n"),
        "change both ends",
    );
    let commit = repo.git(&["rev-parse", "HEAD"]);
    let output = repo.ok(&["show", "-U12", "HEAD", "--", name]);
    for line in [
        "-old start",
        "+new start",
        "-old end",
        "+new end",
        "context 0",
        "context 9",
    ] {
        assert!(output.contains(line), "{line}: {output}");
    }
    assert!(!output.contains("context 5"), "{output}");
    assert!(output.contains("8 unchanged lines omitted"), "{output}");
    assert!(
        output.contains(&format!("bdo --raw git show -U12 {commit} -- '{name}'")),
        "{output}"
    );
    let full = repo.git(&["show", "-U12", &commit, "--", name]);
    assert!(full.contains("context 5"), "{full}");
}

#[test]
fn show_keeps_nearest_context_and_sign_prefixed_changes() {
    let repo = Repo::new();
    let before: String = (0..7).map(|n| format!("before {n}\n")).collect();
    let after: String = (0..7).map(|n| format!("after {n}\n")).collect();
    repo.commit_file("f.txt", &format!("{before}--old marker\n{after}"), "setup");
    repo.commit_file("f.txt", &format!("{before}++new marker\n{after}"), "edit");
    let output = repo.ok(&["show", "--unified=8"]);
    for line in ["before 6", "---old marker", "+++new marker", "after 0"] {
        assert!(
            output.lines().any(|item| item.trim() == line),
            "{line}: {output}"
        );
    }
    assert_eq!(
        output.matches("6 unchanged lines omitted").count(),
        2,
        "{output}"
    );
    for line in ["before 0", "after 6"] {
        assert!(
            !output.lines().any(|item| item.trim() == line),
            "{line}: {output}"
        );
    }
}

#[test]
fn show_no_patch_still_omits_both_patch_and_stats() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "two\n", "second commit");

    for no_patch in ["-s", "--no-patch"] {
        let output = repo.ok(&["show", no_patch, "HEAD", "--", "f.txt"]);
        assert!(output.contains("second commit"), "{output}");
        assert!(!output.contains("+two"), "{output}");
        assert!(!output.contains("file changed"), "{output}");
    }
}

/// `git show REV:path` is how a file is read from history, so it is the bytes.
#[test]
fn show_of_a_file_at_a_revision_is_exact() {
    let repo = Repo::new();
    let content = "// comment\n\n\n\nfn main() {}\n   \n";
    repo.commit_file("a.rs", content, "add");
    let out = repo.bdo(&["show", "HEAD:a.rs"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), content);
}

// ------------------------------------------------------------------- log

#[test]
fn log_keeps_a_commit_body() {
    let repo = Repo::new();
    repo.write("f.txt", "two\n");
    repo.git(&["add", "f.txt"]);
    repo.git(&[
        "commit",
        "-q",
        "-m",
        "subject line",
        "-m",
        "BREAKING CHANGE: body line",
    ]);
    let s = repo.ok(&["log"]);
    assert!(s.contains("subject line"), "{s}");
    assert!(s.contains("BREAKING CHANGE: body line"), "{s}");
}

#[test]
fn log_with_a_patch_or_stat_shows_it() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "two\n", "second");
    assert!(repo.ok(&["log", "-1", "-p"]).contains("+two"));
    assert!(repo.ok(&["log", "-1", "--stat"]).contains("f.txt"));
}

/// Each of these asks for a commit's changes, so the summary must not replace them.
#[test]
fn log_options_that_imply_a_diff_are_gits_own_output() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "two\n", "second");
    for option in [
        "-p",
        "-u",
        "--patch",
        "--patch-with-stat",
        "--stat",
        "--stat=40",
        "--shortstat",
        "--numstat",
        "--summary",
        "--compact-summary",
        "--name-only",
        "--name-status",
        "--raw",
        "-U1",
        "--unified=1",
        "--word-diff",
        "--word-diff=plain",
        "--color-words",
        "-W",
        "--function-context",
    ] {
        let args = ["log", "-1", option];
        let raw = raw_git(&repo.work, &args);
        assert!(raw.status.success(), "{args:?}: {raw:?}");
        let expected = String::from_utf8_lossy(&raw.stdout);
        assert_eq!(repo.ok(&args), expected, "{args:?}");
    }
}

#[test]
fn log_keeps_the_complete_combined_merge_diff() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "-b", "other"]);
    repo.commit_file("f.txt", "other\n", "other change");
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_file("f.txt", "main\n", "main change");
    assert!(!raw_git(&repo.work, &["merge", "--no-ff", "other"])
        .status
        .success());
    repo.commit_file("f.txt", "resolved\n", "resolve merge");

    for args in [
        vec!["log", "-1", "-c"],
        vec!["log", "-1", "--cc"],
        vec!["log", "-1", "--diff-merges=combined"],
        vec!["log", "-1", "--diff-merges=dense-combined"],
    ] {
        let raw = raw_git(&repo.work, &args);
        assert!(raw.status.success(), "{args:?}: {raw:?}");
        let expected = String::from_utf8_lossy(&raw.stdout);
        assert!(expected.contains("++resolved"), "{args:?}: {expected}");
        assert_eq!(repo.ok(&args), expected, "{args:?}");
    }
}

#[test]
fn log_of_a_path_or_range_is_limited_to_it() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "x\n", "touches a");
    repo.commit_file("b.txt", "x\n", "touches b");
    let s = repo.ok(&["log", "--oneline", "--", "a.txt"]);
    assert!(s.contains("touches a"), "{s}");
    assert!(!s.contains("touches b"), "{s}");
    let s = repo.ok(&["log", "--oneline", "HEAD~1..HEAD"]);
    assert!(s.contains("touches b"), "{s}");
    assert!(!s.contains("touches a"), "{s}");
}

// -------------------------------------------------------- add and commit

#[test]
fn add_stages_what_it_is_told_to() {
    let repo = Repo::new();
    repo.write("a.txt", "x\n");
    repo.write("b.txt", "x\n");
    repo.ok(&["add", "a.txt"]);
    let staged = repo.git(&["diff", "--cached", "--name-only"]);
    assert_eq!(staged, "a.txt");
}

#[test]
fn commit_records_the_message_exactly() {
    let repo = Repo::new();
    repo.write("f.txt", "two\n");
    repo.git(&["add", "f.txt"]);
    let message = "fix: say \"hello\" & 'bye' $HOME `x`";
    repo.ok(&["commit", "-m", message, "-m", "second paragraph"]);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), message);
    assert_eq!(repo.git(&["log", "-1", "--format=%b"]), "second paragraph");
}

#[test]
fn commit_says_which_commit_it_made() {
    let repo = Repo::new();
    repo.write("f.txt", "two\n");
    repo.git(&["add", "f.txt"]);
    let s = repo.ok(&["commit", "-m", "made"]);
    let hash = repo.git(&["rev-parse", "--short", "HEAD"]);
    assert!(s.contains(&hash), "{hash} not in: {s}");
}

#[test]
fn commit_amend_replaces_the_message() {
    let repo = Repo::new();
    repo.ok(&["commit", "--amend", "-m", "reworded"]);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "reworded");
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");
}

// ------------------------------------------------- push, pull and fetch

#[test]
fn push_reaches_the_remote() {
    let (repo, origin) = Repo::with_origin();
    repo.commit_file("f.txt", "two\n", "second");
    repo.ok(&["push"]);
    assert_eq!(
        setup(&origin, &["rev-parse", "main"]),
        repo.git(&["rev-parse", "HEAD"])
    );
}

#[test]
fn a_rejected_push_fails_and_says_why() {
    let (repo, origin) = Repo::with_origin();
    repo.commit_file("f.txt", "two\n", "second");
    repo.git(&["push", "-q"]);
    repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
    repo.commit_file("f.txt", "other\n", "diverged");
    let before = setup(&origin, &["rev-parse", "main"]);

    let out = repo.bdo(&["push"]);
    assert!(!out.status.success(), "{out:?}");
    assert!(all_output(&out).contains("rejected"), "{out:?}");
    assert_eq!(setup(&origin, &["rev-parse", "main"]), before);
}

#[test]
fn pull_brings_in_commits() {
    let (repo, origin) = Repo::with_origin();
    let other = repo.root.join("other");
    setup(
        &repo.root,
        &["clone", "-q", origin.to_str().unwrap(), "other"],
    );
    setup(&other, &["config", "user.email", "t@example.com"]);
    setup(&other, &["config", "user.name", "t"]);
    fs::write(other.join("f.txt"), "from other\n").unwrap();
    setup(&other, &["commit", "-qam", "from other"]);
    setup(&other, &["push", "-q"]);

    repo.ok(&["pull"]);
    assert_eq!(repo.read("f.txt"), "from other\n");
}

#[test]
fn a_conflicting_pull_fails_and_names_the_file() {
    let (repo, origin) = Repo::with_origin();
    let other = repo.root.join("other");
    setup(
        &repo.root,
        &["clone", "-q", origin.to_str().unwrap(), "other"],
    );
    setup(&other, &["config", "user.email", "t@example.com"]);
    setup(&other, &["config", "user.name", "t"]);
    fs::write(other.join("f.txt"), "theirs\n").unwrap();
    setup(&other, &["commit", "-qam", "theirs"]);
    setup(&other, &["push", "-q"]);
    repo.commit_file("f.txt", "ours\n", "ours");
    repo.git(&["config", "pull.rebase", "false"]);

    let out = repo.bdo(&["pull"]);
    assert!(!out.status.success(), "{out:?}");
    let s = all_output(&out);
    assert!(s.contains("f.txt"), "{s}");
    assert!(s.to_lowercase().contains("conflict"), "{s}");
}

#[test]
fn fetch_updates_the_remote_ref_and_leaves_the_branch() {
    let (repo, origin) = Repo::with_origin();
    let other = repo.root.join("other");
    setup(
        &repo.root,
        &["clone", "-q", origin.to_str().unwrap(), "other"],
    );
    setup(&other, &["config", "user.email", "t@example.com"]);
    setup(&other, &["config", "user.name", "t"]);
    fs::write(other.join("f.txt"), "from other\n").unwrap();
    setup(&other, &["commit", "-qam", "from other"]);
    setup(&other, &["push", "-q"]);
    let head = repo.git(&["rev-parse", "HEAD"]);

    repo.ok(&["fetch"]);
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(
        repo.git(&["rev-parse", "origin/main"]),
        setup(&other, &["rev-parse", "HEAD"])
    );
}

// ---------------------------------------------------------------- branch

#[test]
fn branch_lists_create_and_delete() {
    let repo = Repo::new();
    repo.ok(&["branch", "feature"]);
    // Creating must not switch.
    assert_eq!(repo.git(&["branch", "--show-current"]), "main");
    assert!(repo
        .git(&["branch", "--list", "feature"])
        .contains("feature"));

    let s = repo.ok(&["branch"]);
    assert!(s.contains("main"), "{s}");
    assert!(s.contains("feature"), "{s}");

    repo.ok(&["branch", "-d", "feature"]);
    assert_eq!(repo.git(&["branch", "--list", "feature"]), "");
}

#[test]
fn branch_with_many_names_lists_them_or_says_how_many_are_left() {
    let repo = Repo::new();
    for i in 0..100 {
        repo.git(&["branch", &format!("topic-{i:03}")]);
    }
    let s = repo.ok(&["branch"]);
    let named = (0..100)
        .filter(|i| s.contains(&format!("topic-{i:03}")))
        .count();
    assert!(
        named == 100 || s.contains("more"),
        "{named} of 100, nothing said about the rest: {s}"
    );
}

#[test]
fn an_unmerged_branch_is_not_deleted_by_d() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "-b", "work"]);
    repo.commit_file("f.txt", "two\n", "unmerged");
    repo.git(&["checkout", "-q", "main"]);
    assert_same_failure(&repo, &["branch", "-d", "work"]);
    assert!(repo.git(&["branch", "--list", "work"]).contains("work"));
}

// ----------------------------------------------------------------- stash

#[test]
fn stash_saves_lists_and_restores() {
    let repo = Repo::new();
    repo.write("f.txt", "work in progress\n");
    repo.ok(&["stash"]);
    assert_eq!(repo.read("f.txt"), "one\n");

    repo.ok(&["stash", "push", "-m", "ignored when empty"]);
    let list = repo.ok(&["stash", "list"]);
    assert!(list.contains("stash@{0}"), "{list}");

    repo.ok(&["stash", "pop"]);
    assert_eq!(repo.read("f.txt"), "work in progress\n");
    assert_eq!(repo.git(&["stash", "list"]), "");
}

#[test]
fn stash_message_and_show_survive() {
    let repo = Repo::new();
    repo.write("f.txt", "work\n");
    repo.ok(&["stash", "push", "-m", "my note"]);
    assert!(repo.ok(&["stash", "list"]).contains("my note"));
    assert!(repo.ok(&["stash", "show", "-p"]).contains("+work"));
}

#[test]
fn a_conflicting_stash_pop_fails_and_keeps_the_stash() {
    let repo = Repo::new();
    repo.write("f.txt", "stashed\n");
    repo.git(&["stash", "-q"]);
    repo.commit_file("f.txt", "committed\n", "moved on");
    let out = repo.bdo(&["stash", "pop"]);
    assert!(!out.status.success(), "{out:?}");
    assert!(
        all_output(&out).to_lowercase().contains("conflict"),
        "{out:?}"
    );
    assert!(repo.git(&["stash", "list"]).contains("stash@{0}"));
}

// -------------------------------------------------------------- worktree

#[test]
fn worktree_adds_lists_and_removes() {
    let repo = Repo::new();
    let path = repo.root.join("wt");
    let path = path.to_str().unwrap();
    repo.ok(&["worktree", "add", "-q", "-b", "wt-branch", path]);
    assert!(repo.root.join("wt/f.txt").exists());

    let list = repo.ok(&["worktree", "list"]);
    assert!(list.contains("wt"), "{list}");
    assert!(list.contains("wt-branch"), "{list}");

    repo.ok(&["worktree", "remove", path]);
    assert!(!repo.root.join("wt").exists());
}

// ------------------------------------------------------ everything else

/// Commands bdo does not filter are the bytes git printed.
#[test]
fn other_commands_pass_through_unchanged() {
    let repo = Repo::new();
    for args in [
        &["rev-parse", "HEAD"][..],
        &["cat-file", "-p", "HEAD"][..],
        &["config", "user.name"][..],
        &["ls-files"][..],
    ] {
        let real = raw_git(&repo.work, args);
        let out = repo.bdo(args);
        assert_eq!(out.stdout, real.stdout, "{args:?}");
        assert_eq!(out.status.code(), real.status.code(), "{args:?}");
    }
}

#[test]
fn global_options_reach_git() {
    let repo = Repo::new();
    repo.write("f.txt", "two\n");
    let elsewhere = repo.root.clone();
    let work = repo.work.to_str().unwrap();
    let out = repo.bdo_in(&elsewhere, &["-C", work, "status"]);
    assert!(out.status.success(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("f.txt"),
        "{out:?}"
    );
}

#[test]
fn failures_keep_gits_exit_code_and_message() {
    let repo = Repo::new();
    for args in [
        &["show", "no-such-ref"][..],
        &["log", "no-such-ref"][..],
        &["diff", "no-such-ref"][..],
        &["add", "no-such-file"][..],
        &["commit", "-m", "nothing to commit"][..],
        &["push"][..],
        &["pull"][..],
        &["fetch", "no-such-remote"][..],
        &["checkout", "no-such-ref"][..],
        &["branch", "-d", "no-such-branch"][..],
        &["stash", "pop"][..],
        &["worktree", "remove", "no-such-tree"][..],
        &["no-such-command"][..],
    ] {
        assert_same_failure(&repo, args);
    }
}

#[test]
fn outside_a_repository_it_fails_like_git() {
    let dir = tempfile::tempdir().unwrap();
    for args in [&["status"][..], &["log"][..], &["diff", "HEAD"][..]] {
        let real = raw_git(dir.path(), args);
        let out = Command::new(env!("CARGO_BIN_EXE_bdo"))
            .current_dir(dir.path())
            .arg("git")
            .args(args)
            .env("BDO_DB_PATH", dir.path().join("track.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env_remove("BDO_RAW")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), real.status.code(), "{args:?}: {out:?}");
    }
}

// ------------------------------------------------- failures and no-op calls

#[test]
fn add_without_a_path_stages_nothing() {
    // Plain `git add` adds nothing; it must not become `git add .` and stage
    // files such as `.env`.
    let repo = Repo::new();
    repo.write(".env", "SECRET=1\n");
    let out = repo.bdo(&["add"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "");
}

#[test]
fn status_reports_a_broken_repository_as_git_does() {
    let repo = Repo::new();
    fs::write(repo.work.join(".git/index"), "broken").unwrap();
    assert_same_failure(&repo, &["status"]);
    let out = repo.bdo(&["status"]);
    assert!(!all_output(&out).contains("Clean working tree"), "{out:?}");
}

#[test]
fn stash_failures_keep_their_exit_code() {
    let repo = Repo::new();
    assert_same_failure(&repo, &["stash", "show", "stash@{5}"]);
    let outside = repo.root.join("not-a-repo");
    fs::create_dir_all(&outside).unwrap();
    let real = raw_git(&outside, &["stash", "list"]);
    if !real.status.success() {
        let out = repo.bdo_in(&outside, &["stash", "list"]);
        assert_eq!(out.status.code(), real.status.code(), "{out:?}");
        assert!(!all_output(&out).contains("No stashes"), "{out:?}");
    }
}

#[test]
fn worktree_runs_the_subcommand_it_was_given() {
    let repo = Repo::new();
    let out = repo.ok(&["worktree", "list", "--porcelain"]);
    assert!(out.contains("worktree "), "{out}");
    // `repair` is not `list`: git prints nothing for a healthy repository.
    let out = repo.ok(&["worktree", "repair"]);
    assert!(!out.contains("main"), "{out}");
}
