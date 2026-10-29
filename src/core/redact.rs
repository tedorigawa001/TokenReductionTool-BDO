//! Secret redaction for locally persisted data.
//!
//! Applied immediately before data is written to durable local storage:
//! command strings entering the tracking DB ([`crate::tracking::Tracker`])
//! and raw output entering tee recovery files ([`crate::core::tee`]).
//!
//! Only high-confidence secret formats are masked (well-known token
//! prefixes, Authorization/Cookie headers, `password=`-style and
//! `*_TOKEN=`-style assignments, secret-named flags, URL userinfo passwords,
//! webhook and signed URLs, private key blocks). bdo's fail-safe philosophy means normal data must survive
//! untouched: a missed secret is still protected by the 0600/0700
//! permissions layer, while a false positive would corrupt recovery data,
//! so patterns err on the side of NOT matching.

use regex::{Captures, Regex};
use std::borrow::Cow;
use std::sync::OnceLock;

/// Replacement marker. Kept greppable so users can tell "bdo masked this"
/// apart from output that was empty to begin with.
const MASK: &str = "[REDACTED]";

/// How a rule rewrites a match: the whole match, or everything after the
/// non-secret context captured in group 1 (`--password `, `Cookie: `, …).
#[derive(Clone, Copy)]
enum Mask {
    Full,
    KeepPrefix,
    /// Group 1, the mask, then a literal `@` (URL userinfo).
    KeepPrefixThenAt,
    /// Group 1 kept, then the shell word in group `v` masked (see
    /// [`mask_shell_word`]). `allow_dash`: whether a value may begin with `-`
    /// (not for flags, where `--token -x` means the next flag).
    ShellWord {
        allow_dash: bool,
    },
    /// Group 1 kept, then the `user:password` word in group `v` with only
    /// the password masked (see [`mask_curl_user`]).
    CurlUser,
}

/// One redaction rule. `triggers` are lowercase substrings at least one of
/// which must occur in the lowercased input before the regex is even
/// compiled: bdo runs once per command, so compiling every rule on every
/// tracked command would cost milliseconds on the hook's hot path.
/// `requires` lists further substrings that must ALL occur as well, for
/// rules whose trigger alone is common (`://`, `curl`).
struct Rule {
    triggers: &'static [&'static str],
    requires: &'static [&'static str],
    pattern: &'static str,
    mask: Mask,
}

// Value shape, appended to a prefix that opens capture group 1 (the kept
// context): one whole shell word, captured as `v`. A word is a run of
// segments with no space between them — unquoted text with `\x` escapes,
// '…', or "…" honouring `\"` — so `"abc"defgh`, `abc"d e"f` and `'a'"b"` are
// each ONE password to the shell and are masked whole. A quote may be left
// unterminated (a truncated line still gets masked). After a quoted segment
// the word does not continue into `,` `}` `]` `:`, so a JSON string value
// ends at its closing quote. Shell operators (`;|&<>()`) always end a word.
// How the matched word is masked — quotes kept or not, references and short
// words left alone — is decided in `mask_shell_word`, which keeps the regex
// itself small: compiling regexes dominates a redaction pass.
macro_rules! shell_value {
    () => {
        r#")(?P<v>(?:(?:\\.|[^\s"'\;|&<>()])+|'[^'\n]*'?|"(?:\\.|[^"\\\n])*"?)(?:'[^'\n]*'?|"(?:\\.|[^"\\\n])*"?|(?:\\.|[^\s"'\;|&<>(),}\]:])+)*)"#
    };
}
// The same shell word without the JSON stops after a quoted segment, for
// arguments that are never JSON: `curl -u "alice":pw` continues past `:`.
macro_rules! shell_word {
    () => {
        r#")(?P<v>(?:(?:\\.|[^\s"'\\;|&<>()])+|'[^'\n]*'?|"(?:\\.|[^"\\\n])*"?)(?:'[^'\n]*'?|"(?:\\.|[^"\\\n])*"?|(?:\\.|[^\s"'\\;|&<>()])+)*)"#
    };
}
macro_rules! env_prefix {
    () => {
        r#"\b([A-Z][A-Z0-9_]*(?:TOKEN|SECRET|PASSWORD|PASSWD|API_KEY|APIKEY|ACCESS_KEY|SECRET_KEY|PRIVATE_KEY|CREDENTIALS?)="#
    };
}
macro_rules! flag_prefix {
    () => {
        r"(?i)(--(?:password|passwd|token|api[-_]?key|secret|client[-_]secret|access[-_]token|auth[-_]token)\s+"
    };
}
macro_rules! mysql_prefix {
    () => {
        r"(\b(?:mysql|mysqldump|mysqladmin|mariadb)\b[^\n|;&]*?\s-p"
    };
}
macro_rules! curl_user_prefix {
    () => {
        r#"(\bcurl\b[^\n|;&]*?\s(?:-u|--user)(?:=|\s+)?"#
    };
}
macro_rules! assignment_prefix {
    () => {
        r#"(?i)\b((?:api[_-]?key|access[_-]?token|auth[_-]?token|client[_-]?secret|password|passwd|pwd|secret|token)["']?\s*[=:]\s*"#
    };
}

const RULES: &[Rule] = &[
    // Private key blocks first (multi-line; would otherwise be shredded
    // piecemeal by the line-level rules below).
    Rule {
        requires: &[],
        triggers: &["private key"],
        pattern: r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
        mask: Mask::Full,
    },
    // GitHub: classic + fine-grained PATs, OAuth, app tokens
    Rule {
        requires: &[],
        triggers: &["ghp_", "gho_", "ghu_", "ghs_", "ghr_"],
        pattern: r"\bgh[pousr]_[A-Za-z0-9]{20,}\b",
        mask: Mask::Full,
    },
    Rule {
        requires: &[],
        triggers: &["github_pat_"],
        pattern: r"\bgithub_pat_[A-Za-z0-9_]{22,}\b",
        mask: Mask::Full,
    },
    // GitLab: PAT, pipeline trigger, deploy, runner tokens
    Rule {
        requires: &[],
        triggers: &["glpat-", "glptt-", "gldt-", "glrt-"],
        pattern: r"\bgl(?:pat|ptt|dt|rt)-[A-Za-z0-9_-]{20,}\b",
        mask: Mask::Full,
    },
    // Hugging Face, Docker Hub PAT, DigitalOcean, PyPI upload tokens
    Rule {
        requires: &[],
        triggers: &["hf_"],
        pattern: r"\bhf_[A-Za-z0-9]{30,}\b",
        mask: Mask::Full,
    },
    Rule {
        requires: &[],
        triggers: &["dckr_pat_"],
        pattern: r"\bdckr_pat_[A-Za-z0-9_-]{20,}\b",
        mask: Mask::Full,
    },
    Rule {
        requires: &[],
        triggers: &["_v1_"],
        pattern: r"\bdo[po]_v1_[a-f0-9]{64}\b",
        mask: Mask::Full,
    },
    Rule {
        requires: &[],
        triggers: &["pypi-"],
        pattern: r"\bpypi-[A-Za-z0-9_-]{50,}",
        mask: Mask::Full,
    },
    // SendGrid (SG.<22>.<43>) and Google OAuth access tokens
    Rule {
        requires: &[],
        triggers: &["sg."],
        pattern: r"\bSG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}\b",
        mask: Mask::Full,
    },
    Rule {
        requires: &[],
        triggers: &["ya29."],
        pattern: r"\bya29\.[0-9A-Za-z_-]{20,}",
        mask: Mask::Full,
    },
    // npm granular / automation tokens
    Rule {
        requires: &[],
        triggers: &["npm_"],
        pattern: r"\bnpm_[A-Za-z0-9]{30,}\b",
        mask: Mask::Full,
    },
    // AWS access key IDs (long-term and STS)
    Rule {
        requires: &[],
        triggers: &["akia", "asia"],
        pattern: r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
        mask: Mask::Full,
    },
    // Slack tokens
    Rule {
        requires: &[],
        triggers: &["xox"],
        pattern: r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b",
        mask: Mask::Full,
    },
    // OpenAI / Anthropic style (sk-..., sk-ant-...) and Stripe
    Rule {
        requires: &[],
        triggers: &["sk-"],
        pattern: r"\bsk-[A-Za-z0-9_-]{20,}\b",
        mask: Mask::Full,
    },
    Rule {
        requires: &[],
        triggers: &["sk_", "rk_"],
        pattern: r"\b[sr]k_(?:live|test)_[A-Za-z0-9]{16,}\b",
        mask: Mask::Full,
    },
    // Google API keys
    Rule {
        requires: &[],
        triggers: &["aiza"],
        pattern: r"\bAIza[0-9A-Za-z_-]{35}\b",
        mask: Mask::Full,
    },
    // JWTs (three base64url segments, first one starting with {"...)
    Rule {
        requires: &[],
        triggers: &["eyj"],
        pattern: r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
        mask: Mask::Full,
    },
    // Authorization headers: keep the scheme, mask the credential
    Rule {
        requires: &[],
        triggers: &["authorization"],
        pattern: r"(?i)\b(authorization\s*:\s*(?:bearer|basic|token)\s+)\S+",
        mask: Mask::KeepPrefix,
    },
    // Cookie headers carry session credentials: mask the whole value, which
    // may itself contain quotes (`session="…"`). Inside a quoted shell
    // argument, mask up to that argument's closing quote; otherwise (raw
    // output, an unquoted header) to the end of the line.
    Rule {
        requires: &[],
        triggers: &["cookie"],
        pattern: r"(?i)('(?:set-)?cookie\s*:\s*)[^'\n]+",
        mask: Mask::KeepPrefix,
    },
    Rule {
        requires: &[],
        triggers: &["cookie"],
        pattern: r#"(?i)("(?:set-)?cookie\s*:\s*)(?:\\.|[^"\\\n])+"#,
        mask: Mask::KeepPrefix,
    },
    Rule {
        requires: &[],
        triggers: &["cookie"],
        pattern: r#"(?im)((?:^|[^'"\w-])(?:set-)?cookie\s*:\s*)[^\n]+"#,
        mask: Mask::KeepPrefix,
    },
    // Webhook URLs are bearer credentials in their path
    Rule {
        requires: &[],
        triggers: &["hooks.slack.com"],
        pattern: r"(hooks\.slack\.com/services/[A-Z0-9]+/[A-Z0-9]+/)[A-Za-z0-9]+",
        mask: Mask::KeepPrefix,
    },
    Rule {
        requires: &[],
        triggers: &["discord"],
        pattern: r"(discord(?:app)?\.com/api/webhooks/\d+/)[A-Za-z0-9_-]+",
        mask: Mask::KeepPrefix,
    },
    // Password in URL userinfo (scheme://user:PASSWORD@host). A colon inside
    // the userinfo and an `@` before any `/` are both required, so
    // `host:8080/path` and `ssh://git@host:22` stay untouched.
    Rule {
        requires: &["@"],
        triggers: &["://"],
        pattern: r"(?i)\b([a-z][a-z0-9+.-]*://[^/\s:@]*:)[^/\s@]+@",
        mask: Mask::KeepPrefixThenAt,
    },
    // Signed-URL credentials (AWS presigned, Azure SAS)
    Rule {
        requires: &[],
        triggers: &["sig=", "signature=", "x-amz-"],
        pattern: r#"(?i)([?&](?:x-amz-signature|x-amz-security-token|x-amz-credential|sig|signature)=)[^&\s"']+"#,
        mask: Mask::KeepPrefix,
    },
    // Environment-style assignments whose NAME ends in a secret word:
    // GITHUB_TOKEN=…, OPENAI_API_KEY=…, AWS_SECRET_ACCESS_KEY=…. Upper-case
    // names only (the env convention) and the word must end the name, so
    // MAX_TOKENS=4096 / TOKEN_LIMIT=8 stay intact.
    Rule {
        requires: &[],
        triggers: &[
            "token=",
            "secret=",
            "password=",
            "passwd=",
            "_key=",
            "apikey=",
            "credential",
        ],
        pattern: concat!(env_prefix!(), shell_value!()),
        mask: Mask::ShellWord { allow_dash: true },
    },
    // Secret-named flags with the value as the next word: `--password
    // hunter2`, `--token "a b"`. `--token-file`, `--password-stdin` and a
    // following `-flag` are not values.
    Rule {
        requires: &[],
        triggers: &[
            "--password",
            "--passwd",
            "--token",
            "--api",
            "--secret",
            "--client",
            "--access",
            "--auth",
        ],
        pattern: concat!(flag_prefix!(), shell_value!()),
        mask: Mask::ShellWord { allow_dash: false },
    },
    // mysql-family `-pPASSWORD` (attached). Case-sensitive: `-P3306` is the port.
    Rule {
        requires: &["-p"],
        triggers: &["mysql", "mariadb"],
        pattern: concat!(mysql_prefix!(), shell_value!()),
        mask: Mask::ShellWord { allow_dash: true },
    },
    // `curl -u user:PASSWORD` (also `-uuser:…`, `--user=…`). The argument is
    // one shell word however it is quoted — `'alice:abc'defgh` is a single
    // credential — so it is captured whole and split in `mask_curl_user`.
    Rule {
        requires: &["-u"],
        triggers: &["curl"],
        pattern: concat!(curl_user_prefix!(), shell_word!()),
        mask: Mask::CurlUser,
    },
    // key=value / key: value assignments for well-known secret names. Leading
    // \b keeps compound words like `input_tokens:` out (no word boundary
    // after `_`), and the mandatory [=:] right after the name keeps plurals
    // like `tokens:` out.
    Rule {
        requires: &[],
        triggers: &["api", "token", "secret", "password", "passwd", "pwd"],
        pattern: concat!(assignment_prefix!(), shell_value!()),
        mask: Mask::ShellWord { allow_dash: true },
    },
];

/// The compiled form of `RULES[i]`, built the first time its trigger appears.
fn compiled(i: usize) -> &'static Regex {
    static COMPILED: OnceLock<Vec<OnceLock<Regex>>> = OnceLock::new();
    let slots = COMPILED.get_or_init(|| RULES.iter().map(|_| OnceLock::new()).collect());
    slots[i].get_or_init(|| Regex::new(RULES[i].pattern).expect("redaction pattern compiles"))
}

/// Mask one shell word holding a secret value.
///
/// - A reference — `$VAR`, `$(cmd)`, `"$VAR"` — is not a literal secret: kept.
/// - Without `allow_dash`, a word starting with `-` is the next flag: kept.
/// - A bare word under 4 characters (`password: ok`) or an empty `""` / `''`
///   holds nothing worth masking.
/// - A word that is exactly one quoted segment keeps its quotes
///   (`"[REDACTED]"`), so JSON strings and shell syntax stay intact.
/// - Anything else — concatenated segments, an unterminated quote — is
///   masked whole, since the shell reads it as a single value.
fn mask_shell_word(word: &str, allow_dash: bool) -> String {
    let is_reference = word.starts_with('$') || word.starts_with("\"$");
    let is_flag = !allow_dash && word.starts_with('-');
    let bare_and_short = !word.contains(['\'', '"']) && word.chars().count() < 4;
    let empty_quotes = word == "''" || word == "\"\"";
    if is_reference || is_flag || bare_and_short || empty_quotes {
        return word.to_string();
    }
    if let Some(quote) = single_quoted_segment(word) {
        return format!("{quote}{MASK}{quote}");
    }
    MASK.to_string()
}

/// Mask the password in a curl `user:password` shell word, keeping the user.
///
/// The split is at the first `:` outside an escape, wherever the quotes fall
/// (`alice:"a b"`, `'alice:abc'defgh`, `"alice":pw`). Everything after it is
/// masked; if the `:` sat inside a quoted segment, that quote is closed again
/// after the mask so the stored line stays balanced. No `:` means curl will
/// prompt for the password, and a `$` reference is not a literal: both kept.
fn mask_curl_user(word: &str) -> String {
    let mut open: Option<char> = None;
    let mut chars = word.char_indices();
    while let Some((i, c)) = chars.next() {
        match (open, c) {
            (Some('"') | None, '\\') => {
                chars.next();
            }
            (None, '\'' | '"') => open = Some(c),
            (Some(q), _) if c == q => open = None,
            (_, ':') => {
                let (user, rest) = word.split_at(i + 1);
                if rest.starts_with('$') || rest.starts_with("\"$") {
                    return word.to_string();
                }
                return match (open, single_quoted_segment(rest)) {
                    (Some(q), _) => format!("{user}{MASK}{q}"),
                    // `alice:"my pass"` keeps the password's own quotes.
                    (None, Some(q)) => format!("{user}{q}{MASK}{q}"),
                    (None, None) => format!("{user}{MASK}"),
                };
            }
            _ => {}
        }
    }
    word.to_string()
}

/// The quote character if `word` is exactly one non-empty, terminated quoted
/// segment (`'…'` or `"…"` with `\"` escapes), else `None`.
fn single_quoted_segment(word: &str) -> Option<char> {
    let quote = word.chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let body = &word[1..];
    let mut chars = body.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '\\' && quote == '"' {
            chars.next();
            continue;
        }
        if c == quote {
            // The closing quote must end the word, and the segment must hold
            // something to mask.
            return (i + 1 == body.len() && i > 0).then_some(quote);
        }
    }
    None
}

/// Mask known secret formats in `input`. Returns `Cow::Borrowed` when
/// nothing matched, so the common case allocates nothing beyond the
/// lowercase trigger scan — and compiles no regex at all.
pub fn redact_secrets(input: &str) -> Cow<'_, str> {
    let lower = input.to_ascii_lowercase();
    let mut out = Cow::Borrowed(input);
    for (i, rule) in RULES.iter().enumerate() {
        if !rule.triggers.iter().any(|t| lower.contains(t))
            || !rule.requires.iter().all(|t| lower.contains(t))
        {
            continue;
        }
        let re = compiled(i);
        if !re.is_match(&out) {
            continue;
        }
        let replaced = match rule.mask {
            Mask::ShellWord { allow_dash } => re
                .replace_all(&out, |caps: &Captures| {
                    format!("{}{}", &caps[1], mask_shell_word(&caps["v"], allow_dash))
                })
                .into_owned(),
            Mask::CurlUser => re
                .replace_all(&out, |caps: &Captures| {
                    format!("{}{}", &caps[1], mask_curl_user(&caps["v"]))
                })
                .into_owned(),
            Mask::Full => re.replace_all(&out, MASK).into_owned(),
            Mask::KeepPrefix => re
                .replace_all(&out, format!("${{1}}{MASK}").as_str())
                .into_owned(),
            Mask::KeepPrefixThenAt => re
                .replace_all(&out, format!("${{1}}{MASK}@").as_str())
                .into_owned(),
        };
        out = Cow::Owned(replaced);
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
    fn test_more_service_tokens() {
        // Token-shaped fixtures are assembled at run time: written out whole,
        // GitHub push protection reads them as real credentials and blocks
        // the push. Each is `prefix` + filler of the length the rule needs.
        let token = |prefix: &str, filler: &str, len: usize| -> String {
            format!(
                "{prefix}{}",
                &filler.repeat(len.div_ceil(filler.len()))[..len]
            )
        };
        let alnum = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJ0123456789";
        let hex = "0123456789abcdef";
        let hf = token("hf_", alnum, 34);
        let docker = token("dckr_pat_", alnum, 27);
        let digitalocean = token("dop_v1_", hex, 64);
        let pypi = token("pypi-", alnum, 56);
        let sendgrid = format!("SG.{}.{}", &alnum[..22], &alnum[..43]);
        let gitlab = token("glptt-", alnum, 20);
        let google = token("ya29.", alnum, 30);
        for (input, secret) in [
            (gitlab.clone(), gitlab.clone()),
            (format!("huggingface-cli login --token {hf}"), hf.clone()),
            (format!("docker login -p {docker}"), docker.clone()),
            (
                format!("doctl auth init -t {digitalocean}"),
                digitalocean.clone(),
            ),
            (format!("twine upload -p {pypi}"), pypi.clone()),
            (sendgrid.clone(), sendgrid.clone()),
            (format!("curl -H {google}"), google.clone()),
        ] {
            assert_masked(&input, &secret);
        }
    }

    #[test]
    fn test_url_userinfo_password() {
        for (input, expected) in [
            (
                "git clone https://user:s3cr3tP4ss@github.com/o/r.git",
                "git clone https://user:[REDACTED]@github.com/o/r.git",
            ),
            (
                "psql postgres://admin:hunter2pass@db:5432/app",
                "psql postgres://admin:[REDACTED]@db:5432/app",
            ),
            (
                "redis-cli -u redis://:Sup3rSecret@localhost:6379",
                "redis-cli -u redis://:[REDACTED]@localhost:6379",
            ),
        ] {
            assert_eq!(redact_secrets(input), expected);
        }
    }

    #[test]
    fn test_env_style_assignments() {
        for (input, expected) in [
            (
                "GITHUB_TOKEN=abc123def456 gh pr list",
                "GITHUB_TOKEN=[REDACTED] gh pr list",
            ),
            (
                "export OPENAI_API_KEY=abcdefghijklmnop",
                "export OPENAI_API_KEY=[REDACTED]",
            ),
            (
                "export AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCY",
                "export AWS_SECRET_ACCESS_KEY=[REDACTED]",
            ),
            (
                "DATABASE_PASSWORD=\"hunter22\" app",
                "DATABASE_PASSWORD=\"[REDACTED]\" app",
            ),
        ] {
            assert_eq!(redact_secrets(input), expected);
        }
    }

    #[test]
    fn test_secret_flag_with_separate_value() {
        assert_eq!(
            redact_secrets("mysql --password hunter22 db"),
            "mysql --password [REDACTED] db"
        );
        assert_eq!(
            redact_secrets("vault login --token s.abcdef12"),
            "vault login --token [REDACTED]"
        );
        assert_eq!(
            redact_secrets("mysql -u root -phunter22 db"),
            "mysql -u root -p[REDACTED] db"
        );
        assert_eq!(
            redact_secrets("curl -u alice:wonderland https://api.example.com"),
            "curl -u alice:[REDACTED] https://api.example.com"
        );
    }

    #[test]
    fn test_cookies_webhooks_and_signed_urls() {
        assert_eq!(
            redact_secrets("curl -H 'Cookie: session=abcdef0123456789'"),
            "curl -H 'Cookie: [REDACTED]'"
        );
        assert_eq!(
            redact_secrets(
                "curl https://hooks.slack.com/services/T0000/B0000/XXXXXXXXXXXXXXXXXXXX"
            ),
            "curl https://hooks.slack.com/services/T0000/B0000/[REDACTED]"
        );
        assert_eq!(
            redact_secrets("https://discord.com/api/webhooks/123456789012345678/abcdefghijk"),
            "https://discord.com/api/webhooks/123456789012345678/[REDACTED]"
        );
        assert_eq!(
            redact_secrets(
                "https://b.s3.amazonaws.com/k?X-Amz-Credential=AKIDX&X-Amz-Signature=abcdef01"
            ),
            "https://b.s3.amazonaws.com/k?X-Amz-Credential=[REDACTED]&X-Amz-Signature=[REDACTED]"
        );
    }

    #[test]
    fn test_cookie_value_with_quotes_is_masked_whole() {
        for (input, expected) in [
            (r#"Cookie: session="secret""#, "Cookie: [REDACTED]"),
            (
                r#"curl -H 'Cookie: session="secret"; theme=dark' https://x"#,
                "curl -H 'Cookie: [REDACTED]' https://x",
            ),
            (
                r#"curl -H "Cookie: session=\"secret\"" https://x"#,
                r#"curl -H "Cookie: [REDACTED]" https://x"#,
            ),
            (
                "Set-Cookie: id=abc123; Path=/; HttpOnly\nnext line",
                "Set-Cookie: [REDACTED]\nnext line",
            ),
        ] {
            assert_eq!(redact_secrets(input), expected);
        }
    }

    #[test]
    fn test_quoted_values_with_spaces_are_masked_whole() {
        for (input, expected) in [
            (
                r#"export DATABASE_PASSWORD="my secret pass""#,
                r#"export DATABASE_PASSWORD="[REDACTED]""#,
            ),
            (
                r#"export DATABASE_PASSWORD=" leading""#,
                r#"export DATABASE_PASSWORD="[REDACTED]""#,
            ),
            ("export API_TOKEN='a b c'", "export API_TOKEN='[REDACTED]'"),
            (
                r#"mysql --password "my pass" db"#,
                r#"mysql --password "[REDACTED]" db"#,
            ),
            (
                "mysql --password ' leading' db",
                "mysql --password '[REDACTED]' db",
            ),
            (
                r"cli --password my\ pass next",
                "cli --password [REDACTED] next",
            ),
            (
                "mysql -u root -p'my pass' db",
                "mysql -u root -p'[REDACTED]' db",
            ),
            (
                r#"mysql -u root -p"my pass" db"#,
                r#"mysql -u root -p"[REDACTED]" db"#,
            ),
            (
                "curl -u 'alice:my pass' https://x",
                "curl -u 'alice:[REDACTED]' https://x",
            ),
            (
                r#"curl -u "alice:my pass" https://x"#,
                r#"curl -u "alice:[REDACTED]" https://x"#,
            ),
            (
                r#"curl -u alice:"my pass" https://x"#,
                r#"curl -u alice:"[REDACTED]" https://x"#,
            ),
            (
                r#"{"password": "two words"}"#,
                r#"{"password": "[REDACTED]"}"#,
            ),
        ] {
            assert_eq!(redact_secrets(input), expected, "{input}");
        }
        // A double-quoted `$…` is an expansion, not a literal secret.
        let input = r#"GITHUB_TOKEN="$GH_TOKEN" gh pr list"#;
        assert_eq!(redact_secrets(input), input);
    }

    #[test]
    fn test_values_starting_with_dash() {
        for (input, expected) in [
            (
                "DATABASE_PASSWORD=-syntheticSecret9",
                "DATABASE_PASSWORD=[REDACTED]",
            ),
            (
                "curl -u alice:-syntheticSecret9 https://x",
                "curl -u alice:[REDACTED] https://x",
            ),
            (
                "mysql -u root -p-syntheticSecret9 db",
                "mysql -u root -p[REDACTED] db",
            ),
            ("password=-syntheticSecret9", "password=[REDACTED]"),
        ] {
            assert_eq!(redact_secrets(input), expected, "{input}");
        }
        // After a flag, a leading `-` is the next flag, not a value.
        for input in ["gh auth login --token -", "cli --password --verbose run"] {
            assert_eq!(redact_secrets(input), input);
        }
    }

    #[test]
    fn test_concatenated_shell_words_are_masked_whole() {
        for (input, expected) in [
            (
                r#"--password "abc"defgh next"#,
                "--password [REDACTED] next",
            ),
            (
                r#"--password abc"def gh"ij next"#,
                "--password [REDACTED] next",
            ),
            (r#"API_TOKEN='abc'"def" cmd"#, "API_TOKEN=[REDACTED] cmd"),
            (r#"mysql -p'my'pass db"#, "mysql -p[REDACTED] db"),
            (
                r#"curl -u alice:"a b"c https://x"#,
                "curl -u alice:[REDACTED] https://x",
            ),
            // an unterminated quote (e.g. a truncated line) is still masked
            (
                r#"export DB_PASSWORD="unterminated secret"#,
                "export DB_PASSWORD=[REDACTED]",
            ),
            // a shell operator ends the word
            (
                "API_TOKEN=abcdef;echo done",
                "API_TOKEN=[REDACTED];echo done",
            ),
        ] {
            assert_eq!(redact_secrets(input), expected, "{input}");
        }
    }

    #[test]
    fn test_single_quoted_values_keep_their_quotes() {
        for (input, expected) in [
            (
                r#"{"password": "two words"}"#,
                r#"{"password": "[REDACTED]"}"#,
            ),
            (
                r#"{"token": "abcd","x":1}"#,
                r#"{"token": "[REDACTED]","x":1}"#,
            ),
            ("export API_TOKEN='a b c'", "export API_TOKEN='[REDACTED]'"),
        ] {
            assert_eq!(redact_secrets(input), expected, "{input}");
        }
        for input in [
            r#"DB_PASSWORD="" cmd"#,
            "DB_PASSWORD='' cmd",
            r#"GITHUB_TOKEN="$GH_TOKEN" gh"#,
        ] {
            assert_eq!(redact_secrets(input), input);
        }
    }

    #[test]
    fn test_curl_user_argument_is_one_shell_word() {
        for (input, expected) in [
            (
                "curl -u 'alice:abc'defgh https://x",
                "curl -u 'alice:[REDACTED]' https://x",
            ),
            (
                r#"curl -u "alice:abc"defgh https://x"#,
                r#"curl -u "alice:[REDACTED]" https://x"#,
            ),
            (
                "curl -u 'alice:my pass' https://x",
                "curl -u 'alice:[REDACTED]' https://x",
            ),
            (
                r#"curl -u alice:"a b"c https://x"#,
                "curl -u alice:[REDACTED] https://x",
            ),
            (
                r#"curl -u "alice":pw1234 https://x"#,
                r#"curl -u "alice":[REDACTED] https://x"#,
            ),
            (
                "curl -ualice:pw1234 https://x",
                "curl -ualice:[REDACTED] https://x",
            ),
            (
                "curl --user=alice:pw1234 https://x",
                "curl --user=alice:[REDACTED] https://x",
            ),
            (
                "curl -u alice:-dash9 https://x",
                "curl -u alice:[REDACTED] https://x",
            ),
        ] {
            assert_eq!(redact_secrets(input), expected, "{input}");
        }
        // No password (curl prompts), a reference, or `--user-agent`: kept.
        for input in [
            "curl -u alice https://x",
            "curl -u alice:$PASS https://x",
            "curl --user-agent 'a:b' https://x",
        ] {
            assert_eq!(redact_secrets(input), input);
        }
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
            // URLs without a userinfo password
            "git clone git@github.com:o/r.git",
            "git clone ssh://git@host.example.com:22/o/r.git",
            "curl http://localhost:8080/path?email=a@b.com",
            "curl -s https://api.github.com/repos/o/r",
            // env names where the secret word does not end the name, and
            // values that are references rather than secrets
            "export MAX_TOKENS=4096",
            "TOKEN_LIMIT=8 cargo test",
            "SKIP_AUTH=true npm test",
            "GITHUB_TOKEN=$GH_TOKEN gh pr list",
            "GITHUB_TOKEN=$(gh auth token) gh api user",
            // secret-named flags that take no inline secret
            "docker login --password-stdin",
            "npm publish --token-file ~/.npmrc",
            "gh auth login --token -",
            // -p / -P / -u that are not passwords
            "cargo test -p bushido",
            "docker run -p 8080:80 nginx",
            "mysql -h db -P3306 -u root",
            "curl -u alice https://api.example.com",
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
