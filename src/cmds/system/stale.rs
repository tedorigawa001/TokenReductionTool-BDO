//! `bdo stale` — scan the whole tracked tree for residue: generated artifacts
//! that slipped into git and high-signal stale strings (legacy names, broken
//! install URLs). Where `bdo review` checks the change set, `stale` audits the
//! entire repo. Exits non-zero when anything is found, so it can gate CI.

use crate::core::changes;
use crate::core::residue::{artifact_reason, scan_stale};
use crate::core::tracking;
use anyhow::Result;
use std::path::Path;

/// Cap on reported stale-string hits, to keep output bounded on large repos.
const MAX_STALE_HITS: usize = 100;

pub fn run(path: &Path, verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if !changes::in_git_repo() {
        anyhow::bail!("bdo stale: not inside a git repository");
    }
    let spec = (path != Path::new(".")).then_some(path);
    let files = changes::tracked_files(spec)?;

    // Tracked generated artifacts (path-based).
    let artifacts: Vec<(&String, &str)> = files
        .iter()
        .filter_map(|f| artifact_reason(f).map(|r| (f, r)))
        .collect();

    // Stale strings (content scan of each tracked text file).
    let mut stale: Vec<String> = Vec::new();
    let mut truncated = false;
    'scan: for f in &files {
        let Ok(content) = std::fs::read_to_string(f) else {
            continue; // missing or binary
        };
        for (lineno, label) in scan_stale(&content) {
            stale.push(format!("  {}:{}  {}", f, lineno, label));
            if stale.len() >= MAX_STALE_HITS {
                truncated = true;
                break 'scan;
            }
        }
    }

    let mut out = String::new();
    out.push_str(&format!("bdo stale — scanned {} tracked files\n", files.len()));

    out.push_str(&section_header("⚠ TRACKED ARTIFACTS", artifacts.len()));
    for (path, reason) in &artifacts {
        out.push_str(&format!("  {}  [{}]\n", path, reason));
    }

    out.push_str(&section_header("⚠ STALE MARKERS", stale.len()));
    for hit in &stale {
        out.push_str(hit);
        out.push('\n');
    }
    if truncated {
        out.push_str(&format!("  … (more; capped at {})\n", MAX_STALE_HITS));
    }

    let total = artifacts.len() + stale.len();
    out.push_str(&format!(
        "\n{}\n",
        if total == 0 {
            "✓ clean — no residue found".to_string()
        } else {
            format!("✗ {} residue item(s) found", total)
        }
    ));

    print!("{}", out);
    if verbose > 0 {
        eprintln!("scanned {} tracked files under {}", files.len(), path.display());
    }
    timer.track("stale", "bdo stale", "", &out);

    // Non-zero exit when residue is present, so `bdo stale` can gate CI.
    Ok(if total == 0 { 0 } else { 1 })
}

fn section_header(title: &str, count: usize) -> String {
    if count == 0 {
        format!("\n{} (0)\n  ✓ none\n", title)
    } else {
        format!("\n{} ({})\n", title, count)
    }
}
