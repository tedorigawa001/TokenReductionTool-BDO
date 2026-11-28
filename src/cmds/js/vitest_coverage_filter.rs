//! Suppress all-passing file rows in a recognized Vitest coverage report.

use crate::core::utils::strip_ansi;
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref RUN: Regex = Regex::new(r"^RUN\s+v[0-9]+\.[0-9]+\.[0-9]+(?:[-.][\w.-]+)?(?:\s|$)").unwrap();
    static ref FILE_SUMMARY: Regex =
        Regex::new(r"^Test Files\s+([0-9]+) passed(?: \| [0-9]+ skipped)? \([0-9]+\)$").unwrap();
    // Vitest's file rows have one leading space; individual tests have more.
    // Recognize qualifiers for the file count, but never remove those rows.
    static ref FILE_PASS: Regex = Regex::new(
        r"^ ✓ (?:\|[^|]+\| )?\S+ \([0-9]+ tests?((?: \| [0-9]+ [a-z]+)*)\) [0-9]+(?:\.[0-9]+)?m?s$"
    ).unwrap();
}

pub fn compact(output: &str, exit_code: i32) -> Option<String> {
    if exit_code != 0 {
        return None;
    }
    let mut saw_run = false;
    let mut expected_passes = None;
    let mut saw_coverage = false;
    let mut passed_files = 0;
    let mut hide = Vec::new();

    for (index, line) in output.split_inclusive('\n').enumerate() {
        let plain = strip_ansi(line.trim_end_matches(['\r', '\n']));
        let trimmed = plain.trim();
        if RUN.is_match(trimmed) {
            if saw_run {
                return None;
            }
            saw_run = true;
        } else if let Some(summary) = FILE_SUMMARY.captures(trimmed) {
            if !saw_run || expected_passes.is_some() {
                return None;
            }
            expected_passes = Some(summary[1].parse::<usize>().ok()?);
        } else if trimmed.starts_with("% Coverage report from ") {
            saw_coverage = expected_passes.is_some();
        } else if saw_run && expected_passes.is_none() {
            let Some(file) = FILE_PASS.captures(&plain) else {
                continue;
            };
            passed_files += 1;
            if file[1].is_empty() {
                hide.push(index);
            }
        }
    }

    // Output can include arbitrary test diagnostics. A disagreement with the
    // report's own file count makes the rows ambiguous, so keep everything.
    if !saw_coverage || expected_passes != Some(passed_files) || hide.is_empty() {
        return None;
    }
    let mut hide = hide.into_iter().peekable();
    let mut filtered = String::with_capacity(output.len());
    for (index, line) in output.split_inclusive('\n').enumerate() {
        if hide.peek() == Some(&index) {
            hide.next();
        } else {
            filtered.push_str(line);
        }
    }
    Some(filtered)
}
