//! Processes incoming hook calls from AI agents and rewrites commands on the fly.
//!
//! Uses `writeln!(stdout, ...)` instead of `println!` — accidental stdout/stderr
//! corrupts the JSON protocol (Claude Code bug #4669 silently disables the hook).

use super::constants::PRE_TOOL_USE_KEY;
use super::permissions::{self, PermissionVerdict};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{self, Read, Write};

use crate::discover::registry::{has_heredoc, rewrite_command};

const STDIN_CAP: usize = 1_048_576; // 1 MiB

fn read_stdin_limited() -> Result<String> {
    let mut input = String::new();
    io::stdin()
        .take((STDIN_CAP + 1) as u64)
        .read_to_string(&mut input)
        .context("Failed to read stdin")?;
    if input.len() > STDIN_CAP {
        anyhow::bail!("hook stdin exceeds {} byte limit", STDIN_CAP);
    }
    Ok(input)
}

// ── Copilot hook (VS Code + Copilot CLI) ──────────────────────

/// Run the Copilot preToolUse hook without changing the original input.
/// Neither Copilot format exposes verified permissions to this hook.
pub fn run_copilot() -> Result<()> {
    read_stdin_limited()?;
    Ok(())
}

fn get_rewritten(cmd: &str) -> Option<String> {
    if has_heredoc(cmd) {
        return None;
    }

    let (excluded, transparent_prefixes) = crate::core::config::Config::load()
        .map(|c| (c.hooks.exclude_commands, c.hooks.transparent_prefixes))
        .unwrap_or_default();

    let rewritten = rewrite_command(cmd, &excluded, &transparent_prefixes)?;

    if rewritten == cmd {
        return None;
    }

    Some(rewritten)
}

enum HookDecision {
    AllowRewrite(String),
    /// An ask rule matched. The host must still ask, so the decision is sent
    /// explicitly; otherwise it would match its rules against `bdo …`.
    AskRuleRewrite(String),
    Defer,
    Deny,
}

fn decide_from_verdict(cmd: &str, verdict: PermissionVerdict) -> HookDecision {
    decide_rewrite(cmd, verdict, || get_rewritten(cmd))
}

fn decide_rewrite(
    cmd: &str,
    verdict: PermissionVerdict,
    rewrite: impl FnOnce() -> Option<String>,
) -> HookDecision {
    let build_decision = match verdict {
        PermissionVerdict::Deny => return HookDecision::Deny,
        // Without a matching rule, the host must evaluate the original input.
        PermissionVerdict::Default => return HookDecision::Defer,
        PermissionVerdict::Allow => HookDecision::AllowRewrite,
        PermissionVerdict::Ask => HookDecision::AskRuleRewrite,
    };
    if crate::discover::lexer::contains_unattestable_construct(cmd) {
        return HookDecision::Defer;
    }
    rewrite().map_or(HookDecision::Defer, build_decision)
}

fn decide_hook_action(cmd: &str, host: permissions::Host) -> HookDecision {
    decide_from_verdict(cmd, permissions::check_command_for(cmd, host))
}

/// [`decide_from_verdict`] plus agent attribution for hosts whose shell
/// children carry no sniffable env vars (Gemini CLI): every rewritten
/// `bdo` segment gets a `BDO_AGENT=<agent>` prefix so `bdo gain --by-agent`
/// attributes the run deterministically. A command that already invokes `bdo`
/// directly (agents following the instructions file do this) normally needs no
/// rewrite — here it still gets one, carrying just the attribution.
fn decide_from_verdict_for_agent(
    cmd: &str,
    verdict: PermissionVerdict,
    agent: &str,
) -> HookDecision {
    use crate::discover::registry::attribute_agent;
    decide_rewrite(cmd, verdict, || match get_rewritten(cmd) {
        Some(r) => Some(attribute_agent(&r, agent)),
        // Heredocs are never touched (same rule as get_rewritten).
        None if has_heredoc(cmd) => None,
        None => {
            let attributed = attribute_agent(cmd, agent);
            (attributed != cmd).then_some(attributed)
        }
    })
}

fn decide_hook_action_for_agent(cmd: &str, host: permissions::Host, agent: &str) -> HookDecision {
    decide_from_verdict_for_agent(cmd, permissions::check_command_for(cmd, host), agent)
}

// ── Gemini hook ───────────────────────────────────────────────

/// Run the Gemini CLI BeforeTool hook.
pub fn run_gemini() -> Result<()> {
    let input = read_stdin_limited()?;

    let json: Value = serde_json::from_str(&input).context("Failed to parse hook input as JSON")?;

    let tool_name = json.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");

    if tool_name != "run_shell_command" {
        print_gemini_allow();
        return Ok(());
    }

    let cmd = json
        .pointer("/tool_input/command")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if cmd.is_empty() {
        print_gemini_allow();
        return Ok(());
    }

    match decide_hook_action_for_agent(cmd, permissions::Host::Gemini, "gemini") {
        HookDecision::Deny => {
            let _ = writeln!(
                io::stdout(),
                r#"{{"decision":"deny","reason":"Blocked by Bushido permission rule"}}"#
            );
        }
        // Gemini's BeforeTool only documents `deny`/`block` (absent = allow); it
        // has no `allow`/`ask_user` value. So for a rewrite we omit `decision`
        // and merge the new command via `hookSpecificOutput.tool_input`.
        HookDecision::AllowRewrite(ref rewritten) => {
            audit_log("rewrite", cmd, rewritten);
            print_gemini_rewrite(rewritten);
        }
        // Gemini has no `ask` value, and its confirmationRequired list would
        // not match `bdo …`: leave the command for Gemini to confirm.
        HookDecision::AskRuleRewrite(_) | HookDecision::Defer => print_gemini_allow(),
    }

    Ok(())
}

/// Gemini "allow": empty JSON. An absent `decision` means permission; only
/// `deny`/`block` are valid action values, so we never emit `allow`/`ask_user`.
fn print_gemini_allow() {
    let _ = writeln!(io::stdout(), "{{}}");
}

/// Gemini allow + rewrite: merge a new command via `hookSpecificOutput.tool_input`
/// (no `decision` — absent means allow).
fn print_gemini_rewrite(rewritten: &str) {
    let _ = writeln!(io::stdout(), "{}", gemini_rewrite_json(rewritten));
}

fn gemini_rewrite_json(rewritten: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": { "tool_input": { "command": rewritten } }
    })
    .to_string()
}

// ── Audit logging ─────────────────────────────────────────────

/// Best-effort audit log when BDO_HOOK_AUDIT=1.
fn audit_log(action: &str, original: &str, rewritten: &str) {
    if std::env::var("BDO_HOOK_AUDIT").as_deref() != Ok("1") {
        return;
    }
    let _ = audit_log_inner(action, original, rewritten);
}

/// Escape newlines to prevent log-line injection in the pipe-delimited audit log.
fn sanitize_log_field(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn prepare_audit_field(s: &str) -> String {
    sanitize_log_field(&crate::core::redact::redact_secrets(s))
}

fn open_private_audit_log(path: &std::path::Path) -> Option<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return None;
    }
    let file = options.open(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .ok()?;
    }
    Some(file)
}

fn audit_log_inner(action: &str, original: &str, rewritten: &str) -> Option<()> {
    // Resolve via the shared helper so the writer honors BDO_AUDIT_DIR and stays
    // aligned with the reader (`bdo hook-audit`).
    let path = crate::hooks::hook_audit_cmd::default_log_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    let mut file = open_private_audit_log(&path)?;
    // UTC with its `Z`, so `hook-audit --since` places it on any clock.
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    writeln!(
        file,
        "{} | {} | {} | {}",
        ts,
        action,
        prepare_audit_field(original),
        prepare_audit_field(rewritten)
    )
    .ok()
}

// ── Claude Code native hook ────────────────────────────────────

enum PayloadAction {
    Rewrite {
        cmd: String,
        rewritten: String,
        output: Value,
    },
    Skip {
        reason: &'static str,
        cmd: String,
    },
    Ignore,
}

fn process_claude_payload(v: &Value, decide: impl FnOnce(&str) -> HookDecision) -> PayloadAction {
    let cmd = match v
        .pointer("/tool_input/command")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())
    {
        Some(c) => c,
        None => return PayloadAction::Ignore,
    };

    let (rewritten, decision) = match decide(cmd) {
        HookDecision::Deny => {
            return PayloadAction::Skip {
                reason: "skip:deny_rule",
                cmd: cmd.to_string(),
            }
        }
        HookDecision::Defer => {
            return PayloadAction::Skip {
                reason: "skip:defer",
                cmd: cmd.to_string(),
            }
        }
        HookDecision::AllowRewrite(r) => (r, "allow"),
        HookDecision::AskRuleRewrite(r) => (r, "ask"),
    };

    let updated_input = {
        let mut ti = v.get("tool_input").cloned().unwrap_or_else(|| json!({}));
        if let Some(obj) = ti.as_object_mut() {
            obj.insert("command".into(), Value::String(rewritten.clone()));
        }
        ti
    };

    let hook_output = json!({
        "hookEventName": PRE_TOOL_USE_KEY,
        "permissionDecisionReason": "Bushido auto-rewrite",
        "updatedInput": updated_input,
        "permissionDecision": decision
    });

    PayloadAction::Rewrite {
        cmd: cmd.to_string(),
        rewritten,
        output: json!({ "hookSpecificOutput": hook_output }),
    }
}

/// Run the Claude Code PreToolUse hook natively.
pub fn run_claude() -> Result<()> {
    let input = read_stdin_limited()?;

    let input = input.trim();
    if input.is_empty() {
        return Ok(());
    }

    let v: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(io::stderr(), "[bdo hook] Failed to parse JSON input: {e}");
            return Ok(());
        }
    };

    match process_claude_payload(&v, |cmd| decide_hook_action(cmd, permissions::Host::Claude)) {
        PayloadAction::Rewrite {
            cmd,
            rewritten,
            output,
        } => {
            audit_log("rewrite", &cmd, &rewritten);
            let _ = writeln!(io::stdout(), "{output}");
        }
        PayloadAction::Skip { reason, cmd } => {
            audit_log(reason, &cmd, "");
        }
        PayloadAction::Ignore => {}
    }

    Ok(())
}

#[cfg(test)]
fn run_claude_inner(input: &str) -> Option<String> {
    let v: Value = serde_json::from_str(input).ok()?;
    match process_claude_payload(&v, |cmd| decide_from_verdict(cmd, PermissionVerdict::Allow)) {
        PayloadAction::Rewrite { output, .. } => Some(output.to_string()),
        _ => None,
    }
}

// ── Cursor native hook ─────────────────────────────────────────

/// Cursor on Windows ships hook payloads with one or more leading
/// UTF-8 BOMs (`EF BB BF`, sometimes doubled), which serde_json
/// refuses to parse. Strip them defensively so the rewrite path keeps
/// working instead of silently returning `{}`.
fn strip_leading_bom(input: &str) -> &str {
    let mut s = input;
    while let Some(rest) = s.strip_prefix('\u{feff}') {
        s = rest;
    }
    s
}

/// Run the Cursor Agent hook natively.
pub fn run_cursor() -> Result<()> {
    let input = read_stdin_limited()?;

    let input = strip_leading_bom(&input).trim();
    if input.is_empty() {
        let _ = writeln!(io::stdout(), "{{}}");
        return Ok(());
    }

    let v: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(_) => {
            let _ = writeln!(io::stdout(), "{{}}");
            return Ok(());
        }
    };

    let cmd = match v
        .pointer("/tool_input/command")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())
    {
        Some(c) => c.to_string(),
        None => {
            let _ = writeln!(io::stdout(), "{{}}");
            return Ok(());
        }
    };

    let output = match decide_hook_action(&cmd, permissions::Host::Cursor) {
        HookDecision::AllowRewrite(rewritten) => {
            audit_log("rewrite", &cmd, &rewritten);
            cursor_allow(&rewritten)
        }
        other => {
            if matches!(other, HookDecision::Deny) {
                audit_log("deny", &cmd, "");
            }
            "{}".to_string()
        }
    };
    let _ = writeln!(io::stdout(), "{output}");
    Ok(())
}

fn cursor_allow(rewritten: &str) -> String {
    json!({
        "continue": true,
        "permission": "allow",
        "updated_input": { "command": rewritten }
    })
    .to_string()
}

#[cfg(test)]
fn run_cursor_inner(input: &str) -> String {
    run_cursor_inner_with_rules(input, &[], &[], &[])
}

#[cfg(test)]
fn run_cursor_inner_with_rules(
    input: &str,
    deny_rules: &[String],
    ask_rules: &[String],
    allow_rules: &[String],
) -> String {
    let input = strip_leading_bom(input);
    let v: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(_) => return "{}".to_string(),
    };

    let cmd = match v
        .pointer("/tool_input/command")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())
    {
        Some(c) => c.to_string(),
        None => return "{}".to_string(),
    };

    let verdict = permissions::check_command_with_rules(&cmd, deny_rules, ask_rules, allow_rules);
    match decide_from_verdict(&cmd, verdict) {
        HookDecision::AllowRewrite(rewritten) => cursor_allow(&rewritten),
        _ => "{}".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite_command_no_prefixes(cmd: &str, excluded: &[String]) -> Option<String> {
        crate::discover::registry::rewrite_command(cmd, excluded, &[])
    }

    #[test]
    fn test_get_rewritten_supported() {
        assert!(get_rewritten("git status").is_some());
    }

    #[test]
    fn test_get_rewritten_unsupported() {
        assert!(get_rewritten("htop").is_none());
    }

    #[test]
    fn test_get_rewritten_already_rtk() {
        assert!(get_rewritten("bdo git status").is_none());
    }

    #[test]
    fn test_get_rewritten_heredoc() {
        assert!(get_rewritten("cat <<'EOF'\nhello\nEOF").is_none());
    }

    // --- Gemini format ---

    // Gemini's BeforeTool documents only `deny`/`block` (absent = allow); it has
    // no `allow`/`ask_user`. Allow must be empty JSON, rewrites omit `decision`.
    #[test]
    fn test_gemini_allow_has_no_decision() {
        let v: Value = serde_json::from_str("{}").unwrap();
        assert!(v.get("decision").is_none());
    }

    #[test]
    fn test_gemini_rewrite_json_omits_decision_and_merges_command() {
        let v: Value = serde_json::from_str(&gemini_rewrite_json("bdo git status")).unwrap();
        assert!(
            v.get("decision").is_none(),
            "rewrite must not set a decision"
        );
        assert_eq!(
            v["hookSpecificOutput"]["tool_input"]["command"],
            "bdo git status"
        );
    }

    #[test]
    fn test_gemini_hook_uses_rewrite_command() {
        assert_eq!(
            rewrite_command_no_prefixes("git status", &[]),
            Some("bdo git status".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("cargo test", &[]),
            Some("bdo cargo test".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("bdo git status", &[]),
            Some("bdo git status".into())
        );
        assert_eq!(rewrite_command_no_prefixes("cat <<EOF", &[]), None);
    }

    #[test]
    fn test_gemini_hook_excluded_commands() {
        let excluded = vec!["curl".to_string()];
        assert_eq!(
            rewrite_command_no_prefixes("curl https://example.com", &excluded),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("git status", &excluded),
            Some("bdo git status".into())
        );
    }

    #[test]
    fn test_gemini_hook_env_prefix_preserved() {
        assert_eq!(
            rewrite_command_no_prefixes("RUST_LOG=debug cargo test", &[]),
            Some("RUST_LOG=debug bdo cargo test".into())
        );
    }

    // --- Claude handler ---

    fn claude_input(cmd: &str) -> String {
        json!({
            "tool_name": "Bash",
            "tool_input": { "command": cmd }
        })
        .to_string()
    }

    fn claude_input_with_fields(cmd: &str, timeout: u64, description: &str) -> String {
        json!({
            "tool_name": "Bash",
            "tool_input": {
                "command": cmd,
                "timeout": timeout,
                "description": description
            }
        })
        .to_string()
    }

    #[test]
    fn test_claude_rewrite_git_status() {
        let result = run_claude_inner(&claude_input("git status")).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let cmd = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(cmd, "bdo git status");
    }

    #[test]
    fn test_claude_rewrite_preserves_tool_input_fields() {
        let input = claude_input_with_fields("git status", 30000, "Check repo status");
        let result = run_claude_inner(&input).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let updated = &v["hookSpecificOutput"]["updatedInput"];
        assert_eq!(updated["command"], "bdo git status");
        assert_eq!(updated["timeout"], 30000);
        assert_eq!(updated["description"], "Check repo status");
    }

    #[test]
    fn test_claude_passthrough_no_output() {
        assert!(run_claude_inner(&claude_input("htop")).is_none());
    }

    #[test]
    fn test_claude_substitution_not_rewritten() {
        // A substitution payload must never be rewritten into updatedInput;
        // Bushido skips so Claude Code evaluates the original command natively.
        assert!(run_claude_inner(&claude_input("git status `rm -rf /tmp/x`")).is_none());
        assert!(run_claude_inner(&claude_input("git status $(rm -rf /tmp/x)")).is_none());
        assert!(run_claude_inner(&claude_input("git log --pretty=\"$(rm -rf /tmp/x)\"")).is_none());
    }

    #[test]
    fn test_claude_file_redirect_not_rewritten() {
        assert!(run_claude_inner(&claude_input("git log > /tmp/out.txt")).is_none());
    }

    #[test]
    fn test_claude_fd_dup_redirect_still_rewritten() {
        // `2>&1` is attestable — the rewrite proceeds as normal.
        assert!(run_claude_inner(&claude_input("git status 2>&1")).is_some());
    }

    #[test]
    fn test_claude_heredoc_passthrough() {
        assert!(run_claude_inner(&claude_input("cat <<EOF\nhello\nEOF")).is_none());
    }

    #[test]
    fn test_claude_already_rtk_passthrough() {
        assert!(run_claude_inner(&claude_input("bdo git status")).is_none());
    }

    #[test]
    fn test_claude_empty_command_passthrough() {
        let input = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "" }
        })
        .to_string();
        assert!(run_claude_inner(&input).is_none());
    }

    #[test]
    fn test_claude_malformed_json_passthrough() {
        assert!(run_claude_inner("not valid json {{{").is_none());
    }

    #[test]
    fn test_claude_env_prefix_preserved() {
        let result = run_claude_inner(&claude_input("GIT_PAGER=cat git status")).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let cmd = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(cmd, "GIT_PAGER=cat bdo git status");
    }

    #[test]
    fn test_claude_compound_command() {
        let result = run_claude_inner(&claude_input("git add . && cargo test")).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let cmd = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(cmd, "bdo git add . && bdo cargo test");
    }

    #[test]
    fn test_claude_json_output_structure() {
        let result = run_claude_inner(&claude_input("git status")).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let hook = &v["hookSpecificOutput"];

        assert_eq!(hook["hookEventName"], PRE_TOOL_USE_KEY);
        assert_eq!(hook["permissionDecision"], "allow");
        assert_eq!(hook["permissionDecisionReason"], "Bushido auto-rewrite");
        assert!(hook["updatedInput"].is_object());
        assert!(hook["updatedInput"]["command"].is_string());
    }

    #[test]
    fn test_claude_no_tool_input_passthrough() {
        let input = json!({ "tool_name": "Bash" }).to_string();
        assert!(run_claude_inner(&input).is_none());
    }

    // --- Cursor handler ---

    fn cursor_input(cmd: &str) -> String {
        json!({
            "tool_name": "Bash",
            "tool_input": { "command": cmd }
        })
        .to_string()
    }

    fn run_cursor_allowed(input: &str) -> String {
        run_cursor_inner_with_rules(input, &[], &[], &["*".to_string()])
    }

    #[test]
    fn test_cursor_rewrite_flat_format() {
        let result = run_cursor_allowed(&cursor_input("git status"));
        let v: Value = serde_json::from_str(&result).unwrap();
        // Cursor preToolUse expects allow/deny for rewrite application.
        assert_eq!(v["permission"], "allow");
        assert_eq!(v["updated_input"]["command"], "bdo git status");
        assert!(v.get("hookSpecificOutput").is_none());
        // `continue: true` keeps the Cursor preToolUse panel from collapsing
        // to `Output: {}`; without it the rewrite is invisible to users.
        assert_eq!(v["continue"], true);
    }

    #[test]
    fn test_cursor_no_allow_rule_defers() {
        assert_eq!(run_cursor_inner(&cursor_input("git status")), "{}");
    }

    #[test]
    fn test_cursor_substitution_defers_even_when_allowed() {
        assert_eq!(
            run_cursor_allowed(&cursor_input("git status `rm -rf /tmp/x`")),
            "{}"
        );
        assert_eq!(
            run_cursor_allowed(&cursor_input("git status $(rm -rf /tmp/x)")),
            "{}"
        );
    }

    #[test]
    fn test_cursor_unallowed_segment_defers() {
        let out = run_cursor_inner_with_rules(
            &cursor_input("git status && rm -rf /tmp/x"),
            &[],
            &[],
            &["git *".to_string()],
        );
        assert_eq!(out, "{}");
    }

    #[test]
    fn test_cursor_passthrough_empty_json() {
        let result = run_cursor_inner(&cursor_input("htop"));
        assert_eq!(result, "{}");
    }

    #[test]
    fn test_cursor_empty_input_empty_json() {
        let result = run_cursor_inner("");
        assert_eq!(result, "{}");
    }

    #[test]
    fn test_cursor_heredoc_passthrough() {
        let result = run_cursor_inner(&cursor_input("cat <<EOF\nhello\nEOF"));
        assert_eq!(result, "{}");
    }

    #[test]
    fn test_cursor_already_rtk_passthrough() {
        let result = run_cursor_inner(&cursor_input("bdo git status"));
        assert_eq!(result, "{}");
    }

    #[test]
    fn test_cursor_no_hook_specific_output() {
        let result = run_cursor_allowed(&cursor_input("cargo test"));
        let v: Value = serde_json::from_str(&result).unwrap();
        assert!(v.get("hookSpecificOutput").is_none());
        assert_eq!(v["permission"], "allow");
        assert_eq!(v["continue"], true);
    }

    #[test]
    fn test_cursor_compound_rewrite_includes_continue() {
        let cmd = "cd \"/tmp/proj\" && git status";
        let result = run_cursor_allowed(&cursor_input(cmd));
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["continue"], true);
        assert_eq!(v["permission"], "allow");
        assert_eq!(
            v["updated_input"]["command"],
            "cd \"/tmp/proj\" && bdo git status"
        );
    }

    #[test]
    fn test_cursor_strips_single_utf8_bom() {
        // Some Cursor builds prepend a single UTF-8 BOM to hook stdin.
        // serde_json rejects BOM-prefixed input, so without the strip
        // the hook returned `{}` and the rewrite became a silent no-op.
        let payload = cursor_input("git status");
        let with_single_bom = format!("\u{feff}{}", payload);
        let result = run_cursor_allowed(&with_single_bom);
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["continue"], true);
        assert_eq!(v["permission"], "allow");
        assert_eq!(v["updated_input"]["command"], "bdo git status");
    }

    #[test]
    fn test_cursor_strips_double_utf8_bom() {
        // Cursor on Windows ships hook stdin with **two** leading
        // UTF-8 BOMs (`EF BB BF EF BB BF`), confirmed via a stdin
        // tracer wrapping `rtk hook cursor` on Cursor 3.2.x. This is
        // the real-world payload shape the loop needs to survive.
        let payload = cursor_input("git status");
        let with_double_bom = format!("\u{feff}\u{feff}{}", payload);
        let result = run_cursor_allowed(&with_double_bom);
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["continue"], true);
        assert_eq!(v["permission"], "allow");
        assert_eq!(v["updated_input"]["command"], "bdo git status");
    }

    #[test]
    fn test_strip_leading_bom_helper() {
        // Direct unit test on the helper so future refactors can't
        // regress the loop semantics without a clear failure signal.
        assert_eq!(strip_leading_bom(""), "");
        assert_eq!(strip_leading_bom("hello"), "hello");
        assert_eq!(strip_leading_bom("\u{feff}hello"), "hello");
        assert_eq!(strip_leading_bom("\u{feff}\u{feff}hello"), "hello");
        assert_eq!(strip_leading_bom("\u{feff}\u{feff}\u{feff}hello"), "hello");
        // BOM in the middle is preserved (not "leading").
        assert_eq!(strip_leading_bom("a\u{feff}b"), "a\u{feff}b");
    }

    // --- Audit logging ---

    #[test]
    fn test_audit_log_silent_when_disabled() {
        std::env::remove_var("BDO_HOOK_AUDIT");
        audit_log("test", "git status", "bdo git status");
    }

    #[test]
    fn test_audit_log_format_four_fields() {
        let tmp = std::env::temp_dir().join("rtk-test-audit");
        let _ = std::fs::create_dir_all(&tmp);
        let log_path = tmp.join("hook-audit.log");
        let _ = std::fs::remove_file(&log_path);

        {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .unwrap();
            let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
            writeln!(file, "{} | rewrite | git status | bdo git status", ts).unwrap();
        }

        let content = std::fs::read_to_string(&log_path).unwrap();
        let parts: Vec<&str> = content.trim().split(" | ").collect();
        assert_eq!(
            parts.len(),
            4,
            "Expected 4 pipe-delimited fields, got: {:?}",
            parts
        );
        assert_eq!(parts[1], "rewrite");
        assert_eq!(parts[2], "git status");
        assert_eq!(parts[3], "bdo git status");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- Adversarial tests ---

    #[test]
    fn test_audit_log_sanitizes_newlines() {
        let sanitized = sanitize_log_field("git status\nfake | inject | evil");
        assert!(!sanitized.contains('\n'));
        assert!(sanitized.contains("\\n"));
    }

    #[test]
    fn test_audit_log_sanitizes_pipe_delimiter() {
        let sanitized = sanitize_log_field("git log | head");
        assert!(
            !sanitized.contains(" | "),
            "unescaped ' | ' breaks field parsing: {}",
            sanitized
        );
        assert!(sanitized.contains("\\|"));
    }

    #[test]
    fn test_audit_field_redacts_secrets() {
        let field = prepare_audit_field(
            "curl -H 'Authorization: Bearer ghp_abcdefghijklmnopqrstuvwxyz012345'",
        );
        assert!(!field.contains("ghp_abcdefghijklmnopqrstuvwxyz012345"));
        assert!(field.contains("[REDACTED]"));
    }

    #[cfg(unix)]
    #[test]
    fn test_audit_writer_refuses_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        let link = dir.path().join("hook-audit.log");
        std::fs::write(&victim, "safe").unwrap();
        symlink(&victim, &link).unwrap();

        assert!(open_private_audit_log(&link).is_none());
        assert_eq!(std::fs::read_to_string(victim).unwrap(), "safe");
    }

    #[test]
    fn test_claude_unicode_null_passthrough() {
        let input = claude_input("git status \u{0000}\u{FEFF}");
        let _ = run_claude_inner(&input);
    }

    #[test]
    fn test_claude_extremely_long_command() {
        let long_cmd = format!("git status {}", "A".repeat(100_000));
        let input = claude_input(&long_cmd);
        let _ = run_claude_inner(&input);
    }

    #[test]
    fn test_cursor_deny_blocks_rewrite() {
        use super::permissions::check_command_with_rules;
        let deny = vec!["git status".to_string()];
        assert_eq!(
            check_command_with_rules("git status", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_gemini_deny_blocks_rewrite() {
        use super::permissions::check_command_with_rules;
        let deny = vec!["cargo test".to_string()];
        assert_eq!(
            check_command_with_rules("cargo test", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
        // Denied commands must not be rewritten — Gemini handler checks deny before rewrite
        assert!(
            get_rewritten("cargo test").is_some(),
            "cargo test should be rewritable when not denied"
        );
    }

    // --- Shared decision flow (all hosts route through this) ---

    fn decide_with_rules(
        cmd: &str,
        deny: &[String],
        ask: &[String],
        allow: &[String],
    ) -> HookDecision {
        let verdict = permissions::check_command_with_rules(cmd, deny, ask, allow);
        decide_from_verdict(cmd, verdict)
    }

    fn all_allowed() -> Vec<String> {
        vec!["*".to_string()]
    }

    #[test]
    fn test_decide_allow_for_attestable_allowed_command() {
        assert!(matches!(
            decide_with_rules("git status", &[], &[], &all_allowed()),
            HookDecision::AllowRewrite(_)
        ));
    }

    #[test]
    fn test_decide_defers_for_default_verdict() {
        assert!(matches!(
            decide_with_rules("git status", &[], &[], &[]),
            HookDecision::Defer
        ));
    }

    #[test]
    fn test_decide_deny() {
        assert!(matches!(
            decide_with_rules(
                "rm -rf /tmp/x",
                &["rm -rf".to_string()],
                &[],
                &all_allowed()
            ),
            HookDecision::Deny
        ));
    }

    #[test]
    fn test_special_quotes_defer_across_hook_hosts() {
        let allow = vec!["git status".to_string()];
        for cmd in [
            r#"git status $'a\'b'; printf BDO_GATE_BYPASS; echo $'c\'d'"#,
            r#"git status $'a\'b' $(printf BDO_GATE_BYPASS) $'c\'d'"#,
            "git status $\\\n'a\\'b'; printf BDO_GATE_BYPASS; echo $'c\\'d'",
        ] {
            assert!(matches!(
                decide_with_rules(cmd, &[], &[], &allow),
                HookDecision::Defer
            ));
            // The attributed helper is shared by Gemini and Copilot integrations.
            assert!(matches!(
                decide_from_verdict_for_agent(cmd, PermissionVerdict::Allow, "gemini"),
                HookDecision::Defer
            ));
            let input = claude_input(cmd);
            assert!(run_claude_inner(&input).is_none(), "Claude: {cmd:?}");
            assert_eq!(
                run_cursor_inner_with_rules(&input, &[], &[], &allow),
                "{}",
                "Cursor: {cmd:?}"
            );
        }
        assert!(matches!(
            decide_with_rules("git status 2>&1", &[], &[], &allow),
            HookDecision::AllowRewrite(_)
        ));
    }

    #[test]
    fn test_decide_defer_for_substitution_even_when_allowed() {
        for cmd in [
            "git status `rm -rf /tmp/x`",
            "git status $(rm -rf /tmp/x)",
            "git log --pretty=\"$(rm -rf /tmp/x)\"",
        ] {
            assert!(
                matches!(
                    decide_with_rules(cmd, &[], &[], &all_allowed()),
                    HookDecision::Defer
                ),
                "expected Defer for {cmd}"
            );
        }
    }

    #[test]
    fn test_decide_defer_for_file_redirect() {
        assert!(matches!(
            decide_with_rules("git log > /tmp/out.txt", &[], &[], &all_allowed()),
            HookDecision::Defer
        ));
    }

    #[test]
    fn test_decide_allow_for_fd_dup_redirect() {
        assert!(matches!(
            decide_with_rules("git status 2>&1", &[], &[], &all_allowed()),
            HookDecision::AllowRewrite(_)
        ));
    }

    // --- Gemini rendering ---

    fn gemini_render(cmd: &str, deny: &[String], ask: &[String], allow: &[String]) -> String {
        // Mirror run_gemini: the agent-attributing decision path.
        let verdict = permissions::check_command_with_rules(cmd, deny, ask, allow);
        match decide_from_verdict_for_agent(cmd, verdict, "gemini") {
            HookDecision::Deny => {
                r#"{"decision":"deny","reason":"Blocked by Bushido permission rule"}"#.to_string()
            }
            // Gemini has no allow/ask value: rewrite via tool_input (no decision).
            HookDecision::AllowRewrite(r) => gemini_rewrite_json(&r),
            HookDecision::AskRuleRewrite(_) | HookDecision::Defer => "{}".to_string(),
        }
    }

    #[test]
    #[cfg(not(windows))]
    fn test_gemini_allow_emits_rewrite() {
        let v: Value =
            serde_json::from_str(&gemini_render("git status", &[], &[], &all_allowed())).unwrap();
        assert!(v.get("decision").is_none());
        assert_eq!(
            v["hookSpecificOutput"]["tool_input"]["command"],
            "BDO_AGENT=gemini bdo git status"
        );
    }

    #[test]
    fn test_gemini_default_leaves_input_unchanged() {
        for command in ["git status", "bdo git status"] {
            assert_eq!(gemini_render(command, &[], &[], &[]), "{}");
        }
    }

    #[test]
    #[cfg(not(windows))]
    fn test_gemini_already_bdo_gets_attribution_only_rewrite() {
        let v: Value =
            serde_json::from_str(&gemini_render("bdo git status", &[], &[], &all_allowed()))
                .unwrap();
        assert_eq!(
            v["hookSpecificOutput"]["tool_input"]["command"],
            "BDO_AGENT=gemini bdo git status"
        );
    }

    #[test]
    fn test_gemini_attribution_is_idempotent() {
        // Second pass over already-attributed output must defer (no rewrite),
        // otherwise the hook would loop rewriting its own output forever.
        assert_eq!(
            gemini_render("BDO_AGENT=gemini bdo git status", &[], &[], &all_allowed()),
            "{}"
        );
    }

    #[test]
    fn test_gemini_substitution_never_attributed() {
        // Unattestable constructs are checked before attribution: even a
        // command that names bdo must not be touched when it embeds `$( )`.
        assert_eq!(
            gemini_render("bdo git status $(rm -rf /tmp/x)", &[], &[], &all_allowed()),
            "{}"
        );
    }

    #[test]
    fn test_gemini_defer_allows_without_rewrite() {
        let v: Value = serde_json::from_str(&gemini_render(
            "git status `rm -rf /tmp/x`",
            &[],
            &[],
            &all_allowed(),
        ))
        .unwrap();
        assert!(v.get("decision").is_none());
        assert!(v.get("hookSpecificOutput").is_none());
    }

    #[test]
    fn test_gemini_deny_decision() {
        let v: Value = serde_json::from_str(&gemini_render(
            "rm -rf /tmp/x",
            &["rm -rf".to_string()],
            &[],
            &[],
        ))
        .unwrap();
        assert_eq!(v["decision"], "deny");
    }
}
