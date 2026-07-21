//! `bdo ci` — a single pre-merge gate that runs the change-set review, the
//! whole-tree residue audit, and the relevant tests, then returns one exit code.
//!
//! It composes three existing commands so an agent (or a CI step) can ask "is
//! this branch mergeable?" in one call instead of three:
//!   1. `bdo review`        — change summary (informational, never gates)
//!   2. `bdo stale`         — whole-tree residue audit (gates: exit 1 on residue)
//!   3. `bdo test --changed`— tests for the change set (gates: test exit code)
//!
//! Stages run cheapest-first (review, stale, then the slow tests) but every
//! stage runs regardless of earlier failures: a pre-merge gate should report
//! *all* blockers in one pass, not stop at the first. The final exit code is
//! non-zero if any gating stage failed, preserving the real test exit code when
//! tests are what failed.

use crate::cmds::rust::runner;
use crate::cmds::system::{review, stale};
use crate::core::{changes, testplan};
use anyhow::Result;

/// Run the tests for the current git change set and return the worst exit code.
///
/// Extracted from the `bdo test --changed` dispatch so both that command and
/// `bdo ci` share one implementation. `command` is the trailing test command;
/// with `--changed` it is ignored (targets come from the diff) but a non-empty
/// value is surfaced so the user knows it was dropped.
pub fn run_changed_tests(against: Option<&str>, command: &[String], verbose: u8) -> Result<i32> {
    if !command.is_empty() {
        eprintln!(
            "bdo test --changed: ignoring command args {:?} (targets are derived from the change set)",
            command
        );
    }
    if !changes::in_git_repo() {
        anyhow::bail!("bdo test --changed: not inside a git repository");
    }
    let changeset = changes::changed_files(against, None)?;
    let root = changes::repo_root().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let plan = testplan::plan_changed_tests(&changeset, &root);
    if plan.is_empty() {
        println!("bdo test --changed: no test targets in the change set");
        return Ok(0);
    }
    // Run each language's tests in turn; surface the first non-zero exit so a
    // later pass can't mask an earlier failure.
    let mut worst = 0;
    for tc in &plan {
        println!("bdo test --changed [{}]: {}", tc.lang, tc.display);
        let code = runner::run_test_argv(&tc.program, &tc.args, &tc.display, verbose)?;
        if code != 0 && worst == 0 {
            worst = code;
        }
    }
    Ok(worst)
}

pub fn run(against: Option<&str>, verbose: u8) -> Result<i32> {
    // One up-front git check so a non-repo fails cleanly instead of bailing
    // three times with three different messages.
    if !changes::in_git_repo() {
        anyhow::bail!("bdo ci: not inside a git repository");
    }

    // ── Stage 1/3: change summary (informational, does not gate) ─────────
    println!("━━━ bdo ci (1/3) — change summary ━━━");
    review::run(against, verbose)?;

    // ── Stage 2/3: whole-tree residue audit (gates) ─────────────────────
    println!("\n━━━ bdo ci (2/3) — residue audit ━━━");
    let stale_code = stale::run(None, verbose, &crate::known_command_names())?;

    // ── Stage 3/3: tests for the change set (gates) ─────────────────────
    println!("\n━━━ bdo ci (3/3) — changed tests ━━━");
    let test_code = run_changed_tests(against, &[], verbose)?;

    let code = gate_exit_code(stale_code, test_code);

    println!("\n━━━ bdo ci: {} ━━━", if code == 0 { "PASS" } else { "FAIL" });
    println!("  review  shown (informational)");
    println!("  stale   {}", stage_status(stale_code));
    println!("  tests   {}", stage_status(test_code));

    Ok(code)
}

/// Combine the two gating stages into one exit code. The gate fails if either
/// fails; a real (non-zero) test exit code wins over the bare `1` from the
/// residue gate because it carries more information (e.g. cargo's 101).
fn gate_exit_code(stale_code: i32, test_code: i32) -> i32 {
    if test_code != 0 {
        test_code
    } else {
        stale_code
    }
}

fn stage_status(code: i32) -> String {
    if code == 0 {
        "ok".to_string()
    } else {
        format!("FAIL (exit {})", code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gate_passes_only_when_both_clean() {
        assert_eq!(gate_exit_code(0, 0), 0);
    }

    #[test]
    fn test_gate_prefers_real_test_exit_code() {
        // A cargo failure (101) must not be masked by the residue gate's 1.
        assert_eq!(gate_exit_code(1, 101), 101);
        assert_eq!(gate_exit_code(0, 101), 101);
    }

    #[test]
    fn test_gate_falls_back_to_residue_when_tests_pass() {
        assert_eq!(gate_exit_code(1, 0), 1);
    }

    #[test]
    fn test_stage_status_labels() {
        assert_eq!(stage_status(0), "ok");
        assert_eq!(stage_status(101), "FAIL (exit 101)");
    }
}
