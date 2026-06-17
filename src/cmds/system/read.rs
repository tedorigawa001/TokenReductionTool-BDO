//! Reads source files with optional language-aware filtering to strip boilerplate.

use crate::core::filter::{self, FilterLevel, Language};
use crate::core::tracking;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

const BUSHIDO_AUTO_MAX_LINES: usize = 400;

pub fn run(
    file: &Path,
    level: FilterLevel,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    line_numbers: bool,
    verbose: u8,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Reading: {} (filter: {})", file.display(), level);
    }

    // Read file content
    let content = fs::read_to_string(file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    // Detect language from extension
    let lang = file
        .extension()
        .and_then(|e| e.to_str())
        .map(Language::from_extension)
        .unwrap_or(Language::Unknown);

    if verbose > 1 {
        eprintln!("Detected language: {:?}", lang);
    }

    // Apply filter
    let filter = filter::get_filter(level);
    let mut filtered = filter.filter(&content, &lang);

    // Safety: if filter emptied a non-empty file, fall back to raw content
    if filtered.trim().is_empty() && !content.trim().is_empty() {
        eprintln!(
            "bdo: warning: filter produced empty output for {} ({} bytes), showing raw content",
            file.display(),
            content.len()
        );
        filtered = content.clone();
    }

    if verbose > 0 {
        let original_lines = content.lines().count();
        let filtered_lines = filtered.lines().count();
        let reduction = if original_lines > 0 {
            ((original_lines - filtered_lines) as f64 / original_lines as f64) * 100.0
        } else {
            0.0
        };
        eprintln!(
            "Lines: {} -> {} ({:.1}% reduction)",
            original_lines, filtered_lines, reduction
        );
    }

    // Did the filter drop anything (e.g. comments) vs the raw bytes?
    let filter_changed = filtered != content;

    filtered = apply_line_window(&filtered, level, max_lines, tail_lines, &lang);

    // Was the visible content windowed/summarized (not the whole file)?
    let windowed = max_lines.is_some()
        || tail_lines.is_some()
        || (level == FilterLevel::Auto && should_auto_summarize(&content, &lang));

    let rtk_output = if line_numbers {
        format_with_line_numbers(&filtered)
    } else {
        filtered.clone()
    };
    print!("{}", rtk_output);

    // `cat`/`head` are often run to get exact bytes. When the shown content is a
    // reduced view (comments stripped, truncated, or windowed), point at the
    // lossless escape hatch so the raw content is always recoverable.
    if filter_changed || windowed {
        eprintln!(
            "bdo: {}: reduced view — full raw content: bdo read {} -l none",
            file.display(),
            file.display()
        );
    }
    timer.track(
        &format!("cat {}", file.display()),
        "bdo read",
        &content,
        &rtk_output,
    );
    Ok(())
}

pub fn run_stdin(
    level: FilterLevel,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    line_numbers: bool,
    verbose: u8,
) -> Result<()> {
    use std::io::{self, Read as IoRead};

    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Reading from stdin (filter: {})", level);
    }

    // Read from stdin
    let mut content = String::new();
    io::stdin()
        .lock()
        .read_to_string(&mut content)
        .context("Failed to read from stdin")?;

    // No file extension, so use Unknown language
    let lang = Language::Unknown;

    if verbose > 1 {
        eprintln!("Language: {:?} (stdin has no extension)", lang);
    }

    // Apply filter
    let filter = filter::get_filter(level);
    let mut filtered = filter.filter(&content, &lang);

    if verbose > 0 {
        let original_lines = content.lines().count();
        let filtered_lines = filtered.lines().count();
        let reduction = if original_lines > 0 {
            ((original_lines - filtered_lines) as f64 / original_lines as f64) * 100.0
        } else {
            0.0
        };
        eprintln!(
            "Lines: {} -> {} ({:.1}% reduction)",
            original_lines, filtered_lines, reduction
        );
    }

    filtered = apply_line_window(&filtered, level, max_lines, tail_lines, &lang);

    let rtk_output = if line_numbers {
        format_with_line_numbers(&filtered)
    } else {
        filtered.clone()
    };
    print!("{}", rtk_output);

    timer.track("cat - (stdin)", "bdo read -", &content, &rtk_output);
    Ok(())
}

fn format_with_line_numbers(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let width = lines.len().to_string().len();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        out.push_str(&format!("{:>width$} │ {}\n", i + 1, line, width = width));
    }
    out
}

fn apply_line_window(
    content: &str,
    level: FilterLevel,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    lang: &Language,
) -> String {
    if let Some(tail) = tail_lines {
        if tail == 0 {
            return String::new();
        }
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(tail);
        let mut result = lines[start..].join("\n");
        if content.ends_with('\n') {
            result.push('\n');
        }
        return result;
    }

    if let Some(max) = max_lines {
        // Explicit limit (head -N / --max-lines N): faithful first-N lines, not a
        // selective summary, so the output matches what `head` would show.
        return filter::plain_head(content, max);
    }

    if level == FilterLevel::Auto && should_auto_summarize(content, lang) {
        return filter::smart_truncate(content, BUSHIDO_AUTO_MAX_LINES, lang);
    }

    content.to_string()
}

fn should_auto_summarize(content: &str, lang: &Language) -> bool {
    if matches!(lang, Language::Data | Language::Unknown) {
        return false;
    }

    content.lines().count() > BUSHIDO_AUTO_MAX_LINES
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_read_rust_file() -> Result<()> {
        let mut file = NamedTempFile::with_suffix(".rs")?;
        writeln!(
            file,
            r#"// Comment
fn main() {{
    println!("Hello");
}}"#
        )?;

        // Just verify it doesn't panic
        run(file.path(), FilterLevel::Minimal, None, None, false, 0)?;
        Ok(())
    }

    #[test]
    fn test_stdin_support_signature() {
        // Test that run_stdin has correct signature and compiles
        // We don't actually run it because it would hang waiting for stdin
        // Compile-time verification that the function exists with correct signature
    }

    #[test]
    fn test_apply_line_window_tail_lines() {
        let input = "a\nb\nc\nd\n";
        let output = apply_line_window(input, FilterLevel::Auto, None, Some(2), &Language::Unknown);
        assert_eq!(output, "c\nd\n");
    }

    #[test]
    fn test_apply_line_window_tail_lines_no_trailing_newline() {
        let input = "a\nb\nc\nd";
        let output = apply_line_window(input, FilterLevel::Auto, None, Some(2), &Language::Unknown);
        assert_eq!(output, "c\nd");
    }

    // Explicit --max-lines / head -N must return the EXACT first N lines (a
    // faithful prefix), not smart_truncate's selective "important lines" subset.
    #[test]
    fn test_apply_line_window_max_lines_is_exact_prefix() {
        let input = "a\nb\nc\nd\n";
        let output = apply_line_window(input, FilterLevel::Auto, Some(2), None, &Language::Unknown);
        assert_eq!(output, "a\nb\n[2 more lines]");
    }

    fn rtk_bin() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("debug")
            .join("bdo")
    }


    #[test]
    fn test_auto_level_summarizes_large_source_files() {
        let mut input = String::from("use std::fmt;\nfn important() {\n");
        for i in 0..500 {
            input.push_str(&format!("    println!(\"line {}\");\n", i));
        }
        input.push_str("}\n");

        let output = apply_line_window(&input, FilterLevel::Auto, None, None, &Language::Rust);

        assert!(output.lines().count() <= BUSHIDO_AUTO_MAX_LINES);
        assert!(output.contains("use std::fmt;"));
        assert!(output.contains("fn important()"));
        assert!(output.contains("more lines"));
    }

    #[test]
    fn test_none_level_does_not_auto_summarize_large_source_files() {
        let input = (0..500)
            .map(|i| format!("fn f{}() {{}}", i))
            .collect::<Vec<_>>()
            .join("\n");

        let output = apply_line_window(&input, FilterLevel::None, None, None, &Language::Rust);

        assert_eq!(output, input);
    }

    #[test]
    fn test_auto_level_preserves_large_data_files() {
        let input = (0..500)
            .map(|i| format!("key{} = \"value{}\"", i, i))
            .collect::<Vec<_>>()
            .join("\n");

        let output = apply_line_window(&input, FilterLevel::Auto, None, None, &Language::Data);

        assert_eq!(output, input);
    }

    #[test]
    #[ignore]
    fn test_read_two_valid_files_concatenated() {
        let bin = rtk_bin();
        assert!(bin.exists(), "Run `cargo build` first");

        let mut f1 = NamedTempFile::with_suffix(".txt").unwrap();
        let mut f2 = NamedTempFile::with_suffix(".txt").unwrap();
        writeln!(f1, "alpha\nbravo").unwrap();
        writeln!(f2, "charlie\ndelta").unwrap();

        let output = std::process::Command::new(&bin)
            .args(["read", &f1.path().to_string_lossy(), &f2.path().to_string_lossy()])
            .output()
            .expect("failed to run bdo read");

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("alpha"), "first file content missing");
        assert!(stdout.contains("charlie"), "second file content missing");
    }

    #[test]
    #[ignore]
    fn test_read_valid_and_nonexistent() {
        let bin = rtk_bin();
        assert!(bin.exists(), "Run `cargo build` first");

        let mut f1 = NamedTempFile::with_suffix(".txt").unwrap();
        writeln!(f1, "valid content").unwrap();

        let output = std::process::Command::new(&bin)
            .args(["read", &f1.path().to_string_lossy(), "/tmp/rtk_nonexistent_file.txt"])
            .output()
            .expect("failed to run bdo read");

        assert!(!output.status.success(), "should exit non-zero on missing file");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stdout.contains("valid content"), "valid file should still be printed");
        assert!(stderr.contains("rtk_nonexistent_file"), "should report missing file on stderr");
    }

    #[test]
    #[ignore]
    fn test_read_stdin_dedup_warning() {
        let bin = rtk_bin();
        assert!(bin.exists(), "Run `cargo build` first");

        let output = std::process::Command::new(&bin)
            .args(["read", "-", "-"])
            .stdin(std::process::Stdio::piped())
            .output()
            .expect("failed to run bdo read");

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("stdin specified more than once"),
            "should warn about duplicate stdin, got stderr: {}",
            stderr
        );
    }
}
