//! Runs a command and produces a heuristic summary of its output.

use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::truncate::CAP_WARNINGS;
use crate::core::utils::truncate;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::process::Command;

const MAX_SUMMARY_LIST: usize = CAP_WARNINGS;
const MAX_SUMMARY_KEYS: usize = CAP_WARNINGS;
/// Lines of evidence to quote from a failure.
const MAX_QUOTED_LINES: usize = 5;
/// A short output is shown whole; a longer one as its head and tail.
const GENERIC_WHOLE_LINES: usize = 10;

lazy_static! {
    // `tests`, `run-tests.sh` and `pytest`, but not `latest` or `contest`.
    static ref TEST_COMMAND_RE: Regex =
        Regex::new(r"(^|[^a-z])(tests?|pytest|vitest|jest|rspec|mocha)($|[^a-z])").unwrap();
    static ref PASSED_COUNT_RE: Regex = Regex::new(r"(\d+)\s*passed").unwrap();
    static ref FAILED_COUNT_RE: Regex = Regex::new(r"(\d+)\s*failed").unwrap();
    static ref SKIPPED_COUNT_RE: Regex = Regex::new(r"(\d+)\s*(?:skipped|ignored)").unwrap();
    static ref PASS_WORD_RE: Regex = Regex::new(r"\b(?:ok|passed)\b|✓").unwrap();
    static ref FAIL_WORD_RE: Regex = Regex::new(r"\b(?:fail|failed|failure|failures)\b|\[x\]|✗").unwrap();
    static ref NO_ERRORS_RE: Regex = Regex::new(r"\b0 errors?\b").unwrap();
    static ref NO_WARNINGS_RE: Regex = Regex::new(r"\b0 warnings?\b").unwrap();
}

/// Run a command and provide a heuristic summary
pub fn run(command: &str, verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Running and summarizing: {}", command);
    }

    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    };
    let result = exec_capture(&mut cmd).context("Failed to execute command")?;

    // Only a separator where one is missing, or the line count would be off.
    let mut raw = result.stdout.clone();
    if !raw.is_empty() && !raw.ends_with('\n') && !result.stderr.is_empty() {
        raw.push('\n');
    }
    raw.push_str(&result.stderr);

    let summary = summarize_output(&raw, command, result.success());
    println!("{}", summary);
    timer.track(command, "bdo summary", &raw, &summary);
    Ok(result.exit_code)
}

fn summarize_output(output: &str, command: &str, success: bool) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let mut result = Vec::new();

    // Status
    let status_icon = if success { "[ok]" } else { "[FAIL]" };
    result.push(format!(
        "{} Command: {}",
        status_icon,
        truncate(command, 60)
    ));
    result.push(format!("   {} lines of output", lines.len()));
    result.push(String::new());

    // Detect type of output and summarize accordingly
    if output.trim().is_empty() {
        result.push("(no output)".to_string());
        return result.join("\n");
    }

    let output_type = detect_output_type(output, command);

    match output_type {
        OutputType::TestResults => summarize_tests(output, &mut result),
        OutputType::BuildOutput => summarize_build(output, success, &mut result),
        OutputType::LogOutput => summarize_logs_quick(output, &mut result),
        OutputType::ListOutput => summarize_list(output, &mut result),
        OutputType::JsonOutput => summarize_json(output, &mut result),
        OutputType::Generic => summarize_generic(output, &mut result),
    }

    result.join("\n")
}

#[derive(Debug)]
enum OutputType {
    TestResults,
    BuildOutput,
    LogOutput,
    ListOutput,
    JsonOutput,
    Generic,
}

fn detect_output_type(output: &str, command: &str) -> OutputType {
    let cmd_lower = command.to_lowercase();
    let out_lower = output.to_lowercase();

    if TEST_COMMAND_RE.is_match(&cmd_lower) || out_lower.contains("passed") && out_lower.contains("failed") {
        OutputType::TestResults
    } else if cmd_lower.contains("build")
        || cmd_lower.contains("compile")
        || out_lower.contains("compiling")
    {
        OutputType::BuildOutput
    } else if out_lower.contains("error:")
        || out_lower.contains("warn:")
        || out_lower.contains("[info]")
    {
        OutputType::LogOutput
    } else if output.trim_start().starts_with('{') || output.trim_start().starts_with('[') {
        OutputType::JsonOutput
    } else if output.lines().all(|l| {
        l.len() < 200
            && if l.contains('\t') {
                false
            } else {
                l.split_whitespace().count() < 10
            }
    }) {
        OutputType::ListOutput
    } else {
        OutputType::Generic
    }
}

/// Quote the first few lines, and say how many more there were.
fn quote_lines(title: &str, lines: &[&str], result: &mut Vec<String>) {
    if lines.is_empty() {
        return;
    }
    result.push(String::new());
    result.push(format!("   {}:", title));
    for line in lines.iter().take(MAX_QUOTED_LINES) {
        result.push(format!("   • {}", truncate(line, 70)));
    }
    if lines.len() > MAX_QUOTED_LINES {
        result.push(format!("   ... +{} more", lines.len() - MAX_QUOTED_LINES));
    }
}

fn captured_count(re: &Regex, line: &str) -> Option<usize> {
    re.captures(line)?.get(1)?.as_str().parse().ok()
}

fn summarize_tests(output: &str, result: &mut Vec<String>) {
    // A totals line is the authority, and one run can print many: `cargo test`
    // prints one per test binary, so they add up rather than replace each other.
    let (mut passed, mut failed, mut skipped) = (None::<usize>, None::<usize>, 0);
    let mut pass_lines = 0;
    let mut failures = Vec::new();

    for line in output.lines() {
        let lower = line.to_lowercase();
        if let Some(n) = captured_count(&PASSED_COUNT_RE, &lower) {
            passed = Some(passed.unwrap_or(0) + n);
        } else if PASS_WORD_RE.is_match(&lower) {
            pass_lines += 1;
        }
        let failed_here = captured_count(&FAILED_COUNT_RE, &lower);
        if let Some(n) = failed_here {
            failed = Some(failed.unwrap_or(0) + n);
        }
        if failed_here != Some(0) && (failed_here.is_some() || FAIL_WORD_RE.is_match(&lower)) {
            failures.push(line);
        }
        skipped += captured_count(&SKIPPED_COUNT_RE, &lower).unwrap_or(0);
    }

    let passed = passed.unwrap_or(pass_lines);
    let failed = failed.unwrap_or(0);

    // No results at all, such as tests that did not compile: show the output,
    // since that is where the reason is.
    if passed == 0 && failed == 0 && failures.is_empty() {
        summarize_generic(output, result);
        return;
    }

    result.push("Test Results:".to_string());
    if passed > 0 {
        result.push(format!("   [ok] {} passed", passed));
    }
    if failed > 0 {
        result.push(format!("   [FAIL] {} failed", failed));
    }
    if skipped > 0 {
        result.push(format!("   skip {} skipped", skipped));
    }
    quote_lines("Failures", &failures, result);
}

fn summarize_build(output: &str, success: bool, result: &mut Vec<String>) {
    let mut warnings = 0;
    let mut compiled = 0;
    let mut error_lines = Vec::new();

    for line in output.lines() {
        let lower = line.to_lowercase();
        if lower.contains("error") && !NO_ERRORS_RE.is_match(&lower) {
            error_lines.push(line);
        }
        if lower.contains("warning") && !NO_WARNINGS_RE.is_match(&lower) {
            warnings += 1;
        }
        if lower.contains("compiling") || lower.contains("compiled") {
            compiled += 1;
        }
    }

    // It failed and nothing here says why: the reason is in the output itself.
    if !success && error_lines.is_empty() {
        summarize_generic(output, result);
        return;
    }

    result.push("Build Summary:".to_string());
    if compiled > 0 {
        result.push(format!("   {} crates/files compiled", compiled));
    }
    if !error_lines.is_empty() {
        result.push(format!("   [error] {} errors", error_lines.len()));
    }
    if warnings > 0 {
        result.push(format!("   [warn] {} warnings", warnings));
    }
    if error_lines.is_empty() && warnings == 0 {
        result.push("   [ok] Build successful".to_string());
    }
    quote_lines("Errors", &error_lines, result);
}

fn summarize_logs_quick(output: &str, result: &mut Vec<String>) {
    result.push("Log Summary:".to_string());

    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut info = 0;

    for line in output.lines() {
        let lower = line.to_lowercase();
        if lower.contains("error") || lower.contains("fatal") {
            errors.push(line);
        } else if lower.contains("warn") {
            warnings.push(line);
        } else if lower.contains("info") {
            info += 1;
        }
    }

    result.push(format!("   [error] {} errors", errors.len()));
    result.push(format!("   [warn] {} warnings", warnings.len()));
    result.push(format!("   [info] {} info", info));
    // A count says something went wrong; only the line says what.
    quote_lines("Errors", &errors, result);
    quote_lines("Warnings", &warnings, result);
}

fn summarize_list(output: &str, result: &mut Vec<String>) {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    result.push(format!("List ({} items):", lines.len()));

    for line in lines.iter().take(MAX_SUMMARY_LIST) {
        result.push(format!("   • {}", truncate(line, 70)));
    }
    if lines.len() > MAX_SUMMARY_LIST {
        result.push(format!("   ... +{} more", lines.len() - MAX_SUMMARY_LIST));
    }
}

fn summarize_json(output: &str, result: &mut Vec<String>) {
    result.push("JSON Output:".to_string());

    // Try to parse and show structure. Only the first value counts, because
    // whatever the command wrote to stderr comes after it.
    let first_value = serde_json::Deserializer::from_str(output)
        .into_iter::<serde_json::Value>()
        .next();
    if let Some(Ok(value)) = first_value {
        match &value {
            serde_json::Value::Array(arr) => {
                result.push(format!("   Array with {} items", arr.len()));
            }
            serde_json::Value::Object(obj) => {
                result.push(format!("   Object with {} keys:", obj.len()));
                for key in obj.keys().take(MAX_SUMMARY_KEYS) {
                    result.push(format!("   • {}", key));
                }
                if obj.len() > MAX_SUMMARY_KEYS {
                    result.push(format!("   ... +{} more keys", obj.len() - MAX_SUMMARY_KEYS));
                }
            }
            _ => {
                result.push(format!("   {}", truncate(&value.to_string(), 100)));
            }
        }
    } else {
        result.push("   (Invalid JSON)".to_string());
    }
}

fn summarize_generic(output: &str, result: &mut Vec<String>) {
    let lines: Vec<&str> = output.lines().collect();

    result.push("Output:".to_string());

    let mut show = |shown: &[&str]| {
        for line in shown.iter().filter(|l| !l.trim().is_empty()) {
            result.push(format!("   {}", truncate(line, 75)));
        }
    };
    if lines.len() <= GENERIC_WHOLE_LINES {
        show(&lines);
        return;
    }
    show(&lines[..5]);
    show(&[&format!("... +{} lines", lines.len() - 8)]);
    show(&lines[lines.len() - 3..]);
}
