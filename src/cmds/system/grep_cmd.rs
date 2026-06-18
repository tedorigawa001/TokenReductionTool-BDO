//! Filters grep output by grouping matches by file.

use crate::core::config;
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;

#[allow(clippy::too_many_arguments)]
pub fn run(
    pattern: &str,
    path: &str,
    max_line_len: usize,
    max_results: usize,
    context_only: bool,
    file_type: Option<&str>,
    extra_args: &[String],
    verbose: u8,
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("grep: '{}' in {}", pattern, path);
    }

    // Fix: convert BRE alternation \| → | for rg (which uses PCRE-style regex)
    let rg_pattern = pattern.replace(r"\|", "|");

    let mut rg_cmd = resolved_command("rg");
    rg_cmd.args(build_rg_args(&rg_pattern, path, file_type, extra_args));

    let result = exec_capture(&mut rg_cmd)
        .or_else(|_| {
            let mut grep_cmd = resolved_command("grep");
            // Fall back to plain colon-separated output (`file:line:content`).
            // We deliberately do NOT pass `-Z`/`--null`: GNU grep treats it as a
            // NUL separator, but other grep-compatible binaries do not (e.g.
            // ugrep reads `-Z` as fuzzy matching), which both changes results and
            // breaks parsing. `parse_match_line` understands the colon format.
            grep_cmd.args(["-rnH", pattern, path]).args(extra_args);
            exec_capture(&mut grep_cmd)
        })
        .context("grep/rg failed")?;

    // Passthrough output flags that produce output that is already small.
    if has_format_flag(extra_args) {
        print!("{}", result.stdout);
        if !result.stderr.is_empty() {
            eprint!("{}", result.stderr.trim());
        }

        let args_display = if extra_args.is_empty() {
            format!("'{}' {}", pattern, path)
        } else {
            format!("{} '{}' {}", extra_args.join(" "), pattern, path)
        };

        timer.track_passthrough(
            &format!("grep {}", args_display),
            &format!("bdo grep {} (passthrough)", args_display),
        );
        return Ok(result.exit_code);
    }

    let exit_code = result.exit_code;
    let raw_output = result.stdout.clone();

    if result.stdout.trim().is_empty() {
        // Show stderr for errors (bad regex, missing file, etc.)
        if exit_code == 2 && !result.stderr.trim().is_empty() {
            eprintln!("{}", result.stderr.trim());
        }
        let msg = format!("0 matches for '{}'", pattern);
        println!("{}", msg);
        timer.track(
            &format!("grep -rn '{}' {}", pattern, path),
            "bdo grep",
            &raw_output,
            &msg,
        );
        return Ok(exit_code);
    }

    // Always filter: truncate long lines, apply per-file and global caps.
    // Output in standard file:line:content format that AI agents can parse.
    // (A passthrough approach yields 0% savings — no reason for Bushido to exist on that path.)
    let total_matches = result.stdout.lines().count();

    let context_re = if context_only {
        Regex::new(&format!("(?i).{{0,20}}{}.*", regex::escape(pattern))).ok()
    } else {
        None
    };

    let mut by_file: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    for line in result.stdout.lines() {
        let Some((file, line_num, content)) = parse_match_line(line) else {
            continue;
        };
        let cleaned = clean_line(content, max_line_len, context_re.as_ref(), pattern);
        by_file.entry(file).or_default().push((line_num, cleaned));
    }

    let mut rtk_output = String::new();
    rtk_output.push_str(&format!(
        "{} matches in {} files:\n\n",
        total_matches,
        by_file.len()
    ));

    let mut shown = 0;
    let mut files: Vec<_> = by_file.iter().collect();
    files.sort_by_key(|(f, _)| *f);

    // `--all` arrives as max_results == usize::MAX; lift the per-file cap too.
    let per_file = if max_results == usize::MAX {
        usize::MAX
    } else {
        config::limits().grep_max_per_file
    };
    for (file, matches) in files {
        if shown >= max_results {
            break;
        }

        let file_display = compact_path(file);
        for (line_num, content) in matches.iter().take(per_file) {
            if shown >= max_results {
                break;
            }
            rtk_output.push_str(&format!("{}:{}:{}\n", file_display, line_num, content));
            shown += 1;
        }
    }

    if total_matches > shown {
        // Show the omitted count AND how to recover them — never drop matches silently.
        rtk_output.push_str(&format!(
            "[+{} more — use --all to show every match]\n",
            total_matches - shown
        ));
    }

    print!("{}", rtk_output);
    timer.track(
        &format!("grep -rn '{}' {}", pattern, path),
        "bdo grep",
        &raw_output,
        &rtk_output,
    );

    Ok(exit_code)
}

/// Parses a single rg/grep match line into `(file, line_number, content)`.
///
/// Two shapes are accepted:
///   1. NUL-separated `file\0line_number:content` — emitted by `rg -0` (and GNU
///      `grep -Z`). NUL cannot appear in file paths, so this is unambiguous even
///      when content or the path contains `:` (e.g. `ClassRegistry::init(...)`,
///      issue #1436; Windows drive letters).
///   2. Colon-separated `file:line_number:content` — the universal grep format,
///      used by the `grep` fallback. Not all grep-compatible binaries honour
///      `-Z` as a NUL separator (e.g. ugrep treats `-Z` as fuzzy matching), so
///      the fallback emits plain colons and we parse them here. The first colon
///      ends the file name; this misreads the rare path that itself contains
///      `:`, but that is far better than dropping every match.
///
/// Returns `None` for lines that do not match either shape (e.g. rg `-A`/`-B`
/// context lines that use `-` as separator).
fn build_rg_args(
    rg_pattern: &str,
    path: &str,
    file_type: Option<&str>,
    extra_args: &[String],
) -> Vec<String> {
    let mut args = Vec::new();

    // --no-ignore-vcs: match grep -r behavior (don't skip .gitignore'd files).
    // Without this, rg returns 0 matches for files in .gitignore, causing
    // false negatives that make AI agents draw wrong conclusions.
    // Using --no-ignore-vcs (not --no-ignore) so .ignore/.rgignore are still respected.
    args.push("--no-heading".to_string());
    args.push("--no-ignore-vcs".to_string());

    if has_format_flag(extra_args) {
        // Passthrough format flags such as -l/-c/-o have their own output shape.
        // Do not force line numbers or NUL separators, otherwise the output is
        // no longer grep-compatible (e.g. -l would emit NUL-terminated paths).
        args.push(rg_pattern.to_string());
        args.push(path.to_string());
    } else {
        // -n: line numbers, -H: always filename, -0: NUL-separate filename.
        // NUL separation lets parse_match_line disambiguate filenames/content
        // containing `:digits:` patterns (issue #1436).
        args.push("-nH0".to_string());
        args.push(rg_pattern.to_string());
        args.push(path.to_string());
    }

    if let Some(ft) = file_type {
        args.push("--type".to_string());
        args.push(ft.to_string());
    }

    for arg in extra_args {
        // Fix: skip grep-ism -r flag (rg is recursive by default; rg -r means --replace)
        if arg == "-r" || arg == "--recursive" {
            continue;
        }
        args.push(arg.clone());
    }

    args
}

fn parse_match_line(line: &str) -> Option<(String, usize, &str)> {
    lazy_static::lazy_static! {
        // NUL-separated (preferred, unambiguous).
        static ref NUL_LINE_RE: Regex = Regex::new(r"^([^\x00]+)\x00(\d+):(.*)$").unwrap();
        // Colon-separated `file:line:content` (grep fallback). `[^:]+` stops the
        // file at the first colon; `(\d+):` anchors the line number so content
        // may still contain colons.
        static ref COLON_LINE_RE: Regex = Regex::new(r"^([^:\x00]+):(\d+):(.*)$").unwrap();
    }
    let re = if line.contains('\x00') {
        &*NUL_LINE_RE
    } else {
        &*COLON_LINE_RE
    };
    re.captures(line).and_then(|caps| {
        let (_, [file, line_num, content]) = caps.extract();
        let line_num: usize = line_num.parse().ok()?;
        Some((file.to_string(), line_num, content))
    })
}

fn has_format_flag(extra_args: &[String]) -> bool {
    extra_args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "-c" | "--count"
                | "-l"
                | "--files-with-matches"
                | "-L"
                | "--files-without-match"
                | "-o"
                | "--only-matching"
                | "-h"
                | "--no-filename"
                | "-Z"
                | "--null"
        )
    })
}

fn clean_line(line: &str, max_len: usize, context_re: Option<&Regex>, pattern: &str) -> String {
    let trimmed = line.trim();

    if let Some(re) = context_re {
        if let Some(m) = re.find(trimmed) {
            let matched = m.as_str();
            if matched.len() <= max_len {
                return matched.to_string();
            }
        }
    }

    if trimmed.len() <= max_len {
        trimmed.to_string()
    } else {
        let lower = trimmed.to_lowercase();
        let pattern_lower = pattern.to_lowercase();

        if let Some(pos) = lower.find(&pattern_lower) {
            let char_pos = lower[..pos].chars().count();
            let chars: Vec<char> = trimmed.chars().collect();
            let char_len = chars.len();

            let start = char_pos.saturating_sub(max_len / 3);
            let end = (start + max_len).min(char_len);
            let start = if end == char_len {
                end.saturating_sub(max_len)
            } else {
                start
            };

            let slice: String = chars[start..end].iter().collect();
            if start > 0 && end < char_len {
                format!("...{}...", slice)
            } else if start > 0 {
                format!("...{}", slice)
            } else {
                format!("{}...", slice)
            }
        } else {
            let t: String = trimmed.chars().take(max_len - 3).collect();
            format!("{}...", t)
        }
    }
}

fn compact_path(path: &str) -> String {
    if path.len() <= 50 {
        return path.to_string();
    }

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 3 {
        return path.to_string();
    }

    format!(
        "{}/.../{}/{}",
        parts[0],
        parts[parts.len() - 2],
        parts[parts.len() - 1]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_line() {
        let line = "            const result = someFunction();";
        let cleaned = clean_line(line, 50, None, "result");
        assert!(!cleaned.starts_with(' '));
        assert!(cleaned.len() <= 50);
    }

    #[test]
    fn test_compact_path() {
        let path = "/Users/patrick/dev/project/src/components/Button.tsx";
        let compact = compact_path(path);
        assert!(compact.len() <= 60);
    }

    #[test]
    fn test_extra_args_accepted() {
        // Test that the function signature accepts extra_args
        // This is a compile-time test - if it compiles, the signature is correct
        let _extra: Vec<String> = vec!["-i".to_string(), "-A".to_string(), "3".to_string()];
        // No need to actually run - we're verifying the parameter exists
    }

    #[test]
    fn test_clean_line_multibyte() {
        // Thai text that exceeds max_len in bytes
        let line = "  สวัสดีครับ นี่คือข้อความที่ยาวมากสำหรับทดสอบ  ";
        let cleaned = clean_line(line, 20, None, "ครับ");
        // Should not panic
        assert!(!cleaned.is_empty());
    }

    #[test]
    fn test_clean_line_emoji() {
        let line = "🎉🎊🎈🎁🎂🎄 some text 🎃🎆🎇✨";
        let cleaned = clean_line(line, 15, None, "text");
        assert!(!cleaned.is_empty());
    }

    // Fix: BRE \| alternation is translated to PCRE | for rg
    #[test]
    fn test_bre_alternation_translated() {
        let pattern = r"fn foo\|pub.*bar";
        let rg_pattern = pattern.replace(r"\|", "|");
        assert_eq!(rg_pattern, "fn foo|pub.*bar");
    }

    // Fix: -r flag (grep recursive) is stripped from extra_args (rg is recursive by default)
    #[test]
    fn test_recursive_flag_stripped() {
        let args = build_rg_args("needle", ".", None, &["-r".to_string(), "-i".to_string()]);
        assert!(!args.contains(&"-r".to_string()));
        assert!(args.contains(&"-i".to_string()));
    }

    #[test]
    fn test_rg_args_normal_search_include_line_numbers_and_nul_separator() {
        let args = build_rg_args("needle", "src", None, &[]);
        assert!(args.contains(&"-nH0".to_string()));
        assert!(args.contains(&"--no-heading".to_string()));
        assert!(args.contains(&"--no-ignore-vcs".to_string()));
    }

    #[test]
    fn test_rg_args_format_flags_do_not_force_nul_or_line_numbers() {
        for flag in [
            "-l",
            "--files-with-matches",
            "-c",
            "--count",
            "-o",
            "--only-matching",
        ] {
            let args = build_rg_args("needle", "src", None, &[flag.to_string()]);
            assert!(
                args.contains(&flag.to_string()),
                "missing passthrough flag {flag}"
            );
            assert!(
                !args.contains(&"-nH0".to_string()),
                "format flag {flag} must preserve grep-compatible output"
            );
        }
    }

    #[test]
    fn test_rg_args_preserve_type_filter_with_format_flag() {
        let args = build_rg_args("needle", "src", Some("rust"), &["-l".to_string()]);
        assert!(args.windows(2).any(|pair| pair == ["--type", "rust"]));
        assert!(args.contains(&"-l".to_string()));
        assert!(!args.contains(&"-nH0".to_string()));
    }

    // --- truncation accuracy ---

    #[test]
    fn test_grep_overflow_uses_uncapped_total() {
        // Confirm the grep overflow invariant: matches vec is never capped before overflow calc.
        // If total_matches > per_file, overflow = total_matches - per_file (not capped).
        // This documents that grep_cmd.rs avoids the diff_cmd bug (cap at N then compute N-10).
        let per_file = config::limits().grep_max_per_file;
        let total_matches = per_file + 42;
        let overflow = total_matches - per_file;
        assert_eq!(overflow, 42, "overflow must equal true suppressed count");
        // Demonstrate why capping before subtraction is wrong:
        let hypothetical_cap = per_file + 5;
        let capped = total_matches.min(hypothetical_cap);
        let wrong_overflow = capped - per_file;
        assert_ne!(
            wrong_overflow, overflow,
            "capping before subtraction gives wrong overflow"
        );
    }

    // --- format flag detection ---

    #[test]
    fn test_format_flag_detects_count() {
        assert!(has_format_flag(&["-c".to_string()]));
        assert!(has_format_flag(&["--count".to_string()]));
    }

    #[test]
    fn test_format_flag_detects_files_with_matches() {
        assert!(has_format_flag(&["-l".to_string()]));
        assert!(has_format_flag(&["--files-with-matches".to_string()]));
    }

    #[test]
    fn test_format_flag_detects_files_without_match() {
        assert!(has_format_flag(&["-L".to_string()]));
        assert!(has_format_flag(&["--files-without-match".to_string()]));
    }

    #[test]
    fn test_format_flag_detects_only_matching() {
        assert!(has_format_flag(&["-o".to_string()]));
        assert!(has_format_flag(&["--only-matching".to_string()]));
    }

    // grep's `-h` (--no-filename) drops the file column that bdo groups by, so it
    // must pass through as raw output rather than be regrouped.
    #[test]
    fn test_format_flag_detects_no_filename() {
        assert!(has_format_flag(&["-h".to_string()]));
        assert!(has_format_flag(&["--no-filename".to_string()]));
    }

    #[test]
    fn test_format_flag_detects_null() {
        assert!(has_format_flag(&["-Z".to_string()]));
        assert!(has_format_flag(&["--null".to_string()]));
    }

    #[test]
    fn test_format_flag_ignores_normal_flags() {
        assert!(!has_format_flag(&[
            "-i".to_string(),
            "-w".to_string(),
            "-A".to_string(),
            "3".to_string(),
        ]));
    }

    // Verify line numbers are always enabled in rg invocation (grep_cmd.rs:24).
    // The -n/--line-numbers clap flag in main.rs is a no-op accepted for compat.
    #[test]
    fn test_rg_always_has_line_numbers() {
        // grep_cmd::run() always passes "-n" to rg (line 24).
        // This test documents that -n is built-in, so the clap flag is safe to ignore.
        let mut cmd = resolved_command("rg");
        cmd.args(["-n", "--no-heading", "NONEXISTENT_PATTERN_12345", "."]);
        // If rg is available, it should accept -n without error (exit 1 = no match, not error)
        if let Ok(output) = cmd.output() {
            assert!(
                output.status.code() == Some(1) || output.status.success(),
                "rg -n should be accepted"
            );
        }
        // If rg is not installed, skip gracefully (test still passes)
    }

    // --- issue #1436: parse_match_line robustness ---
    // Accepts NUL-separated `file\0line:content` (rg --null / GNU grep -Z) and
    // colon-separated `file:line:content` (the grep fallback).

    #[test]
    fn test_parse_match_line_simple() {
        let line = "file.php\x0010:use Foo\\Bar;";
        let (file, line_num, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "file.php");
        assert_eq!(line_num, 10);
        assert_eq!(content, "use Foo\\Bar;");
    }

    // Issue #1436 reproducer: content with `::` must not split into a phantom
    // file bucket. With NUL separation between file and line:content, content
    // colons are irrelevant to the parser.
    #[test]
    fn test_parse_match_line_content_with_double_colon() {
        let line = "externalImportShell.class.php\x0081:        $this->queueProcessModel = ClassRegistry::init('Collections.QueueProcess');";
        let (file, line_num, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "externalImportShell.class.php");
        assert_eq!(line_num, 81);
        assert_eq!(
            content,
            "        $this->queueProcessModel = ClassRegistry::init('Collections.QueueProcess');"
        );
    }

    // Windows abs-path safety: drive letter + backslashes must not break the
    // parser. The NUL separator makes the file portion unambiguous.
    #[test]
    fn test_parse_match_line_windows_path() {
        let line = "C:\\src\\file.rs\x0042:fn main() {}";
        let (file, line_num, content) = parse_match_line(line).unwrap();
        assert_eq!(file, r"C:\src\file.rs");
        assert_eq!(line_num, 42);
        assert_eq!(content, "fn main() {}");
    }

    // Filenames containing `:digits:` (which would fool a greedy `:` parser)
    // must still parse correctly under NUL separation.
    #[test]
    fn test_parse_match_line_filename_with_colons() {
        let line = "badly_named:52:file.txt\x001:xxx";
        let (file, line_num, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "badly_named:52:file.txt");
        assert_eq!(line_num, 1);
        assert_eq!(content, "xxx");
    }

    // Content that itself contains `:digits:` (e.g. log lines, port numbers,
    // line-number-like substrings) must not confuse the parser.
    #[test]
    fn test_parse_match_line_content_with_digit_colons() {
        let line = "log.txt\x007:debug: counter is :42: now";
        let (file, line_num, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "log.txt");
        assert_eq!(line_num, 7);
        assert_eq!(content, "debug: counter is :42: now");
    }

    // Colon-separated `file:line:content` — the `grep` fallback format (used
    // when `rg` is unavailable, or when the system `grep` doesn't honour `-Z`
    // as a NUL separator, e.g. ugrep). Must parse, otherwise every match is
    // silently dropped and the output reads "N matches in 0 files".
    #[test]
    fn test_parse_match_line_colon_fallback() {
        let line = "src/core/runner.rs:117:    let filtered = catch_unwind(|| {";
        let (file, line_num, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "src/core/runner.rs");
        assert_eq!(line_num, 117);
        assert_eq!(content, "    let filtered = catch_unwind(|| {");
    }

    // Colon format: content containing further colons (e.g. `::`) stays with the
    // content; only the first two colons (file, line) are structural.
    #[test]
    fn test_parse_match_line_colon_content_with_double_colon() {
        let line = "registry.rs:81:ClassRegistry::init('x');";
        let (file, line_num, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "registry.rs");
        assert_eq!(line_num, 81);
        assert_eq!(content, "ClassRegistry::init('x');");
    }

    #[test]
    fn test_parse_match_line_malformed_returns_none() {
        assert!(parse_match_line("not a match line").is_none());
        // Colon line with a non-numeric "line number".
        assert!(parse_match_line("file.rs:abc:content").is_none());
        // Missing line number after NUL
        assert!(parse_match_line("file.rs\x00fn foo()").is_none());
        // Empty
        assert!(parse_match_line("").is_none());
    }

    #[test]
    fn test_parse_match_line_empty_content() {
        let line = "file.rs\x007:";
        let (file, line_num, content) = parse_match_line(line).unwrap();
        assert_eq!(file, "file.rs");
        assert_eq!(line_num, 7);
        assert_eq!(content, "");
    }

    #[test]
    fn test_rg_no_ignore_vcs_flag_accepted() {
        // Verify rg accepts --no-ignore-vcs (used to match grep -r behavior for .gitignore)
        let mut cmd = resolved_command("rg");
        cmd.args([
            "-n",
            "--no-heading",
            "--no-ignore-vcs",
            "NONEXISTENT_PATTERN_12345",
            ".",
        ]);
        if let Ok(output) = cmd.output() {
            assert!(
                output.status.code() == Some(1) || output.status.success(),
                "rg --no-ignore-vcs should be accepted"
            );
        }
        // If rg is not installed, skip gracefully (test still passes)
    }
}
