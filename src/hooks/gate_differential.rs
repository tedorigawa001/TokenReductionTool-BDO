//! Differential test of the permission gate against a real shell.
//!
//! Every gate bypass so far was a disagreement between what the gate parses
//! and what the shell does, so this asks the shell. Commands are generated
//! from shell-syntax atoms and judged by `check_command_with_rules`; each one
//! the gate calls Allow is then really executed, with a PATH holding only
//! recording fakes (`git`, `canary`) in a throwaway directory:
//!
//! - allowed `git status` must run nothing but `git status …` and create no file;
//! - with a `git push --force*` deny rule, an allowed command must never run
//!   `git push --force`.
//!
//! The generator is seeded, so a failure reproduces. `GATE_AUDIT_ITERS`,
//! `GATE_AUDIT_SHELL` widen a local audit.

use super::permissions::{check_command_with_rules, PermissionVerdict};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const ATOMS: &[&str] = &[
    "git status",
    "git push",
    "git",
    "status",
    "push",
    "--force",
    "--for",
    "ce",
    "canary",
    " ",
    " ",
    " ",
    "\t",
    "\n",
    "\r",
    "\\\n",
    ";",
    "&",
    "&&",
    "||",
    "|",
    "|&",
    "#",
    "# it's",
    "# \"",
    "'",
    "\"",
    "\\",
    "$",
    "$'",
    "$\"",
    "`",
    "$(",
    ")",
    "(",
    "{",
    "}",
    "{ ",
    "; }",
    "<",
    ">",
    ">>",
    "2>",
    "2>&1",
    "&>",
    ">/dev/null",
    "</dev/null",
    "2>/dev/null",
    "<<",
    "<<<",
    "EOF",
    "\\n",
    "\\'",
    "\\\"",
    "a",
    "=",
    "X=1",
    "*",
    "?",
    "[",
    "]",
    "!",
    "~",
    "${X:-",
    "${IFS}",
    "$IFS",
    "\u{a0}",
    "\u{2028}",
    "\u{b}",
    "\u{c}",
    "if true; then ",
    "fi",
    "for i in a; do ",
    "done",
    "case a in a) ",
    "esac",
    ";;",
    "f(){ ",
    "time ",
    "! ",
    "exec ",
    "eval ",
    "command ",
    "--\"force\"",
    "\"--force\"",
    "--fo''rce",
    "--forc\\e",
    "--forc{e,}",
    "{--force,}",
    "--forc?",
    "\"git\"",
    "g\"\"it",
    "\\git",
    "'status'",
    "=canary",
    "*(e:canary:)",
    "$[1]",
    "^",
    "<(",
    ">(",
    "$((1))",
    "((",
    "))",
    "coproc ",
    "{a,b}",
    "${(e)X}",
    "|| canary",
    "; canary",
    "& canary",
    "\ncanary",
    "&& canary",
    "| canary",
    "--force\\\nx",
    "--force\\ ",
    "\"--force\"\\\n",
    "--force\r",
    "--force\u{a0}",
    "${X:---force}",
    "$X",
];

/// Small deterministic generator: failures must reproduce, and no dependency.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n
    }
}

fn generate(rng: &mut Lcg) -> String {
    let head = ["git status", "git status ", "git push ", "git push", ""][rng.next(5)];
    let mut cmd = String::from(head);
    for _ in 0..(1 + rng.next(5)) {
        cmd.push_str(ATOMS[rng.next(ATOMS.len())]);
    }
    cmd
}

/// Run `cmd` in `shell` among recording fakes that exit with `code`. Returns the
/// recorded invocations (`prog\targ\targ…`) and whether a stray file appeared.
fn execute(shell: &str, cmd: &str, dir: &Path, code: u8, run: usize) -> (Vec<String>, bool) {
    // Per-run paths: a backgrounded fake (`git status &`) may outlive its shell
    // and must not leak into the next command's record.
    let work = dir.join(format!("work{run}"));
    fs::create_dir_all(&work).unwrap();
    let log = dir.join(format!("log{run}"));
    let mut child = Command::new(shell)
        .args(["-c", cmd])
        .current_dir(&work)
        .env_clear()
        .env("PATH", dir.join("bin"))
        .env("FAKE_LOG", &log)
        .env("FAKE_EXIT", code.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn shell");
    let start = Instant::now();
    while child.try_wait().ok().flatten().is_none() {
        if start.elapsed() > Duration::from_secs(2) {
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let calls = fs::read_to_string(&log)
        .unwrap_or_default()
        .split('\u{1}')
        .filter(|c| !c.is_empty())
        .map(String::from)
        .collect();
    let stray = fs::read_dir(&work).map(|d| d.count() > 0).unwrap_or(false);
    let _ = fs::remove_dir_all(&work);
    let _ = fs::remove_file(&log);
    (calls, stray)
}

fn fake_bin(dir: &Path) {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    // A fake `bdo` stands in for the rewritten form: it records the program it
    // would run, so a rewrite can be compared with the command that was judged.
    let bdo = bin.join("bdo");
    fs::write(
        &bdo,
        "#!/bin/sh\nrec=\"$1\"\nshift\nfor a in \"$@\"; do rec=\"$rec\t$a\"; done\nprintf '%s\\001' \"$rec\" >> \"$FAKE_LOG\"\nexit \"$FAKE_EXIT\"\n",
    )
    .unwrap();
    fs::set_permissions(&bdo, fs::Permissions::from_mode(0o755)).unwrap();
    for prog in ["git", "canary"] {
        let p = bin.join(prog);
        fs::write(
            &p,
            format!(
                "#!/bin/sh\nrec={prog}\nfor a in \"$@\"; do rec=\"$rec\t$a\"; done\nprintf '%s\\001' \"$rec\" >> \"$FAKE_LOG\"\nexit \"$FAKE_EXIT\"\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn audit(shell: &str, iters: usize, threads: usize) -> Vec<String> {
    let allow_status = vec!["git status".to_string()];
    let allow_push = vec!["git status".to_string(), "git push *".to_string()];
    let deny = vec!["git push --force*".to_string()];
    let mut handles = Vec::new();
    for t in 0..threads {
        let (allow_status, allow_push, deny, shell) = (
            allow_status.clone(),
            allow_push.clone(),
            deny.clone(),
            shell.to_string(),
        );
        handles.push(std::thread::spawn(move || {
            let tmp = tempfile::tempdir().unwrap();
            fake_bin(tmp.path());
            let mut rng = Lcg(0x5EED + t as u64);
            let mut found = Vec::new();
            let mut run = 0;
            for _ in 0..iters / threads {
                let cmd = generate(&mut rng);
                let status_ok = check_command_with_rules(&cmd, &[], &[], &allow_status)
                    == PermissionVerdict::Allow;
                let push_ok = check_command_with_rules(&cmd, &deny, &[], &allow_push)
                    == PermissionVerdict::Allow;
                if !status_ok && !push_ok {
                    continue;
                }
                for code in [0u8, 1] {
                    run += 1;
                    let (calls, stray) = execute(&shell, &cmd, tmp.path(), code, run);
                    if status_ok {
                        let bad = calls
                            .iter()
                            .find(|c| *c != "git\tstatus" && !c.starts_with("git\tstatus\t"));
                        if let Some(b) = bad {
                            found.push(format!("ALLOW-RAN {cmd:?} ran {b:?}"));
                        }
                    }
                    if status_ok && stray {
                        found.push(format!("ALLOW-WROTE {cmd:?}"));
                    }
                    // The hook runs the rewrite, not the command it judged: the
                    // two must run the same programs with the same arguments.
                    if let Some(rewritten) = crate::discover::registry::rewrite_command(&cmd, &[], &[]) {
                        if rewritten != cmd {
                            run += 1;
                            let (mut after, _) = execute(&shell, &rewritten, tmp.path(), code, run);
                            let escalated = after.iter().find(|c| {
                                (status_ok && *c != "git\tstatus" && !c.starts_with("git\tstatus\t"))
                                    || (push_ok
                                        && (*c == "git\tpush\t--force" || c.starts_with("git\tpush\t--force\t")))
                            });
                            if let Some(e) = escalated {
                                found.push(format!("REWRITE-ESCALATES {cmd:?} -> {rewritten:?} ran {e:?}"));
                            }
                            // Informational: same programs expected, unless the
                            // original was a syntax error or backgrounded.
                            let mut before = calls.clone();
                            before.sort();
                            after.sort();
                            if before != after
                                && !before.is_empty()
                                && !after.is_empty()
                                && std::env::var_os("GATE_AUDIT_REWRITE").is_some()
                            {
                                found.push(format!(
                                    "REWRITE-DIFFERS {cmd:?} -> {rewritten:?}: {before:?} vs {after:?}"
                                ));
                            }
                        }
                    }
                    if push_ok {
                        if let Some(b) = calls.iter().find(|c| {
                            *c == "git\tpush\t--force" || c.starts_with("git\tpush\t--force\t")
                        }) {
                            found.push(format!("DENY-DODGED {cmd:?} ran {b:?}"));
                        }
                    }
                }
            }
            found
        }));
    }
    let mut all: Vec<String> = handles
        .into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect();
    all.sort();
    all.dedup();
    all
}

#[test]
fn gate_agrees_with_the_shell() {
    let shell = std::env::var("GATE_AUDIT_SHELL").unwrap_or_else(|_| "bash".to_string());
    let iters = std::env::var("GATE_AUDIT_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    // Absolute: the child's PATH holds only the fakes.
    let shell = which::which(&shell).expect("shell on PATH");
    let found = audit(shell.to_str().expect("utf-8 shell path"), iters, 4);
    assert!(
        found.is_empty(),
        "{} gate/shell disagreements:\n{}",
        found.len(),
        found.join("\n")
    );
}
