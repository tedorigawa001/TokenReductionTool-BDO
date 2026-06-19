//! `bdo review` — a one-shot change summary for human + agent review.
//!
//! Combines what you'd otherwise assemble by hand after editing: the changed
//! file list, generated artifacts that shouldn't be committed, stale markers
//! (legacy names / broken install URLs), and the unit tests worth running.
//! Scoped to the change set (working tree, or `--against <ref>`), not the whole
//! repo — keeping the output small, in the spirit of the rest of bdo.

use crate::core::changes::{changed_files, in_git_repo, rust_test_targets};
use crate::core::residue::{artifact_reason, scan_stale};
use crate::core::tracking;
use anyhow::Result;

pub fn run(against: Option<&str>, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if !in_git_repo() {
        anyhow::bail!("bdo review: not inside a git repository");
    }

    let changes = changed_files(against, None)?;
    let base_label = against.unwrap_or("uncommitted");

    let mut out = String::new();
    out.push_str(&format!(
        "bdo review — {} changed file(s) ({})\n",
        changes.len(),
        base_label
    ));

    if changes.is_empty() {
        out.push_str("\n✓ no changes to review\n");
        print!("{}", out);
        timer.track("review", "bdo review", "", &out);
        return Ok(());
    }

    // ── Changed files ────────────────────────────────────────────
    out.push_str("\nCHANGED\n");
    for c in &changes {
        out.push_str(&format!("  {:<2} {}\n", c.status, c.path));
    }

    // ── Suspicious artifacts ─────────────────────────────────────
    let artifacts: Vec<(&str, &str)> = changes
        .iter()
        .filter(|c| c.status != "D")
        .filter_map(|c| artifact_reason(&c.path).map(|r| (c.path.as_str(), r)))
        .collect();
    out.push_str(&section_header("⚠ ARTIFACTS", artifacts.len(), "likely should not be committed"));
    for (path, reason) in &artifacts {
        out.push_str(&format!("  {}  [{}]\n", path, reason));
    }

    // ── Stale markers (scan changed, non-deleted text files) ─────
    let mut stale_hits: Vec<String> = Vec::new();
    'scan: for c in &changes {
        if c.status == "D" {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&c.path) else {
            continue; // missing or binary
        };
        for (lineno, label) in scan_stale(&content) {
            stale_hits.push(format!("  {}:{}  {}", c.path, lineno, label));
            if stale_hits.len() >= 40 {
                stale_hits.push("  … (more; capped at 40)".to_string());
                break 'scan;
            }
        }
    }
    out.push_str(&section_header("⚠ STALE MARKERS", stale_hits.len(), "verify before commit"));
    for hit in &stale_hits {
        out.push_str(hit);
        out.push('\n');
    }

    // ── Suggested tests ──────────────────────────────────────────
    let targets = rust_test_targets(&changes);
    if targets.is_empty() {
        out.push_str("\n🧪 SUGGESTED TESTS\n  ✓ none (no Rust sources changed)\n");
    } else {
        out.push_str("\n🧪 SUGGESTED TESTS\n");
        // Multiple filters must follow `--` (libtest ORs them); bare positional
        // filters are rejected by cargo.
        out.push_str(&format!(
            "  cargo test -- {}\n",
            targets.iter().cloned().collect::<Vec<_>>().join(" ")
        ));
    }

    print!("{}", out);
    if verbose > 0 {
        eprintln!("reviewed {} changed files vs {}", changes.len(), base_label);
    }
    timer.track("review", "bdo review", "", &out);
    Ok(())
}

fn section_header(title: &str, count: usize, hint: &str) -> String {
    if count == 0 {
        format!("\n{} (0)\n  ✓ none\n", title)
    } else {
        format!("\n{} ({}) — {}\n", title, count, hint)
    }
}
