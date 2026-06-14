//! `bdo map` — a bird's-eye "repo map" of a directory tree.
//!
//! Walks the tree (respecting `.gitignore`) and, for every source file, prints
//! its top-level declarations collapsed to one line each via
//! [`crate::core::outline::signatures`]. The result lets an agent grasp a whole
//! codebase's API surface in a single command instead of `ls`-ing then reading
//! each file. Non-source files (data, unknown languages) are skipped.

use crate::core::filter::Language;
use crate::core::outline;
use crate::core::tracking;
use anyhow::Result;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

pub fn run(path: &Path, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Mapping: {}", path.display());
    }

    // Collect candidate files first so the output is sorted and deterministic.
    let mut files: Vec<PathBuf> = WalkBuilder::new(path)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| e.into_path())
        .collect();
    files.sort();

    let mut out = String::new();
    let mut raw_all = String::new();
    let mut file_count = 0usize;
    let mut sig_lines = 0usize;
    let mut source_lines = 0usize;

    for file in &files {
        let lang = file
            .extension()
            .and_then(|e| e.to_str())
            .map(Language::from_extension)
            .unwrap_or(Language::Unknown);

        // Unsupported languages (data, shell, ruby, unknown) return None — skip.
        let Some(content) = std::fs::read_to_string(file).ok() else {
            continue;
        };
        let Some(sigs) = outline::signatures(&content, &lang) else {
            continue;
        };
        let sigs = sigs.trim();
        if sigs.is_empty() {
            continue;
        }

        let display = file.strip_prefix(path).unwrap_or(file);
        out.push_str(&display.display().to_string());
        out.push('\n');
        for line in sigs.lines() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
            sig_lines += 1;
        }
        file_count += 1;
        source_lines += content.lines().count();
        raw_all.push_str(&content);
    }

    if file_count == 0 {
        out.push_str("No source files found to map.\n");
    } else {
        out.push_str(&format!(
            "\n— {} files, {} signatures (full source: {} lines)\n",
            file_count, sig_lines, source_lines
        ));
    }

    print!("{}", out);
    // Savings are relative to reading the full source of the mapped files.
    timer.track(&format!("map {}", path.display()), "bdo map", &raw_all, &out);
    Ok(())
}
