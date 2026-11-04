//! Shared command execution skeleton for filter modules.

use anyhow::Result;
use std::process::Command;

use crate::core::stream::{self, FilterMode, StdinMode, StreamFilter};
use crate::core::tracking;

pub fn print_with_hint(filtered: &str, raw: &str, tee_label: &str, exit_code: i32) {
    if let Some(hint) = crate::core::tee::tee_and_hint(raw, tee_label, exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }
}

/// Lines of a failed command's output to show when its filter kept none.
const MAX_FAILURE_LINES: usize = 10;

/// What must be added to a failed command's filtered output, if anything.
///
/// A filter recognises the output it expects. A command can fail in a way it does
/// not expect (a bad flag, a broken config, a crash), and the filter then reports
/// what it found, which is nothing: "No issues found", with the error gone. So if
/// a failed command's message appears nowhere in what is shown, this returns the
/// end of it. The message is stderr if the command wrote any. A filter that got
/// structured output has read it, and reports it in its own words.
pub fn failure_evidence(stdout: &str, stderr: &str, shown: &str, exit_code: i32) -> Option<String> {
    if exit_code == 0 {
        return None;
    }
    let significant = |text: &str| -> Vec<String> {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(crate::core::utils::strip_ansi)
            .collect()
    };
    let (out_lines, err_lines) = (significant(stdout), significant(stderr));
    if out_lines.is_empty() && err_lines.is_empty() {
        return Some(format!(
            "[bdo] the command failed (exit {exit_code}) and printed nothing"
        ));
    }
    if out_lines
        .iter()
        .any(|l| l.starts_with('{') || l.starts_with('['))
    {
        return None;
    }
    // Compared by their start, since filters shorten long lines.
    let is_shown = |line: &String| {
        let end = line.char_indices().nth(40).map_or(line.len(), |(i, _)| i);
        shown.contains(&line[..end])
    };
    if err_lines.iter().any(is_shown) {
        return None;
    }
    let lines = if out_lines.iter().any(is_shown) {
        // The filter read stdout, so stderr only matters where it reports an
        // error. The rest is progress, such as cargo's `Compiling`.
        err_lines
            .into_iter()
            .filter(|line| {
                let lower = line.to_lowercase();
                ["error", "fatal", "failed", "exception", "panic"]
                    .iter()
                    .any(|word| lower.contains(word))
            })
            .collect()
    } else if err_lines.is_empty() {
        out_lines
    } else {
        err_lines
    };
    if lines.is_empty() {
        return None;
    }
    let tail = &lines[lines.len().saturating_sub(MAX_FAILURE_LINES)..];
    Some(format!(
        "[bdo] the command failed (exit {exit_code}). Its output ended with:\n{}",
        tail.join("\n")
    ))
}

/// `filtered`, followed by [`failure_evidence`] if there is any.
pub fn with_failure_evidence(
    filtered: String,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
) -> String {
    match failure_evidence(stdout, stderr, &filtered, exit_code) {
        Some(evidence) if filtered.trim().is_empty() => evidence,
        Some(evidence) => format!("{}\n{evidence}", filtered.trim_end()),
        None => filtered,
    }
}

#[derive(Default)]
pub struct RunOptions<'a> {
    pub tee_label: Option<&'a str>,
    pub filter_stdout_only: bool,
    pub skip_filter_on_failure: bool,
    pub no_trailing_newline: bool,
    /// Forward rtk's own stdin to the child process. Needed for commands that
    /// can read from a pipe (e.g. `cat file | rtk wc`); without it the child
    /// gets an empty stdin and reports zero.
    pub inherit_stdin: bool,
}

impl<'a> RunOptions<'a> {
    pub fn with_tee(label: &'a str) -> Self {
        Self {
            tee_label: Some(label),
            ..Default::default()
        }
    }

    pub fn stdout_only() -> Self {
        Self {
            filter_stdout_only: true,
            ..Default::default()
        }
    }

    pub fn tee(mut self, label: &'a str) -> Self {
        self.tee_label = Some(label);
        self
    }

    pub fn early_exit_on_failure(mut self) -> Self {
        self.skip_filter_on_failure = true;
        self
    }

    pub fn no_trailing_newline(mut self) -> Self {
        self.no_trailing_newline = true;
        self
    }

    pub fn inherit_stdin(mut self) -> Self {
        self.inherit_stdin = true;
        self
    }
}

pub type CaptureFilter<'a> = Box<dyn Fn(&str) -> String + 'a>;
pub type ExitAwareCaptureFilter<'a> = Box<dyn Fn(&str, i32) -> String + 'a>;

pub enum RunMode<'a> {
    Filtered(CaptureFilter<'a>),
    FilteredWithExit(ExitAwareCaptureFilter<'a>),
    Streamed(Box<dyn StreamFilter + 'a>),
    Passthrough,
}

fn run_captured_filter<F>(
    mut cmd: Command,
    cmd_label: &str,
    filter_fn: F,
    opts: RunOptions<'_>,
    timer: tracking::TimedExecution,
) -> Result<i32>
where
    F: Fn(&str, i32) -> String,
{
    let stdin_mode = if opts.inherit_stdin {
        StdinMode::Inherit
    } else {
        StdinMode::Null
    };
    // run_streaming already reports a clear, tool-named error (e.g. "'git' was
    // not found in PATH"), so no extra "Failed to run X" wrapper is needed.
    let result = stream::run_streaming(&mut cmd, stdin_mode, FilterMode::CaptureOnly)?;

    let exit_code = result.exit_code;
    let raw = &result.raw;
    let raw_stdout = &result.raw_stdout;

    if opts.skip_filter_on_failure && exit_code != 0 {
        if !result.raw_stdout.trim().is_empty() {
            print!("{}", result.raw_stdout);
        }
        if !result.raw_stderr.trim().is_empty() {
            eprint!("{}", result.raw_stderr);
        }
        timer.track(cmd_label, &format!("bdo {}", cmd_label), raw, raw);
        return Ok(exit_code);
    }

    let text_to_filter = if opts.filter_stdout_only {
        raw_stdout
    } else {
        raw
    };
    // Fail-safe (ARCHITECTURE.md principle #4): a filter that panics on
    // adversarial command output must not take down the process and swallow the
    // user's output. Catch the unwind and fall back to the raw text.
    let filtered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        filter_fn(text_to_filter, exit_code)
    }))
    .unwrap_or_else(|_| {
        eprintln!(
            "[bdo] warning: filter for '{}' panicked — passing through raw output",
            cmd_label
        );
        text_to_filter.to_string()
    });
    let filtered = match failure_evidence(raw_stdout, &result.raw_stderr, &filtered, exit_code) {
        Some(_) => {
            with_failure_evidence(filtered, raw_stdout, &result.raw_stderr, exit_code) + "\n"
        }
        None => filtered,
    };

    if let Some(label) = opts.tee_label {
        print_with_hint(&filtered, raw, label, exit_code);
    } else if opts.no_trailing_newline {
        print!("{}", filtered);
    } else {
        println!("{}", filtered);
    }

    let raw_for_tracking = if opts.filter_stdout_only {
        raw_stdout
    } else {
        raw
    };
    timer.track(
        cmd_label,
        &format!("bdo {}", cmd_label),
        raw_for_tracking,
        &filtered,
    );
    Ok(exit_code)
}

pub fn run(
    mut cmd: Command,
    tool_name: &str,
    args_display: &str,
    mode: RunMode<'_>,
    opts: RunOptions<'_>,
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let cmd_label = format!("{} {}", tool_name, args_display);

    // Raw mode (`--raw` / `BDO_RAW=1`): commands with a same-named program are
    // exec'd directly before clap ever runs (see core::raw). What reaches here
    // is a bdo command whose tool has a different name — never reduce it.
    let mode = if crate::core::raw::is_active() {
        RunMode::Passthrough
    } else {
        mode
    };

    match mode {
        RunMode::Filtered(filter_fn) => {
            run_captured_filter(cmd, &cmd_label, move |text, _| filter_fn(text), opts, timer)
        }
        RunMode::FilteredWithExit(filter_fn) => run_captured_filter(
            cmd,
            &cmd_label,
            move |text, exit_code| filter_fn(text, exit_code),
            opts,
            timer,
        ),
        RunMode::Streamed(filter) => {
            let result =
                stream::run_streaming(&mut cmd, StdinMode::Null, FilterMode::Streaming(filter))?;

            if let Some(evidence) = failure_evidence(
                &result.raw_stdout,
                &result.raw_stderr,
                &result.filtered,
                result.exit_code,
            ) {
                println!("{evidence}");
            }

            if let Some(label) = opts.tee_label {
                if let Some(hint) =
                    crate::core::tee::tee_and_hint(&result.raw, label, result.exit_code)
                {
                    println!("{}", hint);
                }
            }

            timer.track(
                &cmd_label,
                &format!("bdo {}", cmd_label),
                &result.raw,
                &result.filtered,
            );
            Ok(result.exit_code)
        }
        RunMode::Passthrough => {
            let result =
                stream::run_streaming(&mut cmd, StdinMode::Inherit, FilterMode::Passthrough)?;

            timer.track_passthrough(&cmd_label, &format!("bdo {} (passthrough)", cmd_label));
            Ok(result.exit_code)
        }
    }
}

pub fn run_filtered<F>(
    cmd: Command,
    tool_name: &str,
    args_display: &str,
    filter_fn: F,
    opts: RunOptions<'_>,
) -> Result<i32>
where
    F: Fn(&str) -> String,
{
    run(
        cmd,
        tool_name,
        args_display,
        RunMode::Filtered(Box::new(filter_fn)),
        opts,
    )
}

pub fn run_filtered_with_exit<F>(
    cmd: Command,
    tool_name: &str,
    args_display: &str,
    filter_fn: F,
    opts: RunOptions<'_>,
) -> Result<i32>
where
    F: Fn(&str, i32) -> String,
{
    run(
        cmd,
        tool_name,
        args_display,
        RunMode::FilteredWithExit(Box::new(filter_fn)),
        opts,
    )
}

/// Raw-mode exit for modules that execute their command themselves (via
/// `exec_capture`) instead of through [`run`], and so would otherwise filter
/// regardless of `--raw` / `BDO_RAW=1`. Use as
/// `if raw::is_active() { return runner::run_raw(cmd, …); }` right before
/// capturing, with the command built *without* the output-format flags the
/// module injects for parsing (`--reporter=json` and the like) — but keeping
/// the ones that change what the command does (`run`, `--check`, a default
/// `.`), because raw means the same work with unreduced output.
pub fn run_raw(cmd: Command, tool: &str, args_display: &str) -> Result<i32> {
    run(
        cmd,
        tool,
        args_display,
        RunMode::Passthrough,
        RunOptions::default(),
    )
}

pub fn run_passthrough(tool: &str, args: &[std::ffi::OsString], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("{} passthrough: {:?}", tool, args);
    }
    let mut cmd = crate::core::utils::resolved_command(tool);
    cmd.args(args);
    let args_str = tracking::args_display(args);
    run(
        cmd,
        tool,
        &args_str,
        RunMode::Passthrough,
        RunOptions::default(),
    )
}

pub fn run_streamed(
    cmd: Command,
    tool_name: &str,
    args_display: &str,
    filter: Box<dyn StreamFilter + '_>,
    opts: RunOptions<'_>,
) -> Result<i32> {
    run(
        cmd,
        tool_name,
        args_display,
        RunMode::Streamed(filter),
        opts,
    )
}
