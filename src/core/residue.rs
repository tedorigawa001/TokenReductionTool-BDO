//! Shared "residue" detection: generated artifacts and high-signal stale
//! strings (legacy names, broken install URLs). Used by `bdo review` (scoped to
//! the change set) and `bdo stale` (whole tracked tree).

use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

/// Inline suppression marker: a line containing this literal string is
/// exempt from every check in this module (stale markers *and* doc command
/// drift), regardless of surrounding comment syntax (`#`, `//`, `<!-- -->`,
/// …) — the marker itself is the signal, not its wrapper.
pub const INLINE_IGNORE_MARKER: &str = "bdo-stale-ignore";

/// Load `.bdostaleignore` (gitignore-style globs) from the repo root, if present.
/// Files it matches are skipped by both `bdo stale` and `bdo review` — for files
/// that legitimately *document* residue (a changelog, a rename ledger). Absent or
/// unparseable → an empty matcher (nothing ignored).
pub fn load_ignore(root: &Path) -> ignore::gitignore::Gitignore {
    let mut b = ignore::gitignore::GitignoreBuilder::new(root);
    let _ = b.add(root.join(".bdostaleignore")); // Some(err) on read failure — ignore
    b.build()
        .unwrap_or_else(|_| ignore::gitignore::Gitignore::empty())
}

/// Generated/junk path fragments that usually should not be committed.
/// `(fragment, label)` — directory fragments (ending `/`) match only as a full
/// path segment; the rest match as a substring.
pub const ARTIFACT_MARKERS: &[(&str, &str)] = &[
    ("__pycache__/", "python bytecode dir"),
    (".pyc", "python bytecode"),
    ("target/", "cargo build output"),
    (".DS_Store", "macOS metadata"),
    ("node_modules/", "node dependencies"),
    (".orig", "merge leftover"),
    (".rej", "patch reject"),
    (".bak", "backup file"),
];

/// If `path` looks like a generated/committed-by-mistake artifact, the reason.
pub fn artifact_reason(path: &str) -> Option<&'static str> {
    ARTIFACT_MARKERS
        .iter()
        .find(|(frag, _)| {
            if let Some(dir) = frag.strip_suffix('/') {
                // Directory marker: match only as a full path segment, so
                // `mytarget/x` doesn't trip the `target/` rule.
                path.starts_with(frag) || path.contains(&format!("/{dir}/"))
            } else {
                // Suffix/substring marker (.pyc, .DS_Store, .bak, …).
                path.contains(frag)
            }
        })
        .map(|(_, label)| *label)
}

/// High-signal stale strings that are almost always a mistake in this repo.
/// Built with `concat!` so the patterns are not contiguous in this source file
/// (otherwise scanning bdo's own tree would flag this very list).
pub fn stale_markers() -> Vec<(String, &'static str)> {
    vec![
        (
            concat!("cargo install ", "bdo").to_string(),
            "wrong crate name (use --git or `bushido`)",
        ),
        (concat!("rtk", "-rewrite").to_string(), "legacy hook script name"),
        (
            concat!("rtk", "-hook-version").to_string(),
            "legacy hook version marker",
        ),
        (
            concat!("rtk", "-awareness").to_string(),
            "legacy awareness file name",
        ),
        (concat!(".config/", "rtk").to_string(), "legacy config dir"),
        (
            concat!("blob/", "master").to_string(),
            "stale master-branch URL (default branch is main; use raw for downloads)",
        ),
        (concat!("feat/", "all-features").to_string(), "obsolete fork branch"),
    ]
}

/// Scan `content` for stale markers, returning `(1-based line number, label)`
/// for each matching line (at most one hit per line). A line carrying
/// [`INLINE_IGNORE_MARKER`] is skipped — for documented, intentional mentions
/// that don't warrant a whole-file `.bdostaleignore` entry.
pub fn scan_stale(content: &str) -> Vec<(usize, &'static str)> {
    let markers = stale_markers();
    let mut hits = Vec::new();
    for (lineno, line) in content.lines().enumerate() {
        if line.contains(INLINE_IGNORE_MARKER) {
            continue;
        }
        for (pat, label) in &markers {
            if line.contains(pat.as_str()) {
                hits.push((lineno + 1, *label));
                break;
            }
        }
    }
    hits
}

/// Matches a `` `bdo <word>` `` reference inside a single-backtick code span
/// — restricted to backtick spans (not fenced blocks) because that's where
/// doc examples cite real subcommands; free-flowing prose about bdo is
/// rarely backtick-wrapped, so this keeps false positives low.
fn doc_command_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`bdo\s+([a-z][a-z0-9_-]*)").unwrap())
}

/// Find `` `bdo <word>` `` references whose `<word>` isn't in
/// `valid_commands` — a doc example naming a subcommand that was renamed or
/// removed. `valid_commands` should come from the CLI's own clap definition
/// (the source of truth), not a hand-maintained list, so this check can't
/// itself drift from what's shipped. Honors [`INLINE_IGNORE_MARKER`].
pub fn scan_doc_command_drift(content: &str, valid_commands: &[String]) -> Vec<(usize, String)> {
    let re = doc_command_regex();
    let mut hits = Vec::new();
    for (lineno, line) in content.lines().enumerate() {
        if line.contains(INLINE_IGNORE_MARKER) {
            continue;
        }
        for cap in re.captures_iter(line) {
            let cmd = &cap[1];
            if !valid_commands.iter().any(|c| c == cmd) {
                hits.push((
                    lineno + 1,
                    format!("`bdo {cmd}` — not a known subcommand (bdo --help doesn't list it)"),
                ));
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_artifact_reason_dir_segment_vs_substring() {
        assert_eq!(artifact_reason("a/__pycache__/x.pyc"), Some("python bytecode dir"));
        assert_eq!(artifact_reason("target/debug/bdo"), Some("cargo build output"));
        assert_eq!(artifact_reason("src/foo.bak"), Some("backup file"));
        // `mytarget/` must not trip the `target/` segment rule.
        assert_eq!(artifact_reason("src/mytarget/x.rs"), None);
        assert_eq!(artifact_reason("src/core/filter.rs"), None);
    }

    #[test]
    fn test_scan_stale_reports_line_and_label() {
        let content = format!(
            "ok line\nrun: {}\nanother ok\n",
            concat!("cargo install ", "bdo")
        );
        let hits = scan_stale(&content);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, 2); // 1-based line number
        assert!(hits[0].1.contains("wrong crate name"));
    }

    #[test]
    fn test_scan_stale_clean_content() {
        assert!(scan_stale("a perfectly normal file\nwith no residue\n").is_empty());
    }

    #[test]
    fn test_scan_stale_inline_ignore_suppresses_hit() {
        let content = format!(
            "run: {} {}\n",
            concat!("cargo install ", "bdo"),
            INLINE_IGNORE_MARKER
        );
        assert!(scan_stale(&content).is_empty());
    }

    #[test]
    fn test_scan_doc_command_drift_flags_unknown_command() {
        let valid = vec!["review".to_string(), "stale".to_string()];
        let content = "See `bdo oldcmd --flag` for details.\n";
        let hits = scan_doc_command_drift(content, &valid);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, 1);
        assert!(hits[0].1.contains("bdo oldcmd"));
    }

    #[test]
    fn test_scan_doc_command_drift_accepts_known_command() {
        let valid = vec!["review".to_string(), "git".to_string()];
        let content = "Run `bdo review --against origin/main` or `bdo git status`.\n";
        assert!(scan_doc_command_drift(content, &valid).is_empty());
    }

    #[test]
    fn test_scan_doc_command_drift_ignores_flags_and_placeholders() {
        let valid = vec!["review".to_string()];
        let content = "Usage: `bdo <command>` or `bdo --help` or `bdo -v`.\n";
        assert!(scan_doc_command_drift(content, &valid).is_empty());
    }

    #[test]
    fn test_scan_doc_command_drift_ignores_prose_outside_backticks() {
        let valid = vec!["review".to_string()];
        // Not backtick-wrapped, so it's prose, not a code example.
        let content = "bdo works well once configured.\n";
        assert!(scan_doc_command_drift(content, &valid).is_empty());
    }

    #[test]
    fn test_scan_doc_command_drift_honors_inline_ignore() {
        let valid = vec!["review".to_string()];
        let content = format!("`bdo oldcmd` {}\n", INLINE_IGNORE_MARKER);
        assert!(scan_doc_command_drift(&content, &valid).is_empty());
    }

    #[test]
    fn test_load_ignore_matches_listed_globs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".bdostaleignore"),
            "CHANGELOG.md\ndocs/*.md\n",
        )
        .unwrap();
        let ig = load_ignore(dir.path());
        assert!(ig.matched("CHANGELOG.md", false).is_ignore());
        assert!(ig.matched("docs/notes.md", false).is_ignore());
        assert!(!ig.matched("src/main.rs", false).is_ignore());
    }

    #[test]
    fn test_load_ignore_absent_file_ignores_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let ig = load_ignore(dir.path());
        assert!(!ig.matched("CHANGELOG.md", false).is_ignore());
    }
}
