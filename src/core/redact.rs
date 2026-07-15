//! Secret redaction for locally persisted data.
//!
//! Applied immediately before data is written to durable local storage:
//! command strings entering the tracking DB ([`crate::tracking::Tracker`])
//! and raw output entering tee recovery files ([`crate::core::tee`]).
//!
//! Only high-confidence secret formats are masked (well-known token
//! prefixes, Authorization headers, `password=`-style assignments, private
//! key blocks). bdo's fail-safe philosophy means normal data must survive
//! untouched: a missed secret is still protected by the 0600/0700
//! permissions layer, while a false positive would corrupt recovery data,
//! so patterns err on the side of NOT matching.

use regex::Regex;
use std::borrow::Cow;
use std::sync::OnceLock;

/// Replacement marker. Kept greppable so users can tell "bdo masked this"
/// apart from output that was empty to begin with.
const MASK: &str = "[REDACTED]";

/// (pattern, replacement) pairs. Replacements may reference capture groups
/// (`$1`) to preserve the non-secret context around the masked value.
fn rules() -> &'static Vec<(Regex, String)> {
    static RULES: OnceLock<Vec<(Regex, String)>> = OnceLock::new();
    RULES.get_or_init(|| {
        let full = |re: &str| (Regex::new(re).unwrap(), MASK.to_string());
        let keep_prefix = |re: &str| (Regex::new(re).unwrap(), format!("${{1}}{}", MASK));
        vec![
            // Private key blocks first (multi-line; would otherwise be
            // shredded piecemeal by the line-level rules below).
            full(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----"),
            // GitHub: classic + fine-grained PATs, OAuth, app tokens
            full(r"\bgh[pousr]_[A-Za-z0-9]{20,}\b"),
            full(r"\bgithub_pat_[A-Za-z0-9_]{22,}\b"),
            // GitLab PAT
            full(r"\bglpat-[A-Za-z0-9_-]{20,}\b"),
            // npm granular / automation tokens
            full(r"\bnpm_[A-Za-z0-9]{30,}\b"),
            // AWS access key IDs (long-term and STS)
            full(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
            // Slack tokens
            full(r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b"),
            // OpenAI / Anthropic style (sk-..., sk-ant-...) and Stripe
            full(r"\bsk-[A-Za-z0-9_-]{20,}\b"),
            full(r"\b[sr]k_(?:live|test)_[A-Za-z0-9]{16,}\b"),
            // Google API keys
            full(r"\bAIza[0-9A-Za-z_-]{35}\b"),
            // JWTs (three base64url segments, first one starting with {"...)
            full(r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b"),
            // Authorization headers: keep the scheme, mask the credential
            keep_prefix(r"(?i)\b(authorization\s*:\s*(?:bearer|basic|token)\s+)\S+"),
            // key=value / key: value assignments for well-known secret names.
            // Leading \b keeps compound words like `input_tokens:` out
            // (no word boundary after `_`), and the mandatory [=:] right
            // after the name keeps plurals like `tokens:` out.
            keep_prefix(
                r#"(?i)\b((?:api[_-]?key|access[_-]?token|auth[_-]?token|client[_-]?secret|password|passwd|pwd|secret|token)["']?\s*[=:]\s*["']?)([^\s"']{4,})"#,
            ),
        ]
    })
}

/// Cheap trigger substrings: if none of these occur (case-insensitively for
/// the alphabetic ones), no rule can match and the regex pass is skipped.
/// This keeps the hook hot path (every tracked command) at near-zero cost.
fn might_contain_secret(input: &str) -> bool {
    const TRIGGERS: &[&str] = &[
        "private key",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "glpat-",
        "npm_",
        "akia",
        "asia",
        "xox",
        "sk-",
        "sk_",
        "rk_",
        "aiza",
        "eyj",
        "authorization",
        "api",
        "token",
        "secret",
        "password",
        "passwd",
        "pwd",
    ];
    let lower = input.to_ascii_lowercase();
    TRIGGERS.iter().any(|t| lower.contains(t))
}

/// Mask known secret formats in `input`. Returns `Cow::Borrowed` when
/// nothing matched, so the common case allocates nothing beyond the
/// lowercase trigger scan.
pub fn redact_secrets(input: &str) -> Cow<'_, str> {
    if !might_contain_secret(input) {
        return Cow::Borrowed(input);
    }
    let mut out = Cow::Borrowed(input);
    for (re, replacement) in rules() {
        if re.is_match(&out) {
            out = Cow::Owned(re.replace_all(&out, replacement.as_str()).into_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_masked(input: &str, must_not_contain: &str) {
        let out = redact_secrets(input);
        assert!(
            !out.contains(must_not_contain),
            "secret survived redaction: {out}"
        );
        assert!(out.contains(MASK), "no mask marker in: {out}");
    }

    #[test]
    fn test_github_tokens() {
        assert_masked(
            "git push https://ghp_abcdefghijklmnopqrstuvwxyz012345@github.com/o/r",
            "ghp_abcdefghijklmnopqrstuvwxyz012345",
        );
        assert_masked(
            "export GH=github_pat_11ABCDEFG0abcdefghijklmn",
            "github_pat_11ABCDEFG0abcdefghijklmn",
        );
    }

    #[test]
    fn test_cloud_and_service_tokens() {
        assert_masked(
            "aws configure set key AKIAIOSFODNN7EXAMPLE",
            "AKIAIOSFODNN7EXAMPLE",
        );
        assert_masked(
            "slack --token xoxb-1234567890-abcdefghij",
            "xoxb-1234567890-abcdefghij",
        );
        assert_masked(
            "curl -H 'x-api-key: sk-ant-api03-abcdefghijklmnopqrst'",
            "sk-ant-api03-abcdefghijklmnopqrst",
        );
        assert_masked(
            "stripe listen --api-key sk_live_abcdefghijklmnop",
            "sk_live_abcdefghijklmnop",
        );
        assert_masked(
            "key=AIzaSyA-abcdefghijklmnopqrstuvwxyz01234",
            "AIzaSyA-abcdefghijklmnopqrstuvwxyz01234",
        );
        assert_masked("npm config set //registry.npmjs.org/:_authToken npm_abcdefghijklmnopqrstuvwxyz0123456789", "npm_abcdefghijklmnopqrstuvwxyz0123456789");
    }

    #[test]
    fn test_authorization_header_keeps_scheme() {
        let out =
            redact_secrets("curl -H \"Authorization: Bearer abc.def.ghi\" https://api.example.com");
        assert!(out.contains("Authorization: Bearer [REDACTED]"), "{out}");
        assert!(!out.contains("abc.def.ghi"), "{out}");
    }

    #[test]
    fn test_jwt() {
        assert_masked(
            "token eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NSJ9.SflKxwRJSMeKKF2QT4fwpM",
            "eyJhbGciOiJIUzI1NiJ9",
        );
    }

    #[test]
    fn test_assignment_forms() {
        let out = redact_secrets("mysql -u root --password=hunter22 db");
        assert!(out.contains("--password=[REDACTED]"), "{out}");
        let out = redact_secrets("api_key: \"abcd1234\"");
        assert!(!out.contains("abcd1234"), "{out}");
    }

    #[test]
    fn test_private_key_block() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\nmore\n-----END RSA PRIVATE KEY-----";
        let out = redact_secrets(pem);
        assert_eq!(out, MASK);
    }

    #[test]
    fn test_normal_commands_untouched() {
        for cmd in [
            "ls -la",
            "git status",
            "cargo test --workspace",
            "bdo gain --history",
            // bdo's own stats output: `tokens`/plural forms must not match
            "input_tokens: 1200 output_tokens: 300 saved 900 tokens",
            // short values after a secret-ish name are left alone (< 4 chars)
            "password: ok",
        ] {
            assert_eq!(redact_secrets(cmd), cmd, "false positive on: {cmd}");
        }
    }

    #[test]
    fn test_no_match_returns_borrowed() {
        let input = "git log --oneline -5";
        assert!(matches!(redact_secrets(input), Cow::Borrowed(_)));
    }
}
