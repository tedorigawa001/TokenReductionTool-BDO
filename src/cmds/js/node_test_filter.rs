//! Compacts the output of Node's built-in test runner (`node --test`, spec reporter).

use crate::core::utils::strip_ansi;
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    /// `ℹ tests 12`: the spec reporter's summary, which marks the output as its own.
    static ref TESTS_SUMMARY: Regex = Regex::new(r"^ℹ tests \d+$").unwrap();
    static ref FAIL_COUNT: Regex = Regex::new(r"^ℹ fail (\d+)$").unwrap();
    /// A test or suite result: `  ✔ name (0.12ms)`. A `# SKIP` or `# TODO`
    /// after the duration does not match, so those lines stay.
    static ref RESULT: Regex = Regex::new(r"^(\s*)([✔✖]) (.*) \(\d+(?:\.\d+)?ms\)$").unwrap();
    /// A suite header: `▶ name`.
    static ref SUITE: Regex = Regex::new(r"^(\s*)▶ (.+)$").unwrap();
}

struct SuiteHeader<'a> {
    indent: usize,
    name: &'a str,
    index: usize,
    kept_after_header: usize,
}

/// The output with passing tests hidden, or `None` when it is not
/// `node --test` output, or when the run failed and no test failure explains it.
pub fn compact(output: &str, exit_code: i32) -> Option<String> {
    let lines: Vec<(&str, String)> = output
        .lines()
        .map(|line| (line, strip_ansi(line)))
        .collect();
    if !lines.iter().any(|(_, plain)| TESTS_SUMMARY.is_match(plain)) {
        return None;
    }
    let a_test_failed = lines.iter().any(|(_, plain)| {
        RESULT.captures(plain).is_some_and(|c| &c[2] == "✖")
            || FAIL_COUNT
                .captures(plain)
                .is_some_and(|c| c[1].parse::<u64>().map_or(true, |n| n > 0))
    });
    // Only passing lines are hidden, but if nothing says why the run failed,
    // show it whole rather than a page of results that look like a pass.
    if exit_code != 0 && !a_test_failed {
        return None;
    }

    let mut kept: Vec<Option<&str>> = Vec::with_capacity(lines.len());
    let mut kept_count = 0usize;
    let mut open_suites: Vec<SuiteHeader<'_>> = Vec::new();
    let mut hidden = 0usize;
    for (line, plain) in &lines {
        // Keep a potential heading in place until a matching result proves it
        // belongs to a suite. Ordinary logs can start with the same arrow.
        if let Some(c) = SUITE.captures(plain) {
            let (_, [indent, name]) = c.extract();
            let indent = indent.chars().count();
            while open_suites.last().is_some_and(|suite| suite.indent >= indent) {
                open_suites.pop();
            }
            open_suites.push(SuiteHeader {
                indent,
                name,
                index: kept.len(),
                kept_after_header: kept_count + 1,
            });
        } else if let Some(c) = RESULT.captures(plain) {
            let (_, [indent, mark, name]) = c.extract();
            let indent = indent.chars().count();
            while open_suites.last().is_some_and(|suite| suite.indent > indent) {
                open_suites.pop();
            }
            if open_suites.last().is_some_and(|suite| suite.indent == indent) {
                let suite = open_suites.pop().unwrap();
                // Preserve the heading if any detail is still visible inside
                // it, including a nested suite, a failure, a SKIP/TODO or a log.
                if mark == "✔" && suite.name == name && suite.kept_after_header == kept_count {
                    kept[suite.index] = None;
                    kept_count -= 1;
                }
            }
            if mark == "✔" {
                hidden += 1;
                kept.push(None);
                continue;
            }
        } else if TESTS_SUMMARY.is_match(plain) {
            open_suites.clear();
        }
        kept.push(Some(line));
        kept_count += 1;
    }

    let mut out = kept.into_iter().flatten().collect::<Vec<_>>().join("\n");
    if hidden > 0 {
        out.push_str(&format!("\n[bdo] {hidden} passing test lines hidden"));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSING: &str = "\
✔ adds (0.378792ms)
▶ group
  ✔ inner ok (0.0665ms)
  ✔ inner two (0.1ms)
✔ group (1.366792ms)
ℹ tests 3
ℹ suites 1
ℹ pass 3
ℹ fail 0
ℹ cancelled 0
ℹ skipped 0
ℹ todo 0
ℹ duration_ms 54.744167
";

    const FAILING: &str = "\
✔ adds (0.378792ms)
▶ group
  ✔ inner ok (0.0665ms)
  ✖ inner fail (0.574083ms)
✖ group (1.366792ms)
▶ other
  ✔ fine (0.1ms)
✔ other (0.2ms)
ℹ tests 4
ℹ suites 2
ℹ pass 3
ℹ fail 1
ℹ cancelled 0
ℹ skipped 0
ℹ todo 0
ℹ duration_ms 54.744167

✖ failing tests:

test at a.test.mjs:3:60
✖ inner fail (0.574083ms)
  AssertionError [ERR_ASSERTION]: 1 == 2
      at TestContext.<anonymous> (file:///x/a.test.mjs:3:92)
";

    #[test]
    fn passing_tests_are_hidden_and_the_summary_is_kept() {
        let out = compact(PASSING, 0).expect("node --test output");
        assert!(!out.contains("✔ adds"), "{out}");
        assert!(!out.contains("inner ok"), "{out}");
        assert!(!out.contains("▶ group"), "{out}");
        assert!(out.contains("ℹ tests 3"), "{out}");
        assert!(out.contains("ℹ pass 3"), "{out}");
        assert!(out.contains("ℹ fail 0"), "{out}");
        assert!(out.contains("4 passing test lines hidden"), "{out}");
    }

    #[test]
    fn failures_keep_their_suite_and_details() {
        let out = compact(FAILING, 1).expect("node --test output");
        assert!(out.contains("▶ group"), "{out}");
        assert!(out.contains("  ✖ inner fail (0.574083ms)"), "{out}");
        assert!(out.contains("✖ group (1.366792ms)"), "{out}");
        assert!(out.contains("✖ failing tests:"), "{out}");
        assert!(out.contains("test at a.test.mjs:3:60"), "{out}");
        assert!(out.contains("AssertionError [ERR_ASSERTION]: 1 == 2"), "{out}");
        assert!(out.contains("at TestContext.<anonymous>"), "{out}");
        assert!(out.contains("ℹ fail 1"), "{out}");
        assert!(!out.contains("inner ok"), "{out}");
        assert!(!out.contains("▶ other"), "{out}");
        assert!(!out.contains("fine (0.1ms)"), "{out}");
    }

    #[test]
    fn other_output_around_the_tests_is_kept() {
        let input = format!(
            "building...\n✔ Build complete\nconsole output from a test\n{FAILING}\
             node:internal/modules/cjs/loader:1228\nError: Cannot find module './x'\n"
        );
        let out = compact(&input, 1).expect("node --test output");
        assert!(out.contains("building..."), "{out}");
        // No duration, so not a test result line.
        assert!(out.contains("✔ Build complete"), "{out}");
        assert!(out.contains("console output from a test"), "{out}");
        assert!(out.contains("Error: Cannot find module './x'"), "{out}");
        assert!(!out.contains("inner ok"), "{out}");
    }

    #[test]
    fn ordinary_arrow_logs_stay_in_place() {
        let input = format!("▶ Browser fixture: mock\n{PASSING}▶ Upload complete\n");
        let out = compact(&input, 0).expect("node --test output");
        assert!(out.starts_with("▶ Browser fixture: mock\n"), "{out}");
        assert!(out.contains("ℹ duration_ms 54.744167\n▶ Upload complete"), "{out}");
        assert!(!out.contains("▶ group"), "{out}");
    }

    #[test]
    fn skipped_and_todo_tests_keep_their_suite_path() {
        let input = "\
▶ account
  ▶ checkout
    ﹣ renders empty state (0.1ms) # browser unavailable
    ✔ sends confirmation (0.2ms) # implement later
    ✔ calculates total (0.3ms)
  ✔ checkout (0.5ms)
✔ account (0.6ms)
ℹ tests 3
ℹ pass 1
ℹ fail 0
ℹ skipped 1
ℹ todo 1
";
        let out = compact(input, 0).expect("node --test output");
        assert!(
            out.starts_with(
                "▶ account\n  ▶ checkout\n    ﹣ renders empty state (0.1ms) # browser unavailable\n    ✔ sends confirmation (0.2ms) # implement later\n"
            ),
            "{out}"
        );
        assert!(!out.contains("calculates total"), "{out}");
    }

    #[test]
    fn a_failed_run_with_no_test_failure_is_not_summarised() {
        // Every test passed but the command failed: nothing here explains why.
        assert_eq!(compact(PASSING, 1), None);
    }

    #[test]
    fn output_without_a_node_test_summary_is_left_alone() {
        assert_eq!(compact("✔ adds (0.3ms)\nall good\n", 0), None);
        assert_eq!(compact("", 0), None);
    }

    #[test]
    fn several_runs_in_one_script_are_all_compacted() {
        let input = format!("{PASSING}\n> second\n{FAILING}");
        let out = compact(&input, 1).expect("node --test output");
        assert_eq!(out.matches("ℹ tests").count(), 2, "{out}");
        assert!(out.contains("✖ inner fail"), "{out}");
        assert!(!out.contains("inner ok"), "{out}");
    }

    #[test]
    fn coloured_output_is_recognised() {
        let input = "\u{1b}[32m✔ adds \u{1b}[90m(0.3ms)\u{1b}[39m\u{1b}[39m\n\
                     \u{1b}[34mℹ tests 1\u{1b}[39m\n\u{1b}[34mℹ fail 0\u{1b}[39m\n";
        let out = compact(input, 0).expect("node --test output");
        assert!(!out.contains("adds"), "{out}");
        assert!(out.contains("tests 1"), "{out}");
    }
}
