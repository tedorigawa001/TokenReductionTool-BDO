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
//!
//! Every value embedded here (filenames, stems, package dirs) originates from
//! changed *paths*, which an attacker controls by naming a file. Commands are
//! therefore run as argv (`Command::new(program).args(args)`), never through a
//! shell — `args` is exec'd as-is, so there is no quoting step that could be
//! gotten wrong and no shell metacharacter (`$(...)`, backticks, `$IFS`) is ever
//! live. `display` strings are for human-readable logging only and are never
//! parsed or executed.

use crate::core::changes::{rust_test_targets, Change};
use std::collections::BTreeSet;
use std::path::Path;

/// A planned test invocation for one language.
pub struct TestCommand {
    /// Short language tag for display (`rust`, `go`, `python`, `js`).
    pub lang: &'static str,
    /// The program to exec directly (no shell).
    pub program: String,
    /// Arguments passed straight to `Command::args` — never shell-parsed.
    pub args: Vec<String>,
    /// Human-readable rendering of `program`/`args` for logs only.
    pub display: String,
}

/// JS/TS source extensions that a `*.test.*` file may cover.
const JS_EXTS: &[&str] = &[".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs"];

/// Single-quote one value for the human-readable `display` string so paths or
/// stems containing spaces still read as one token. This is cosmetic only —
/// `args` (what actually gets exec'd) carries the raw, unquoted value.
/// (`a b` → `'a b'`, `it's` → `'it'\''s'`.)
fn display_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Quote each item and join with spaces for display.
fn display_join<'a, I: IntoIterator<Item = &'a String>>(items: I) -> String {
    items
        .into_iter()
        .map(|s| display_quote(s))
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
        let mut args = vec!["test".to_string(), "--".to_string()];
        args.extend(rust.iter().cloned());
        let display = format!("cargo test -- {}", display_join(&rust));
        cmds.push(TestCommand {
            lang: "rust",
            program: "cargo".to_string(),
            args,
            display,
        });
    }

    // Go — the unique parent packages of changed `.go` files.
    let go = go_test_packages(changes);
    if !go.is_empty() {
        let mut args = vec!["test".to_string()];
        args.extend(go.iter().cloned());
        let display = format!("go test {}", display_join(&go));
        cmds.push(TestCommand {
            lang: "go",
            program: "go".to_string(),
            args,
            display,
        });
    }

    // Python — changed tests run directly, source files via a `-k` stem filter.
    if let Some((args, display)) = python_test_cmd(changes) {
        cmds.push(TestCommand {
            lang: "python",
            program: "pytest".to_string(),
            args,
            display,
        });
    }

    // JS/TS — the runner's "related tests" mode over changed files.
    if let Some((program, args, display)) = js_test_cmd(changes, root) {
        cmds.push(TestCommand {
            lang: "js",
            program,
            args,
            display,
        });
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
/// filter so their related tests run too. Returns `(args, display)`; `None`
/// when no `.py` files changed.
pub fn python_test_cmd(changes: &[Change]) -> Option<(Vec<String>, String)> {
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

    let mut args: Vec<String> = test_files.iter().cloned().collect();
    let mut display = String::from("pytest");
    for f in &test_files {
        display.push(' ');
        display.push_str(&display_quote(f));
    }
    if !stems.is_empty() {
        // One argv element holding the whole pytest keyword expression
        // (`a or b`) — pytest itself splits on `or`/`and`, no shell involved.
        let k = stems.into_iter().collect::<Vec<_>>().join(" or ");
        args.push("-k".to_string());
        display.push_str(&format!(" -k {}", display_quote(&k)));
        args.push(k);
    }
    Some((args, display))
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
/// JS/TS files, using whichever runner the repo declares. Returns
/// `(program, args, display)`; `None` when no JS/TS files changed.
pub fn js_test_cmd(changes: &[Change], root: &Path) -> Option<(String, Vec<String>, String)> {
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
    let (mut args, prefix) = match js_runner(root) {
        JsRunner::Jest => (
            vec!["jest".to_string(), "--findRelatedTests".to_string()],
            "npx jest --findRelatedTests",
        ),
        JsRunner::Vitest => (
            vec![
                "vitest".to_string(),
                "related".to_string(),
                "--run".to_string(),
            ],
            "npx vitest related --run",
        ),
    };
    args.extend(files.iter().cloned());
    let display = format!("{prefix} {}", display_join(&files));
    Some(("npx".to_string(), args, display))
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
            ch("M", "root.go"),     // repo-root package → "."
            ch("D", "old/gone.go"), // deleted — skipped
            ch("M", "README.md"),   // non-go — skipped
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
        let (args, display) = python_test_cmd(&changes).unwrap();
        assert!(display.starts_with("pytest "));
        assert!(display.contains("'tests/test_auth.py'"));
        assert!(display.contains("-k 'service'"));
        assert!(!display.contains("__init__"));
        assert!(!display.contains("gone"));
        // argv carries the raw (unquoted) values — no quoting needed off-shell.
        assert!(args.contains(&"tests/test_auth.py".to_string()));
        assert_eq!(args.last(), Some(&"service".to_string()));
        assert!(args.iter().any(|a| a == "-k"));
    }

    #[test]
    fn test_python_k_stem_cannot_inject_because_no_shell_is_involved() {
        // A malicious filename must not let `$(...)`/backticks/`$IFS` execute.
        // With argv exec there is no shell to interpret them in the first
        // place — the `-k` arg lands in pytest as inert literal text.
        let changes = vec![ch("M", "pkg/evil$(touch${IFS}pwned).py")];
        let (args, display) = python_test_cmd(&changes).unwrap();
        assert_eq!(
            args.last(),
            Some(&"evil$(touch${IFS}pwned)".to_string()),
            "args: {args:?}"
        );
        assert!(
            display.contains("-k 'evil$(touch${IFS}pwned)'"),
            "display not quoted: {display}"
        );
    }

    #[test]
    fn test_python_none_when_no_py() {
        assert!(python_test_cmd(&[ch("M", "src/main.rs")]).is_none());
    }

    #[test]
    fn test_js_defaults_to_vitest_related() {
        let dir = tempfile::tempdir().unwrap();
        let changes = vec![ch("M", "src/app.ts"), ch("D", "src/gone.ts")];
        let (program, args, display) = js_test_cmd(&changes, dir.path()).unwrap();
        assert_eq!(program, "npx");
        assert_eq!(args[0], "vitest");
        assert!(display.starts_with("npx vitest related --run "));
        assert!(display.contains("'src/app.ts'"));
        assert!(args.contains(&"src/app.ts".to_string()));
        assert!(!display.contains("gone"));
    }

    #[test]
    fn test_js_picks_jest_from_package_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{ "devDependencies": { "jest": "^29.0.0" } }"#,
        )
        .unwrap();
        let (program, args, display) = js_test_cmd(&[ch("M", "src/app.jsx")], dir.path()).unwrap();
        assert_eq!(program, "npx");
        assert_eq!(args[0], "jest");
        assert!(display.starts_with("npx jest --findRelatedTests "));
    }

    #[test]
    fn test_js_prefers_vitest_when_both_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{ "devDependencies": { "jest": "^29", "vitest": "^1" } }"#,
        )
        .unwrap();
        let (_, _, display) = js_test_cmd(&[ch("M", "a.ts")], dir.path()).unwrap();
        assert!(display.contains("vitest related"));
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
        assert!(plan[0].display.contains("cargo test -- 'outline'"));
        assert!(plan[1].display.contains("go test './cmd/app'"));
        assert!(plan[2].display.contains("pytest"));
        assert!(plan[3].display.contains("vitest related"));
    }

    #[test]
    fn test_paths_with_spaces_are_shell_quoted_for_display() {
        let dir = tempfile::tempdir().unwrap();
        let changes = vec![
            ch("M", "cmd/my app/main.go"),
            ch("M", "web/my view.ts"),
            ch("M", "tests/test my thing.py"),
        ];
        let plan = plan_changed_tests(&changes, dir.path());
        let go = &plan.iter().find(|t| t.lang == "go").unwrap();
        let js = &plan.iter().find(|t| t.lang == "js").unwrap();
        let py = &plan.iter().find(|t| t.lang == "python").unwrap();
        // Display wraps space-bearing paths so logs read as one token...
        assert!(go.display.contains("'./cmd/my app'"), "go: {}", go.display);
        assert!(
            js.display.contains("'web/my view.ts'"),
            "js: {}",
            js.display
        );
        assert!(
            py.display.contains("'tests/test my thing.py'"),
            "py: {}",
            py.display
        );
        // ...while argv carries the raw path as a single element, unquoted,
        // since it's never re-split by a shell.
        assert!(go.args.contains(&"./cmd/my app".to_string()));
        assert!(js.args.contains(&"web/my view.ts".to_string()));
        assert!(py.args.contains(&"tests/test my thing.py".to_string()));
    }

    #[test]
    fn test_plan_empty_when_no_test_targets() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_changed_tests(&[ch("M", "README.md")], dir.path());
        assert!(plan.is_empty());
    }
}
