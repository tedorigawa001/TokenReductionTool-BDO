//! Shared "residue" detection: generated artifacts and high-signal stale
//! strings (legacy names, broken install URLs). Used by `bdo review` (scoped to
//! the change set) and `bdo stale` (whole tracked tree).

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
/// for each matching line (at most one hit per line).
pub fn scan_stale(content: &str) -> Vec<(usize, &'static str)> {
    let markers = stale_markers();
    let mut hits = Vec::new();
    for (lineno, line) in content.lines().enumerate() {
        for (pat, label) in &markers {
            if line.contains(pat.as_str()) {
                hits.push((lineno + 1, *label));
                break;
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
}
