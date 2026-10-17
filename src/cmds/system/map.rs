//! `bdo map` — a bird's-eye "repo map" of a directory tree.
//!
//! Walks the tree (respecting `.gitignore`) and, for every source file, prints
//! its top-level declarations collapsed to one line each via
//! [`crate::core::outline::signatures`]. The result lets an agent grasp a whole
//! codebase's API surface in a single command instead of `ls`-ing then reading
//! each file. Non-source files (data, unknown languages) are skipped.
//!
//! `--changed [--against <ref>]` narrows the map to the git change set, so a
//! reviewer sees just the API surface they touched.
//!
//! `--grep <PATTERN>` keeps only the signatures matching a regex (files with no
//! match are dropped), so "where is `Tracker` defined, and what takes one?" is
//! one command. Drill into a hit with `bdo map <file>` or
//! `bdo read <file> -l outline`.

use crate::core::changes;
use crate::core::filter::Language;
use crate::core::outline;
use crate::core::tracking;
use anyhow::{Context, Result};
use ignore::WalkBuilder;
use regex::Regex;
use std::path::{Path, PathBuf};

pub fn run(
    path: &Path,
    changed: bool,
    against: Option<&str>,
    grep: Option<&str>,
    verbose: u8,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    // User-supplied, so compiled once per run rather than in a lazy_static.
    let pattern = grep
        .map(|p| Regex::new(p).with_context(|| format!("bdo map --grep: invalid pattern {p:?}")))
        .transpose()?;

    if verbose > 0 {
        eprintln!("Mapping: {} (changed={})", path.display(), changed);
    }

    // Collect candidate files first so the output is sorted and deterministic.
    let files = if changed {
        changed_source_files(path, against)?
    } else {
        walk_source_files(path)
    };

    let mut out = String::new();
    let mut raw_all = String::new();
    let mut file_count = 0usize;
    let mut sig_lines = 0usize;
    let mut total_sigs = 0usize;
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
        let all: Vec<&str> = sigs.trim().lines().collect();
        total_sigs += all.len();
        let kept: Vec<&str> = match &pattern {
            Some(re) => all.into_iter().filter(|l| re.is_match(l)).collect(),
            None => all,
        };
        if kept.is_empty() {
            continue;
        }

        out.push_str(&display_path(file, path).display().to_string());
        out.push('\n');
        for line in kept {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
            sig_lines += 1;
        }
        file_count += 1;
        source_lines += content.lines().count();
        raw_all.push_str(&content);
    }

    let scope = match (changed, against) {
        (true, Some(base)) => format!(" (changed vs {})", base),
        (true, None) => " (changed)".to_string(),
        (false, _) => String::new(),
    };
    if file_count == 0 {
        match grep {
            Some(p) => out.push_str(&format!(
                "No signatures match /{}/{} ({} searched).\n",
                p, scope, total_sigs
            )),
            None => out.push_str(if changed {
                "No changed source files to map.\n"
            } else {
                "No source files found to map.\n"
            }),
        }
    } else {
        let count = match grep {
            Some(p) => format!("{} of {} signatures match /{}/", sig_lines, total_sigs, p),
            None => format!("{} signatures", sig_lines),
        };
        out.push_str(&format!(
            "\n— {} files, {}{} (full source: {} lines)\n",
            file_count, count, scope, source_lines
        ));
    }

    print!("{}", out);
    // Savings are relative to reading the full source of the mapped files.
    timer.track(&format!("map {}", path.display()), "bdo map", &raw_all, &out);
    Ok(())
}

/// How a mapped file is labelled: relative to the mapped directory, or — when
/// `bdo map` was pointed at a single file, which would strip to nothing — the
/// path as given.
fn display_path<'a>(file: &'a Path, root: &'a Path) -> &'a Path {
    match file.strip_prefix(root) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel,
        _ => file,
    }
}

/// Source files under `path`, respecting `.gitignore` (the default whole-tree map).
fn walk_source_files(path: &Path) -> Vec<PathBuf> {
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
    files
}

/// Files in the git change set (working tree, or vs `against`) that still exist
/// on disk and live under `path`. Non-source files are dropped later when
/// `outline::signatures` returns `None`.
fn changed_source_files(path: &Path, against: Option<&str>) -> Result<Vec<PathBuf>> {
    if !changes::in_git_repo() {
        anyhow::bail!("bdo map --changed: not inside a git repository");
    }
    // `path` is handed to git as a pathspec, so git resolves it (absolute,
    // relative, or a subdirectory) and returns repo-relative paths.
    let spec = (path != Path::new(".")).then_some(path);
    let mut files: Vec<PathBuf> = changes::changed_files(against, spec)?
        .into_iter()
        .filter(|c| c.status != "D") // deleted files can't be mapped
        .map(|c| PathBuf::from(c.path))
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_path_is_relative_to_the_mapped_dir() {
        assert_eq!(
            display_path(Path::new("src/core/a.rs"), Path::new("src")),
            Path::new("core/a.rs")
        );
    }

    #[test]
    fn display_path_keeps_a_single_mapped_file() {
        // `bdo map src/a.rs` used to print an empty header line.
        assert_eq!(
            display_path(Path::new("src/a.rs"), Path::new("src/a.rs")),
            Path::new("src/a.rs")
        );
    }
}
