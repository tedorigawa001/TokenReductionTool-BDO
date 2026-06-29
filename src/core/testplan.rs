//! Multi-language test planning for `bdo test --changed`: turn a git change set
//! into the narrowest test command(s) worth running, one per language present.
//!
//! Each language maps changed files to the tightest sensible invocation:
//! - **Rust**   → `cargo test -- <stems>` (inline `mod tests` filters)
//! - **Go**     → `go test <./pkg dirs>` (the parent package of each changed .go)
//! - **Python** → `pytest <test files> [-k "<stems>"]` (changed tests + related)
//! - **JS/TS**  → `vitest related --run <files>` / `jest --findRelatedTests <files>`
//!
//! Languages with no matching changes are omitted; an empty plan means there is
//! nothing to test in the change set.

use crate::core::changes::{rust_test_targets, Change};
use std::collections::BTreeSet;
use std::path::Path;

/// A planned test invocation for one language.
pub struct TestCommand {
    /// Short language tag for display (`rust`, `go`, `python`, `js`).
    pub lang: &'static str,
    /// The shell command to run.
    pub cmd: String,
}

/// JS/TS source extensions that a `*.test.*` file may cover.
const JS_EXTS: &[&str] = &[
    ".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs",
];

/// Single-quote one argument so the shell keeps it intact even when the path or
/// stem contains spaces or metacharacters — the commands here are run via
/// `sh -c`, where a bare `join(" ")` would split such paths into extra args.
/// (`a b` → `'a b'`, `it's` → `'it'\''s'`.)
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Quote each item and join with spaces, ready to interpolate into a command.
fn quote_join<I: IntoIterator<Item = String>>(items: I) -> String {
    items
        .into_iter()
        .map(|s| shell_quote(&s))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build the per-language test commands for a change set. `root` is the repo
/// root (used to pick the JS runner from `package.json`).
pub fn plan_changed_tests(changes: &[Change], root: &Path) -> Vec<TestCommand> {
    let mut cmds = Vec::new();

    // Rust — inline test-module filters (`cargo test -- <stem>`).
    let rust = rust_test_targets(changes);
    if !rust.is_empty() {
        let filters = quote_join(rust);
        cmds.push(TestCommand {
            lang: "rust",
            cmd: format!("cargo test -- {filters}"),
        });
    }

    // Go — the unique parent packages of changed `.go` files.
    let go = go_test_packages(changes);
    if !go.is_empty() {
        let pkgs = quote_join(go);
        cmds.push(TestCommand {
            lang: "go",
            cmd: format!("go test {pkgs}"),
        });
    }

    // Python — changed tests run directly, source files via a `-k` stem filter.
    if let Some(cmd) = python_test_cmd(changes) {
        cmds.push(TestCommand { lang: "python", cmd });
    }

    // JS/TS — the runner's "related tests" mode over changed files.
    if let Some(cmd) = js_test_cmd(changes, root) {
        cmds.push(TestCommand { lang: "js", cmd });
    }

    cmds
}

/// Unique Go packages (parent dirs, as `.` or `./path`) touched by changed,
/// non-deleted `.go` files.
pub fn go_test_packages(changes: &[Change]) -> BTreeSet<String> {
    let mut pkgs = BTreeSet::new();
    for c in changes {
        if c.status == "D" || !c.path.ends_with(".go") {
            continue;
        }
        let pkg = match c.path.rsplit_once('/') {
            Some((dir, _)) if !dir.is_empty() => format!("./{dir}"),
            _ => ".".to_string(),
        };
        pkgs.insert(pkg);
    }
    pkgs
}

/// A `pytest` invocation for changed `.py` files: changed test files run
/// directly, and the stems of changed non-test source files become a `-k`
/// filter so their related tests run too. `None` when no `.py` files changed.
pub fn python_test_cmd(changes: &[Change]) -> Option<String> {
    let mut test_files: BTreeSet<String> = BTreeSet::new();
    let mut stems: BTreeSet<String> = BTreeSet::new();
    for c in changes {
        if c.status == "D" || !c.path.ends_with(".py") {
            continue;
        }
        if is_python_test_file(&c.path) {
            test_files.insert(c.path.clone());
        } else {
            let stem = c
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&c.path)
                .trim_end_matches(".py");
            // `__init__`/`conftest`/`setup` are too generic to be useful `-k` keys.
            if !matches!(stem, "__init__" | "conftest" | "setup") {
                stems.insert(stem.to_string());
            }
        }
    }
    if test_files.is_empty() && stems.is_empty() {
        return None;
    }
    let mut cmd = String::from("pytest");
    for f in &test_files {
        cmd.push(' ');
        cmd.push_str(&shell_quote(f));
    }
    if !stems.is_empty() {
        // The `-k` value is one shell argument holding a pytest keyword
        // expression (`a or b`). The stems come from changed *filenames*, which
        // are attacker-controllable, and this runs via `sh -c` — so it must be
        // single-quoted like every other path here. Double quotes would leave
        // `$(...)`, backticks and `$IFS` live (command injection); single quotes
        // neutralize them while preserving the spaces pytest needs.
        let k = stems.into_iter().collect::<Vec<_>>().join(" or ");
        cmd.push_str(&format!(" -k {}", shell_quote(&k)));
    }
    Some(cmd)
}

/// A test file by pytest's discovery conventions: `test_*.py`, `*_test.py`, or
/// any file under a `test`/`tests` directory.
fn is_python_test_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.starts_with("test_")
        || name.ends_with("_test.py")
        || path.split('/').any(|seg| seg == "tests" || seg == "test")
}

/// A `vitest related` / `jest --findRelatedTests` invocation over changed
/// JS/TS files, using whichever runner the repo declares. `None` when no
/// JS/TS files changed.
pub fn js_test_cmd(changes: &[Change], root: &Path) -> Option<String> {
    let mut files: BTreeSet<String> = BTreeSet::new();
    for c in changes {
        if c.status == "D" {
            continue;
        }
        if JS_EXTS.iter().any(|e| c.path.ends_with(e)) {
            files.insert(c.path.clone());
        }
    }
    if files.is_empty() {
        return None;
    }
    let joined = quote_join(files);
    let cmd = match js_runner(root) {
        JsRunner::Jest => format!("npx jest --findRelatedTests {joined}"),
        JsRunner::Vitest => format!("npx vitest related --run {joined}"),
    };
    Some(cmd)
}

enum JsRunner {
    Vitest,
    Jest,
}

/// Pick the JS test runner from `package.json`: jest only when it is declared
/// and vitest is not; vitest otherwise (the modern default, also used when
/// `package.json` is missing or names neither).
fn js_runner(root: &Path) -> JsRunner {
    let pkg = std::fs::read_to_string(root.join("package.json")).unwrap_or_default();
    let names = |name: &str| pkg.contains(&format!("\"{name}\""));
    if names("jest") && !names("vitest") {
        JsRunner::Jest
    } else {
        JsRunner::Vitest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(status: &str, path: &str) -> Change {
        Change {
            status: status.into(),
            path: path.into(),
        }
    }

    #[test]
    fn test_go_packages_unique_dirs_and_root() {
        let changes = vec![
            ch("M", "cmd/app/main.go"),
            ch("A", "cmd/app/handler.go"), // same package — deduped
            ch("M", "internal/store/db.go"),
            ch("M", "root.go"),            // repo-root package → "."
            ch("D", "old/gone.go"),        // deleted — skipped
            ch("M", "README.md"),          // non-go — skipped
        ];
        let pkgs = go_test_packages(&changes);
        assert!(pkgs.contains("./cmd/app"));
        assert!(pkgs.contains("./internal/store"));
        assert!(pkgs.contains("."));
        assert!(!pkgs.iter().any(|p| p.contains("old")));
        assert_eq!(pkgs.len(), 3);
    }

    #[test]
    fn test_python_test_files_run_directly_and_sources_via_k() {
        let changes = vec![
            ch("M", "tests/test_auth.py"), // test file → direct
            ch("M", "pkg/service.py"),     // source → -k stem
            ch("M", "pkg/__init__.py"),    // generic stem → dropped
            ch("D", "pkg/gone.py"),        // deleted — skipped
        ];
        let cmd = python_test_cmd(&changes).unwrap();
        assert!(cmd.starts_with("pytest "));
        assert!(cmd.contains("'tests/test_auth.py'"));
        assert!(cmd.contains("-k 'service'"));
        assert!(!cmd.contains("__init__"));
        assert!(!cmd.contains("gone"));
    }

    #[test]
    fn test_python_k_stem_is_shell_quoted_against_injection() {
        // A malicious filename must not let `$(...)`/backticks/`$IFS` reach the
        // shell through the `-k` argument. The whole keyword expression is
        // single-quoted, so the substitution stays inert text.
        let changes = vec![ch("M", "pkg/evil$(touch${IFS}pwned).py")];
        let cmd = python_test_cmd(&changes).unwrap();
        assert!(
            cmd.contains("-k 'evil$(touch${IFS}pwned)'"),
            "k stem not single-quoted: {cmd}"
        );
        assert!(!cmd.contains("-k \""), "k must not use double quotes: {cmd}");
    }

    #[test]
    fn test_python_none_when_no_py() {
        assert!(python_test_cmd(&[ch("M", "src/main.rs")]).is_none());
    }

    #[test]
    fn test_js_defaults_to_vitest_related() {
        let dir = tempfile::tempdir().unwrap();
        let changes = vec![ch("M", "src/app.ts"), ch("D", "src/gone.ts")];
        let cmd = js_test_cmd(&changes, dir.path()).unwrap();
        assert!(cmd.starts_with("npx vitest related --run "));
        assert!(cmd.contains("'src/app.ts'"));
        assert!(!cmd.contains("gone"));
    }

    #[test]
    fn test_js_picks_jest_from_package_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{ "devDependencies": { "jest": "^29.0.0" } }"#,
        )
        .unwrap();
        let cmd = js_test_cmd(&[ch("M", "src/app.jsx")], dir.path()).unwrap();
        assert!(cmd.starts_with("npx jest --findRelatedTests "));
    }

    #[test]
    fn test_js_prefers_vitest_when_both_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{ "devDependencies": { "jest": "^29", "vitest": "^1" } }"#,
        )
        .unwrap();
        let cmd = js_test_cmd(&[ch("M", "a.ts")], dir.path()).unwrap();
        assert!(cmd.contains("vitest related"));
    }

    #[test]
    fn test_plan_groups_multiple_languages() {
        let dir = tempfile::tempdir().unwrap();
        let changes = vec![
            ch("M", "src/core/outline.rs"),
            ch("M", "cmd/app/main.go"),
            ch("M", "pkg/service.py"),
            ch("M", "web/app.ts"),
        ];
        let plan = plan_changed_tests(&changes, dir.path());
        let langs: Vec<_> = plan.iter().map(|t| t.lang).collect();
        assert_eq!(langs, vec!["rust", "go", "python", "js"]);
        assert!(plan[0].cmd.contains("cargo test -- 'outline'"));
        assert!(plan[1].cmd.contains("go test './cmd/app'"));
        assert!(plan[2].cmd.contains("pytest"));
        assert!(plan[3].cmd.contains("vitest related"));
    }

    #[test]
    fn test_paths_with_spaces_are_shell_quoted() {
        let dir = tempfile::tempdir().unwrap();
        let changes = vec![
            ch("M", "cmd/my app/main.go"),
            ch("M", "web/my view.ts"),
            ch("M", "tests/test my thing.py"),
        ];
        let plan = plan_changed_tests(&changes, dir.path());
        let go = &plan.iter().find(|t| t.lang == "go").unwrap().cmd;
        let js = &plan.iter().find(|t| t.lang == "js").unwrap().cmd;
        let py = &plan.iter().find(|t| t.lang == "python").unwrap().cmd;
        // Each space-bearing path is wrapped so the shell sees one argument.
        assert!(go.contains("'./cmd/my app'"), "go: {go}");
        assert!(js.contains("'web/my view.ts'"), "js: {js}");
        assert!(py.contains("'tests/test my thing.py'"), "py: {py}");
    }

    #[test]
    fn test_plan_empty_when_no_test_targets() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_changed_tests(&[ch("M", "README.md")], dir.path());
        assert!(plan.is_empty());
    }
}
