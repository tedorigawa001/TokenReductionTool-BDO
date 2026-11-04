#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Arg,
    Operator,
    Pipe,
    Redirect,
    Shellism,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedToken {
    pub kind: TokenKind,
    pub value: String,
    pub offset: usize,
}

pub fn tokenize(input: &str) -> Vec<ParsedToken> {
    tokenize_inner(input, false)
}

fn tokenize_inner(input: &str, emit_newline: bool) -> Vec<ParsedToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_start: usize = 0;
    let mut byte_pos: usize = 0;
    let mut chars = input.chars().peekable();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    // The previous character, for recognizing a comment: bash starts one at
    // a `#` that begins a word, i.e. after whitespace, an operator or the
    // start of input (`foo#bar`, `$#` and `${#x}` are not comments).
    let mut prev: Option<char> = None;

    while let Some(c) = chars.next() {
        let char_len = c.len_utf8();
        let at_word_start = current.is_empty() && starts_comment_after(prev);
        prev = Some(c);

        if escaped {
            current.push('\\');
            current.push(c);
            byte_pos += char_len;
            escaped = false;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escaped = true;
            if current.is_empty() {
                current_start = byte_pos;
            }
            byte_pos += char_len;
            continue;
        }

        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            current.push(c);
            byte_pos += char_len;
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
            if current.is_empty() {
                current_start = byte_pos;
            }
            current.push(c);
            byte_pos += char_len;
            continue;
        }

        // A comment runs to the end of the line, and nothing in it — a quote
        // in `# it's` above all — may open a construct that hides the next
        // line's command from the permission check.
        if c == '#' && at_word_start {
            byte_pos += char_len;
            while let Some(&nc) = chars.peek() {
                if nc == '\n' {
                    break;
                }
                chars.next();
                byte_pos += nc.len_utf8();
            }
            prev = Some(' ');
            current_start = byte_pos;
            continue;
        }

        match c {
            '$' => {
                flush_arg(&mut tokens, &mut current, current_start);
                let start = byte_pos;
                byte_pos += char_len;
                if chars
                    .peek()
                    .is_some_and(|&nc| nc.is_ascii_alphabetic() || nc == '_')
                {
                    let mut name = String::from("$");
                    while let Some(&nc) = chars.peek() {
                        if !nc.is_ascii_alphanumeric() && nc != '_' {
                            break;
                        }
                        chars.next();
                        byte_pos += nc.len_utf8();
                        name.push(nc);
                    }
                    tokens.push(ParsedToken {
                        kind: TokenKind::Arg,
                        value: name,
                        offset: start,
                    });
                } else {
                    tokens.push(ParsedToken {
                        kind: TokenKind::Shellism,
                        value: "$".into(),
                        offset: start,
                    });
                }
                current_start = byte_pos;
            }
            '*' | '?' | '`' | '(' | ')' | '{' | '}' | '!' => {
                flush_arg(&mut tokens, &mut current, current_start);
                tokens.push(ParsedToken {
                    kind: TokenKind::Shellism,
                    value: c.to_string(),
                    offset: byte_pos,
                });
                byte_pos += char_len;
                current_start = byte_pos;
            }
            '|' => {
                flush_arg(&mut tokens, &mut current, current_start);
                let start = byte_pos;
                byte_pos += char_len;
                if chars.peek() == Some(&'|') {
                    chars.next();
                    byte_pos += 1;
                    tokens.push(ParsedToken {
                        kind: TokenKind::Operator,
                        value: "||".into(),
                        offset: start,
                    });
                } else {
                    tokens.push(ParsedToken {
                        kind: TokenKind::Pipe,
                        value: "|".into(),
                        offset: start,
                    });
                }
                current_start = byte_pos;
            }
            ';' => {
                flush_arg(&mut tokens, &mut current, current_start);
                tokens.push(ParsedToken {
                    kind: TokenKind::Operator,
                    value: ";".into(),
                    offset: byte_pos,
                });
                byte_pos += char_len;
                current_start = byte_pos;
            }
            '&' => {
                flush_arg(&mut tokens, &mut current, current_start);
                let start = byte_pos;
                byte_pos += char_len;
                if chars.peek() == Some(&'&') {
                    chars.next();
                    byte_pos += 1;
                    tokens.push(ParsedToken {
                        kind: TokenKind::Operator,
                        value: "&&".into(),
                        offset: start,
                    });
                } else if chars.peek() == Some(&'>') {
                    chars.next();
                    byte_pos += 1;
                    let mut val = String::from("&>");
                    if chars.peek() == Some(&'>') {
                        chars.next();
                        byte_pos += 1;
                        val.push('>');
                    }
                    tokens.push(ParsedToken {
                        kind: TokenKind::Redirect,
                        value: val,
                        offset: start,
                    });
                } else {
                    tokens.push(ParsedToken {
                        kind: TokenKind::Shellism,
                        value: "&".into(),
                        offset: start,
                    });
                }
                current_start = byte_pos;
            }
            '>' => {
                let fd_prefix =
                    if !current.is_empty() && current.chars().all(|ch| ch.is_ascii_digit()) {
                        Some(std::mem::take(&mut current))
                    } else {
                        flush_arg(&mut tokens, &mut current, current_start);
                        None
                    };
                let redir_start = if fd_prefix.is_some() {
                    current_start
                } else {
                    byte_pos
                };
                let mut val = fd_prefix.unwrap_or_default();
                val.push('>');
                byte_pos += char_len;
                if chars.peek() == Some(&'>') {
                    chars.next();
                    byte_pos += 1;
                    val.push('>');
                }
                if chars.peek() == Some(&'&') {
                    chars.next();
                    byte_pos += 1;
                    val.push('&');
                    let mut fd = String::new();
                    while let Some(&nc) = chars.peek() {
                        if !nc.is_ascii_digit() && nc != '-' {
                            break;
                        }
                        chars.next();
                        fd.push(nc);
                        byte_pos += nc.len_utf8();
                    }
                    // `2>&1` duplicates a descriptor, but `2>&1x` writes to a
                    // file named `1x`: only a whole word of digits (or `-`) is
                    // an fd. Otherwise those characters start the target word.
                    let word_continues = chars.peek().is_some_and(|&nc| {
                        !is_shell_blank(nc)
                            && !matches!(nc, ';' | '&' | '|' | '(' | ')' | '<' | '>')
                    });
                    if word_continues {
                        current = fd;
                        tokens.push(ParsedToken {
                            kind: TokenKind::Redirect,
                            value: val,
                            offset: redir_start,
                        });
                        current_start = byte_pos - current.len();
                        continue;
                    }
                    val.push_str(&fd);
                }
                tokens.push(ParsedToken {
                    kind: TokenKind::Redirect,
                    value: val,
                    offset: redir_start,
                });
                current_start = byte_pos;
            }
            '<' => {
                flush_arg(&mut tokens, &mut current, current_start);
                let start = byte_pos;
                let mut val = String::from("<");
                byte_pos += char_len;
                if chars.peek() == Some(&'<') {
                    chars.next();
                    byte_pos += 1;
                    val.push('<');
                }
                tokens.push(ParsedToken {
                    kind: TokenKind::Redirect,
                    value: val,
                    offset: start,
                });
                current_start = byte_pos;
            }
            '\n' if emit_newline => {
                flush_arg(&mut tokens, &mut current, current_start);
                tokens.push(ParsedToken {
                    kind: TokenKind::Operator,
                    value: "\n".into(),
                    offset: byte_pos,
                });
                byte_pos += char_len;
                current_start = byte_pos;
            }
            // Only the shell's blanks separate words. `\r`, `\v`, `\f`, NBSP and
            // other Unicode spaces are ordinary word characters to bash.
            c if is_shell_blank(c) => {
                flush_arg(&mut tokens, &mut current, current_start);
                byte_pos += c.len_utf8();
                current_start = byte_pos;
            }
            _ => {
                if current.is_empty() {
                    current_start = byte_pos;
                }
                current.push(c);
                byte_pos += char_len;
            }
        }
    }

    if escaped {
        current.push('\\');
    }
    flush_arg(&mut tokens, &mut current, current_start);
    tokens
}

/// Remove line continuations the way bash does before it reads words:
/// a backslash-newline outside single quotes (inside double quotes too)
/// disappears, so `git push --for\<newline>ce` is `git push --force`. Single
/// quotes keep it, `\\` before a newline is an escaped backslash followed by a
/// real newline, and a comment keeps it too: a comment ends at its newline, so
/// `# note \<newline>rm` still runs `rm` on the next line.
pub fn remove_line_continuations(cmd: &str) -> std::borrow::Cow<'_, str> {
    if !cmd.contains("\\\n") {
        return std::borrow::Cow::Borrowed(cmd);
    }
    let mut out = String::with_capacity(cmd.len());
    let mut chars = cmd.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut prev: Option<char> = None;
    while let Some(c) = chars.next() {
        if in_single {
            if c == '\'' {
                in_single = false;
            }
            out.push(c);
        } else if c == '\\' {
            match chars.peek() {
                Some('\n') => {
                    chars.next();
                    continue; // continuation: both characters vanish
                }
                Some(&next) => {
                    chars.next();
                    out.push(c);
                    out.push(next);
                    prev = Some(next);
                    continue;
                }
                None => out.push(c),
            }
        } else if c == '"' {
            in_double = !in_double;
            out.push(c);
        } else if c == '\'' && !in_double {
            in_single = true;
            out.push(c);
        } else if c == '#' && !in_double && starts_comment_after(prev) {
            out.push(c);
            while let Some(&nc) = chars.peek() {
                if nc == '\n' {
                    break;
                }
                out.push(nc);
                chars.next();
            }
            prev = Some(' ');
            continue;
        } else {
            out.push(c);
        }
        prev = Some(c);
    }
    std::borrow::Cow::Owned(out)
}

/// The characters that separate words in a shell. Deliberately not
/// `char::is_whitespace`: treating NBSP as a blank made `x\u{a0}# y; cmd` look
/// like a comment hiding `cmd`, while bash reads a word and runs `cmd`.
pub(crate) fn is_shell_blank(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n')
}

/// True when a `#` following `prev` starts a comment (see `tokenize_inner`).
fn starts_comment_after(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(p) => is_shell_blank(p) || matches!(p, ';' | '&' | '|' | '(' | ')'),
    }
}

fn flush_arg(tokens: &mut Vec<ParsedToken>, current: &mut String, offset: usize) {
    if !current.is_empty() {
        tokens.push(ParsedToken {
            kind: TokenKind::Arg,
            value: std::mem::take(current),
            offset,
        });
    }
}

/// True for constructs the permission gate can't decompose, so they must never
/// be auto-allowed: ANSI-C quoting, command/process substitution, or a real file-target redirect
/// (fd-dup like `2>&1` and `/dev/null` are exempt). Separators and subshells are
/// handled by [`split_for_permissions`], not flagged here.
pub fn contains_unattestable_construct(cmd: &str) -> bool {
    if contains_unsupported_shell_construct(cmd) {
        return true;
    }
    let tokens = tokenize(cmd);
    tokens
        .iter()
        .enumerate()
        .any(|(i, tok)| tok.kind == TokenKind::Redirect && redirect_has_file_target(&tokens, i))
}

/// Quote-aware: bash runs backtick/`$(...)` unquoted and inside double quotes,
/// but treats single-quoted text literally; `<(`/`>(` is unquoted-only.
/// ANSI-C `$'...'` quoting is not modeled by the tokenizer: its escaped quotes
/// can hide real shell separators. Defer the entire command before parsing it.
fn contains_unsupported_shell_construct(cmd: &str) -> bool {
    let bytes = cmd.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if !in_single => {
                i += 2;
                continue;
            }
            // Skip a comment to the end of its line: a quote inside it must
            // not toggle the quote state for the lines that follow.
            b'#' if !in_single
                && !in_double
                && starts_comment_after(i.checked_sub(1).map(|j| bytes[j] as char)) =>
            {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'$' if !in_single
                && !in_double
                && next_after_line_continuations(bytes, i + 1) == Some(b'\'') =>
            {
                return true
            }
            b'`' if !in_single => return true,
            // `$\<newline>(` is `$(` once bash drops the line continuation,
            // inside double quotes too.
            b'$' if !in_single && next_after_line_continuations(bytes, i + 1) == Some(b'(') => {
                return true
            }
            b'<' | b'>'
                if !in_single
                    && !in_double
                    && next_after_line_continuations(bytes, i + 1) == Some(b'(') =>
            {
                return true
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Bash removes backslash-newline pairs before recognizing an ANSI-C quote or a
/// `$(`/`<(`/`>(` substitution.
/// Inspect that logical next byte without changing quote/escape handling for
/// literal dollar signs or for the rest of the original command.
fn next_after_line_continuations(bytes: &[u8], mut pos: usize) -> Option<u8> {
    while bytes.get(pos) == Some(&b'\\') && bytes.get(pos + 1) == Some(&b'\n') {
        pos += 2;
    }
    bytes.get(pos).copied()
}

// `>&N`/`>&-` (and `N>&M`) is fd-dup/close; bare `>&` before a word is
// `>word 2>&1` — a file target.
fn redirect_has_file_target(tokens: &[ParsedToken], i: usize) -> bool {
    let value = &tokens[i].value;
    if let Some(pos) = value.find(">&") {
        let tail = &value[pos + 2..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit() || c == '-') {
            return false;
        }
    }
    match tokens.get(i + 1) {
        Some(next) if next.kind == TokenKind::Arg => next.value != "/dev/null",
        _ => true,
    }
}

/// Like [`split_on_operators`] but also breaks on newline, background `&`, and
/// subshell `( ... )`, and removes each redirect — with its file target —
/// while keeping the words after it: `git push 2>/dev/null --force` is checked
/// as `git push --force`. (Truncating at the first redirect used to drop the
/// `--force`, so a deny rule for it never matched.) Comments are dropped by the
/// tokenizer. Callers must still gate on [`contains_unattestable_construct`].
pub fn split_for_permissions(cmd: &str) -> Vec<String> {
    let trimmed = cmd.trim_matches(is_shell_blank);
    if trimmed.is_empty() {
        return vec![];
    }

    let tokens = tokenize_inner(trimmed, true);
    let mut results = Vec::new();
    // Original-text runs of the current segment. A run grows token by token
    // (so `*.rs` and the spacing inside it stay exactly as written); a removed
    // redirect starts a new run, and runs are joined with one space.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut cut = false;
    let mut skip_target = false;

    let flush = |runs: &mut Vec<(usize, usize)>, results: &mut Vec<String>| {
        if !runs.is_empty() {
            let parts: Vec<&str> = runs
                .iter()
                .map(|&(a, b)| trimmed[a..b].trim_matches(is_shell_blank))
                .collect();
            let segment = parts.join(" ");
            if !segment.is_empty() {
                results.push(segment);
            }
            runs.clear();
        }
    };

    for tok in &tokens {
        let is_boundary = match tok.kind {
            TokenKind::Operator | TokenKind::Pipe => true,
            TokenKind::Shellism => matches!(tok.value.as_str(), "&" | "(" | ")"),
            _ => false,
        };
        if is_boundary {
            flush(&mut runs, &mut results);
            cut = false;
            skip_target = false;
            continue;
        }
        if tok.kind == TokenKind::Redirect {
            // `>file`, `2> file`, `<in`, `&>log`, `<<EOF` take the next word
            // as their target; fd-dups (`2>&1`, `>&2`, `>&-`) take none.
            skip_target = redirect_takes_target(&tok.value);
            cut = true;
            continue;
        }
        if skip_target && tok.kind == TokenKind::Arg {
            skip_target = false;
            continue;
        }
        skip_target = false;
        let end = (tok.offset + tok.value.len()).min(trimmed.len());
        match runs.last_mut() {
            Some(run) if !cut => run.1 = end,
            _ => runs.push((tok.offset, end)),
        }
        cut = false;
    }
    flush(&mut runs, &mut results);
    results
}

/// A permission segment as the shell will read it, for matching deny and ask
/// rules: quotes and escapes removed, words joined by one space, so `"--force"`,
/// `--fo''rce`, `--forc\\e` and `push  --force` all read as written in a rule.
pub struct CanonicalSegment {
    pub text: String,
    pub first_word: String,
    /// A word holds something only the shell can resolve: `$…`, a glob, a
    /// brace. Its real value is unknown, so a rule cannot be ruled out.
    pub has_expansion: bool,
}

pub fn canonicalize_segment(segment: &str) -> CanonicalSegment {
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut has_expansion = false;
    let mut chars = segment.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if is_shell_blank(c) => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\\' => {
                in_word = true;
                if let Some(next) = chars.next() {
                    if next != '\n' {
                        word.push(next);
                    }
                }
            }
            '\'' => {
                in_word = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    word.push(q);
                }
            }
            '"' => {
                in_word = true;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('\n') | None => {}
                            Some(e @ ('$' | '`' | '"' | '\\')) => word.push(e),
                            Some(e) => {
                                word.push('\\');
                                word.push(e);
                            }
                        },
                        '$' | '`' => {
                            has_expansion = true;
                            word.push(q);
                        }
                        _ => word.push(q),
                    }
                }
            }
            // `$"…"` is a locale string: the `$` vanishes.
            '$' if chars.peek() == Some(&'"') => {}
            '$' | '*' | '?' | '[' | '{' | '`' => {
                has_expansion = true;
                in_word = true;
                word.push(c);
            }
            _ => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    CanonicalSegment {
        first_word: words.first().cloned().unwrap_or_default(),
        text: words.join(" "),
        has_expansion,
    }
}

/// Whether a redirect operator consumes the following word as its target.
fn redirect_takes_target(op: &str) -> bool {
    match op.find(">&") {
        Some(pos) => op[pos + 2..].is_empty(),
        None => true,
    }
}

/// Split a shell command on operators (`&&`, `||`, `;`) and optionally pipes (`|`),
/// respecting quoted strings via the lexer.
///
/// When `stop_at_pipe` is true, returns only segments before the first `|`
/// (used by command rewriting — only the left side of a pipe gets rewritten).
/// When false, splits through pipes too (used by permission checking —
/// every segment must be validated).
pub fn split_on_operators(cmd: &str, stop_at_pipe: bool) -> Vec<&str> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    let tokens = tokenize(trimmed);
    let mut results = Vec::new();
    let mut seg_start: usize = 0;

    for tok in &tokens {
        match tok.kind {
            TokenKind::Operator => {
                let segment = trimmed[seg_start..tok.offset].trim();
                if !segment.is_empty() {
                    results.push(segment);
                }
                seg_start = tok.offset + tok.value.len();
            }
            TokenKind::Pipe => {
                let segment = trimmed[seg_start..tok.offset].trim();
                if !segment.is_empty() {
                    results.push(segment);
                }
                if stop_at_pipe {
                    return results;
                }
                seg_start = tok.offset + tok.value.len();
            }
            _ => {}
        }
    }

    let tail = trimmed[seg_start..].trim();
    if !tail.is_empty() {
        results.push(tail);
    }

    results
}

#[cfg(test)]
pub fn strip_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() >= 2
        && ((chars[0] == '"' && chars[chars.len() - 1] == '"')
            || (chars[0] == '\'' && chars[chars.len() - 1] == '\''))
    {
        return chars[1..chars.len() - 1].iter().collect();
    }
    s.to_string()
}

pub fn shell_split(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(c) = chars.next() {
        match c {
            '\\' if !in_single => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            ' ' | '\t' if !in_single && !in_double => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => {
                current.push(c);
            }
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_command() {
        let tokens = tokenize("git status");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].kind, TokenKind::Arg);
        assert_eq!(tokens[0].value, "git");
        assert_eq!(tokens[1].value, "status");
    }

    #[test]
    fn test_command_with_args() {
        let tokens = tokenize("git commit -m message");
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[0].value, "git");
        assert_eq!(tokens[1].value, "commit");
        assert_eq!(tokens[2].value, "-m");
        assert_eq!(tokens[3].value, "message");
    }

    #[test]
    fn test_quoted_operator_not_split() {
        let tokens = tokenize(r#"git commit -m "Fix && Bug""#);
        assert!(!tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::Operator) && t.value == "&&"));
        assert!(tokens.iter().any(|t| t.value.contains("Fix && Bug")));
    }

    #[test]
    fn test_single_quoted_string() {
        let tokens = tokenize("echo 'hello world'");
        assert!(tokens.iter().any(|t| t.value == "'hello world'"));
    }

    #[test]
    fn test_double_quoted_string() {
        let tokens = tokenize(r#"echo "hello world""#);
        assert!(tokens.iter().any(|t| t.value == "\"hello world\""));
    }

    #[test]
    fn test_empty_quoted_string() {
        let tokens = tokenize("echo \"\"");
        assert!(tokens.iter().any(|t| t.value == "\"\""));
    }

    #[test]
    fn test_nested_quotes() {
        let tokens = tokenize(r#"echo "outer 'inner' outer""#);
        assert!(tokens.iter().any(|t| t.value.contains("'inner'")));
    }

    #[test]
    fn test_escaped_space() {
        let tokens = tokenize("echo hello\\ world");
        assert!(tokens.iter().any(|t| t.value.contains("hello")));
    }

    #[test]
    fn test_backslash_in_single_quotes() {
        let tokens = tokenize(r#"echo 'hello\nworld'"#);
        assert!(tokens.iter().any(|t| t.value.contains(r"\n")));
    }

    #[test]
    fn test_escaped_quote_in_double() {
        let tokens = tokenize(r#"echo "hello\"world""#);
        assert!(tokens.iter().any(|t| t.value.contains("hello")));
    }

    #[test]
    fn test_empty_input() {
        assert!(tokenize("").is_empty());
    }

    #[test]
    fn test_whitespace_only() {
        assert!(tokenize("   ").is_empty());
    }

    #[test]
    fn test_unclosed_single_quote() {
        let tokens = tokenize("'unclosed");
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_unclosed_double_quote() {
        let tokens = tokenize("\"unclosed");
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_unicode_preservation() {
        let tokens = tokenize("echo \"héllo wörld\"");
        assert!(tokens.iter().any(|t| t.value.contains("héllo")));
    }

    #[test]
    fn test_multiple_spaces() {
        let tokens = tokenize("git   status");
        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn test_leading_trailing_spaces() {
        let tokens = tokenize("  git status  ");
        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn test_and_operator() {
        let tokens = tokenize("cmd1 && cmd2");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Operator && t.value == "&&"));
    }

    #[test]
    fn test_or_operator() {
        let tokens = tokenize("cmd1 || cmd2");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Operator && t.value == "||"));
    }

    #[test]
    fn test_semicolon() {
        let tokens = tokenize("cmd1 ; cmd2");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Operator && t.value == ";"));
    }

    #[test]
    fn test_multiple_and() {
        let tokens = tokenize("a && b && c");
        let ops: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Operator)
            .collect();
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn test_mixed_operators() {
        let tokens = tokenize("a && b || c");
        let ops: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Operator)
            .collect();
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn test_operator_at_start() {
        let tokens = tokenize("&& cmd");
        assert!(tokens.iter().any(|t| t.value == "&&"));
    }

    #[test]
    fn test_operator_at_end() {
        let tokens = tokenize("cmd &&");
        assert!(tokens.iter().any(|t| t.value == "&&"));
    }

    #[test]
    fn test_pipe_detection() {
        let tokens = tokenize("cat file | grep pattern");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Pipe));
    }

    #[test]
    fn test_quoted_pipe_not_pipe() {
        let tokens = tokenize("\"a|b\"");
        assert!(!tokens.iter().any(|t| t.kind == TokenKind::Pipe));
    }

    #[test]
    fn test_multiple_pipes() {
        let tokens = tokenize("a | b | c");
        let pipes: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Pipe)
            .collect();
        assert_eq!(pipes.len(), 2);
    }

    #[test]
    fn test_glob_detection() {
        let tokens = tokenize("ls *.rs");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_quoted_glob_not_shellism() {
        let tokens = tokenize("echo \"*.txt\"");
        assert!(!tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_simple_var_is_arg() {
        let tokens = tokenize("echo $HOME");
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == TokenKind::Arg && t.value == "$HOME"),
            "Simple $VAR must be Arg — shell expands at execution time"
        );
        assert!(
            !tokens.iter().any(|t| t.kind == TokenKind::Shellism),
            "No Shellism expected for simple $VAR"
        );
    }

    #[test]
    fn test_simple_var_enables_native_routing() {
        let tokens = tokenize("git log $BRANCH");
        assert!(
            !tokens.iter().any(|t| t.kind == TokenKind::Shellism),
            "git log $BRANCH must have no Shellism"
        );
    }

    #[test]
    fn test_dollar_subshell_stays_shellism() {
        let tokens = tokenize("echo $(date)");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_dollar_brace_stays_shellism() {
        let tokens = tokenize("echo ${HOME}");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_dollar_special_vars_stay_shellism() {
        for s in &["echo $?", "echo $$", "echo $!"] {
            let tokens = tokenize(s);
            assert!(
                tokens.iter().any(|t| t.kind == TokenKind::Shellism),
                "{} should produce Shellism",
                s
            );
        }
    }

    #[test]
    fn test_dollar_digit_stays_shellism() {
        let tokens = tokenize("echo $1");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_quoted_variable_not_shellism() {
        let tokens = tokenize("echo \"$HOME\"");
        assert!(!tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_backtick_substitution() {
        let tokens = tokenize("echo `date`");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_subshell_detection() {
        let tokens = tokenize("echo $(date)");
        let shellisms: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Shellism)
            .collect();
        assert!(!shellisms.is_empty());
    }

    #[test]
    fn test_brace_expansion() {
        let tokens = tokenize("echo {a,b}.txt");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Shellism));
    }

    #[test]
    fn test_escaped_glob() {
        let tokens = tokenize("echo \\*.txt");
        assert!(!tokens
            .iter()
            .any(|t| t.kind == TokenKind::Shellism && t.value == "*"));
    }

    #[test]
    fn test_redirect_out() {
        let tokens = tokenize("cmd > file");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Redirect));
    }

    #[test]
    fn test_redirect_append() {
        let tokens = tokenize("cmd >> file");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == ">>"));
    }

    #[test]
    fn test_redirect_in() {
        let tokens = tokenize("cmd < file");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Redirect));
    }

    #[test]
    fn test_redirect_stderr() {
        let tokens = tokenize("cmd 2> file");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value.starts_with("2>")));
    }

    #[test]
    fn test_redirect_stderr_no_space() {
        let tokens = tokenize("cmd 2>/dev/null");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "2>"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Arg && t.value == "/dev/null"));
    }

    #[test]
    fn test_redirect_dev_null() {
        let tokens = tokenize("cmd > /dev/null");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == ">"));
    }

    #[test]
    fn test_redirect_2_to_1_single_token() {
        let tokens = tokenize("cmd 2>&1");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[1].kind, TokenKind::Redirect);
        assert_eq!(tokens[1].value, "2>&1");
        assert!(!tokens
            .iter()
            .any(|t| t.kind == TokenKind::Shellism && t.value == "&"));
    }

    #[test]
    fn test_redirect_1_to_2_single_token() {
        let tokens = tokenize("cmd 1>&2");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "1>&2"));
    }

    #[test]
    fn test_redirect_fd_close() {
        let tokens = tokenize("cmd 2>&-");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "2>&-"));
    }

    #[test]
    fn test_redirect_shorthand_dup() {
        let tokens = tokenize("cmd >&2");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == ">&2"));
    }

    #[test]
    fn test_redirect_amp_gt() {
        let tokens = tokenize("cmd &>/dev/null");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "&>"));
    }

    #[test]
    fn test_redirect_amp_gt_gt() {
        let tokens = tokenize("cmd &>>/dev/null");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "&>>"));
    }

    #[test]
    fn test_combined_redirect_chain() {
        let tokens = tokenize("cmd > /dev/null 2>&1");
        let redirects: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Redirect)
            .collect();
        assert_eq!(redirects.len(), 2);
        assert_eq!(redirects[0].value, ">");
        assert_eq!(redirects[1].value, "2>&1");
    }

    #[test]
    fn test_redirect_append_to_file() {
        let tokens = tokenize("echo hello >> /tmp/output.txt");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == ">>"));
    }

    #[test]
    fn test_redirect_heredoc_marker() {
        let tokens = tokenize("cat <<EOF");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "<<"));
    }

    #[test]
    fn test_redirect_2_to_1_with_pipe() {
        let tokens = tokenize("cargo test 2>&1 | head");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "2>&1"));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Pipe));
    }

    #[test]
    fn test_redirect_2_to_1_with_and() {
        let tokens = tokenize("cargo test 2>&1 && echo done");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "2>&1"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Operator && t.value == "&&"));
    }

    #[test]
    fn test_exclamation_is_shellism() {
        let tokens = tokenize("if ! grep -q pattern file; then echo missing; fi");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Shellism && t.value == "!"));
    }

    #[test]
    fn test_background_job_is_shellism() {
        let tokens = tokenize("sleep 10 &");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Shellism && t.value == "&"));
    }

    #[test]
    fn test_background_not_confused_with_amp_redirect() {
        let tokens = tokenize("cargo test &>/dev/null");
        assert!(!tokens
            .iter()
            .any(|t| t.kind == TokenKind::Shellism && t.value == "&"));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Redirect));
    }

    #[test]
    fn test_semicolon_no_space() {
        let tokens = tokenize("git status;cargo test");
        assert_eq!(
            tokens
                .iter()
                .filter(|t| t.kind == TokenKind::Operator)
                .count(),
            1
        );
        assert_eq!(
            tokens.iter().filter(|t| t.kind == TokenKind::Arg).count(),
            4
        );
    }

    #[test]
    fn test_offset_tracking() {
        let tokens = tokenize("a && b");
        assert_eq!(tokens[0].offset, 0);
        assert_eq!(tokens[1].offset, 2);
        assert_eq!(tokens[2].offset, 5);
    }

    #[test]
    fn test_offset_segment_extraction() {
        let cmd = "git add . && cargo test";
        let tokens = tokenize(cmd);
        let op = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Operator)
            .unwrap();
        let left = cmd[..op.offset].trim();
        let right_start = op.offset + op.value.len();
        let right = cmd[right_start..].trim();
        assert_eq!(left, "git add .");
        assert_eq!(right, "cargo test");
    }

    #[test]
    fn test_env_prefix_is_arg() {
        let tokens = tokenize("GIT_SSH_COMMAND=ssh git push");
        assert_eq!(tokens[0].kind, TokenKind::Arg);
        assert_eq!(tokens[0].value, "GIT_SSH_COMMAND=ssh");
    }

    #[test]
    fn test_complex_compound() {
        let tokens = tokenize("cargo fmt --all && cargo clippy --all-targets && cargo test");
        let operators: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Operator)
            .collect();
        assert_eq!(operators.len(), 2);
        assert!(operators.iter().all(|t| t.value == "&&"));
    }

    #[test]
    fn test_find_pipe_xargs() {
        let tokens = tokenize("find . -name '*.rs' | xargs grep 'fn run'");
        let pipe_idx = tokens
            .iter()
            .position(|t| t.kind == TokenKind::Pipe)
            .unwrap();
        assert!(pipe_idx > 0);
        let before_pipe: Vec<_> = tokens[..pipe_idx]
            .iter()
            .filter(|t| t.kind == TokenKind::Arg)
            .collect();
        assert!(before_pipe.iter().any(|t| t.value == "find"));
    }

    #[test]
    fn test_fd_redirect_needs_adjacent_digit() {
        let tokens = tokenize("echo 2 > file");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Arg && t.value == "2"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == ">"));
    }

    #[test]
    fn test_fd_redirect_no_space() {
        let tokens = tokenize("echo 2>file");
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Redirect && t.value == "2>"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Arg && t.value == "file"));
    }

    #[test]
    fn test_shell_split_simple() {
        assert_eq!(
            shell_split("head -50 file.php"),
            vec!["head", "-50", "file.php"]
        );
    }

    #[test]
    fn test_shell_split_double_quotes() {
        assert_eq!(
            shell_split(r#"git log --format="%H %s""#),
            vec!["git", "log", "--format=%H %s"]
        );
    }

    #[test]
    fn test_shell_split_single_quotes() {
        assert_eq!(
            shell_split("grep -r 'hello world' ."),
            vec!["grep", "-r", "hello world", "."]
        );
    }

    #[test]
    fn test_shell_split_single_word() {
        assert_eq!(shell_split("ls"), vec!["ls"]);
    }

    #[test]
    fn test_shell_split_empty() {
        let result: Vec<String> = shell_split("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_shell_split_backslash_escape() {
        assert_eq!(
            shell_split(r"echo hello\ world"),
            vec!["echo", "hello world"]
        );
    }

    #[test]
    fn test_shell_split_unclosed_quote() {
        let result = shell_split("echo 'hello");
        assert_eq!(result, vec!["echo", "hello"]);
    }

    #[test]
    fn test_shell_split_mixed_quotes() {
        assert_eq!(
            shell_split(r#"echo "it's" 'a "test"'"#),
            vec!["echo", "it's", "a \"test\""]
        );
    }

    #[test]
    fn test_shell_split_tabs() {
        assert_eq!(shell_split("a\tb\tc"), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_shell_split_multiple_spaces() {
        assert_eq!(shell_split("a   b   c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_strip_quotes_double() {
        assert_eq!(strip_quotes("\"hello\""), "hello");
    }

    #[test]
    fn test_strip_quotes_single() {
        assert_eq!(strip_quotes("'hello'"), "hello");
    }

    #[test]
    fn test_strip_quotes_none() {
        assert_eq!(strip_quotes("hello"), "hello");
    }

    #[test]
    fn test_strip_quotes_mismatched() {
        assert_eq!(strip_quotes("\"hello'"), "\"hello'");
    }

    #[test]
    fn test_split_on_operators_stop_at_pipe() {
        assert_eq!(split_on_operators("a | b | c", true), vec!["a"]);
        assert_eq!(split_on_operators("a && b | c", true), vec!["a", "b"]);
    }

    #[test]
    fn test_split_on_operators_through_pipes() {
        assert_eq!(split_on_operators("a | b | c", false), vec!["a", "b", "c"]);
        assert_eq!(
            split_on_operators("a && b | c ; d", false),
            vec!["a", "b", "c", "d"]
        );
    }

    #[test]
    fn test_split_on_operators_quoted() {
        assert_eq!(
            split_on_operators(r#"echo "a && b" && cargo test"#, false),
            vec![r#"echo "a && b""#, "cargo test"]
        );
    }

    #[test]
    fn test_split_on_operators_empty() {
        assert!(split_on_operators("", false).is_empty());
        assert!(split_on_operators("  ", true).is_empty());
    }

    // --- contains_unattestable_construct (security) -------------------------

    #[test]
    fn test_unattestable_special_quotes() {
        for cmd in [
            r#"git status $'a\'b'; printf BDO_GATE_BYPASS; echo $'c\'d'"#,
            "git status $'a\\'b'\nprintf BDO_GATE_BYPASS; echo $'c\\'d'",
            r#"git status --ignored=$'a\'b' & printf BDO_GATE_BYPASS; echo $'c\'d'"#,
            r#"git status $'a\'b' $(printf BDO_GATE_BYPASS) $'c\'d'"#,
            r#"git status $'ordinary'"#,
            r#"git status $'unterminated"#,
            r#"git status $''"#,
            r#"git status prefix$'日本語\t'"#,
            r#"VALUE=$'a\'b' git status"#,
            r#"git status \\$'active'"#,
        ] {
            assert!(contains_unattestable_construct(cmd), "{cmd:?}");
        }
    }

    #[test]
    fn test_unattestable_special_quotes_across_line_continuations() {
        for cmd in [
            "git status $\\\n'a\\'b'; printf BDO_GATE_BYPASS; echo $'c\\'d'",
            "git status $\\\n\\\n'ordinary'",
        ] {
            assert!(contains_unattestable_construct(cmd), "{cmd:?}");
        }
    }

    #[test]
    fn test_unattestable_substitution_across_line_continuations() {
        for cmd in [
            "git status $\\\n(printf BDO_GATE_BYPASS)",
            "git status $\\\n\\\n(printf BDO_GATE_BYPASS)",
            "git status \"$\\\n(printf BDO_GATE_BYPASS)\"",
            "git log $\\\n(echo --output=/tmp/x)",
            "diff <\\\n(printf a) b",
            "tee >\\\n(cat) </dev/null",
        ] {
            assert!(contains_unattestable_construct(cmd), "{cmd:?}");
        }
        // A continuation that is not followed by `(` stays attestable.
        assert!(!contains_unattestable_construct("git status \\\n--short"));
        assert!(!contains_unattestable_construct(
            "git status '$\\\n(literal)'"
        ));
    }

    #[test]
    fn test_attestable_literal_special_quotes() {
        for cmd in [
            r#"git status "$'literal'""#,
            r#"git status 'literal $' 2>&1"#,
            r#"git status \$\'literal\'"#,
            r#"git status \$\"literal\""#,
            r#"git status \\\$\'literal\'"#,
            r#"git status $"ordinary""#,
            r#"git status "ordinary""#,
        ] {
            assert!(!contains_unattestable_construct(cmd), "{cmd:?}");
        }
    }

    #[test]
    fn test_unattestable_backtick() {
        assert!(contains_unattestable_construct("git status `whoami`"));
    }

    #[test]
    fn test_unattestable_command_substitution() {
        assert!(contains_unattestable_construct(
            "git log --pretty=$(rm -rf ~)"
        ));
    }

    #[test]
    fn test_unattestable_process_substitution() {
        assert!(contains_unattestable_construct("diff <(secret) <(other)"));
        assert!(contains_unattestable_construct("tee >(cat)"));
    }

    #[test]
    fn test_unattestable_substitution_inside_double_quotes() {
        assert!(contains_unattestable_construct(
            r#"git log --pretty="$(rm -rf ~)""#
        ));
        assert!(contains_unattestable_construct(
            r#"git log --pretty="`rm -rf ~`""#
        ));
        assert!(contains_unattestable_construct(
            r#"git -c x="$(whoami)" status"#
        ));
    }

    #[test]
    fn test_attestable_substitution_inside_single_quotes() {
        assert!(!contains_unattestable_construct("echo '$(rm -rf ~)'"));
        assert!(!contains_unattestable_construct("echo '`whoami`'"));
        assert!(!contains_unattestable_construct(r#"echo "\$(rm -rf ~)""#));
    }

    #[test]
    fn test_unattestable_file_redirects() {
        assert!(contains_unattestable_construct("git log > /tmp/x"));
        // nosemgrep: sensitive-path-reference -- test fixture
        assert!(contains_unattestable_construct("echo evil >> ~/.bashrc"));
        assert!(contains_unattestable_construct("cmd &> /tmp/x"));
        // nosemgrep: sensitive-path-reference -- test fixture
        assert!(contains_unattestable_construct("cat < /etc/passwd"));
        assert!(contains_unattestable_construct("cat << EOF"));
    }

    #[test]
    fn test_unattestable_ampersand_file_redirect() {
        // `>&word` (word not a number) == `>word 2>&1` — a file write.
        assert!(contains_unattestable_construct("git status >& /tmp/evil"));
        // nosemgrep: sensitive-path-reference -- test fixture
        assert!(contains_unattestable_construct("cat x >&~/.bashrc"));
        assert!(contains_unattestable_construct("echo hi 2>& /tmp/evil"));
    }

    #[test]
    fn test_attestable_fd_dup_and_devnull_redirects() {
        assert!(!contains_unattestable_construct("git status 2>&1"));
        assert!(!contains_unattestable_construct("cmd >&2"));
        assert!(!contains_unattestable_construct("cmd 2>&-"));
        assert!(!contains_unattestable_construct("cmd 2>/dev/null"));
        assert!(!contains_unattestable_construct("cmd > /dev/null"));
        assert!(!contains_unattestable_construct("cmd &> /dev/null"));
        assert!(!contains_unattestable_construct("cmd >& /dev/null"));
    }

    #[test]
    fn test_attestable_subshell_and_separators() {
        assert!(!contains_unattestable_construct(
            "(git status; cargo build)"
        ));
        assert!(!contains_unattestable_construct(
            "git status && cargo build"
        ));
        assert!(!contains_unattestable_construct("git status; cargo build"));
        assert!(!contains_unattestable_construct("git log | head"));
        assert!(!contains_unattestable_construct("sleep 1 &"));
        assert!(!contains_unattestable_construct("git status\ncargo build"));
    }

    #[test]
    fn test_attestable_variable_expansion() {
        assert!(!contains_unattestable_construct("echo $HOME"));
        assert!(!contains_unattestable_construct("echo ${HOME}"));
        assert!(!contains_unattestable_construct("git status"));
        assert!(!contains_unattestable_construct(""));
    }

    // --- split_for_permissions ---------------------------------------------

    #[test]
    fn test_split_perms_keeps_words_after_a_redirect() {
        // Truncating at the redirect dropped `--force`, so a deny rule for it
        // never saw it.
        for (cmd, want) in [
            (
                "git push 2>/dev/null --force origin main",
                "git push --force origin main",
            ),
            ("git push >out.log --force", "git push --force"),
            ("git push 2> err.txt --force", "git push --force"),
            ("git push 2>&1 --force", "git push --force"),
            ("git push &>log --force", "git push --force"),
            ("cmd <in.txt --flag", "cmd --flag"),
        ] {
            assert_eq!(split_for_permissions(cmd), vec![want], "{cmd}");
        }
        // Text without redirects is kept exactly as written.
        assert_eq!(
            split_for_permissions("ls  *.rs 2>&1 | head"),
            vec!["ls  *.rs", "head"]
        );
    }

    #[test]
    fn test_split_perms_drops_comments() {
        // A quote inside a comment must not swallow the next line.
        assert_eq!(
            split_for_permissions("git status # it's\nrm -rf important '"),
            vec!["git status", "rm -rf important '"]
        );
        assert_eq!(
            split_for_permissions("git status # it\"s\nrm -rf important \""),
            vec!["git status", "rm -rf important \""]
        );
        assert_eq!(
            split_for_permissions("git status # note"),
            vec!["git status"]
        );
        assert_eq!(
            split_for_permissions("# only a comment"),
            Vec::<String>::new()
        );
        // Not comments: `#` inside a word, `$#`, `${#x}`, quoted, escaped.
        for cmd in [
            "echo a#b",
            "echo $#",
            "echo ${#x}",
            "echo '# x'",
            "echo \\# x",
        ] {
            assert_eq!(split_for_permissions(cmd), vec![cmd], "{cmd}");
        }
    }

    #[test]
    fn test_remove_line_continuations_matches_bash() {
        for (input, want) in [
            ("git push --for\\\nce", "git push --force"),
            ("echo \"a\\\nb\"", "echo \"ab\""),
            ("echo 'c\\\nd'", "echo 'c\\\nd'"),
            ("echo x\\\\\necho y", "echo x\\\\\necho y"),
            ("echo A # note \\\necho B", "echo A # note \\\necho B"),
            ("git status", "git status"),
        ] {
            assert_eq!(remove_line_continuations(input), want, "{input:?}");
        }
    }

    #[test]
    fn test_comment_ends_only_at_newline_and_starts_only_after_a_blank() {
        // A comment runs through a CR, as in bash.
        assert_eq!(
            split_for_permissions("git status # x\r; rm y\nls"),
            vec!["git status", "ls"]
        );
        // After NBSP there is no comment: `;` still separates.
        assert_eq!(
            split_for_permissions("git status\u{a0}# x; rm y"),
            vec!["git status\u{a0}# x", "rm y"]
        );
    }

    #[test]
    fn test_canonicalize_segment() {
        for (input, want, expansion) in [
            ("git  push\t--force", "git push --force", false),
            ("git \"push\" --fo''rce", "git push --force", false),
            ("\\git pu\\sh", "git push", false),
            ("echo \"a b\"", "echo a b", false),
            ("echo $\"x\"", "echo x", false),
            ("echo 'a*b' \"\\$x\"", "echo a*b $x", false),
            ("echo $X", "echo $X", true),
            ("echo \"$X\"", "echo $X", true),
            ("echo a*", "echo a*", true),
            ("echo {a,b}", "echo {a,b}", true),
        ] {
            let c = canonicalize_segment(input);
            assert_eq!(c.text, want, "{input:?}");
            assert_eq!(c.has_expansion, expansion, "{input:?}");
        }
        assert_eq!(canonicalize_segment("\"git\" x").first_word, "git");
    }

    #[test]
    fn test_comment_quote_does_not_hide_substitution() {
        assert!(contains_unattestable_construct(
            "git status # it's\n$(rm -rf x) '"
        ));
        assert!(contains_unattestable_construct(
            "git status # it's\necho `id` '"
        ));
        assert!(!contains_unattestable_construct("git status # $(not run)"));
    }

    #[test]
    fn test_split_perms_operators() {
        assert_eq!(
            split_for_permissions("git status && cargo build"),
            vec!["git status", "cargo build"]
        );
        assert_eq!(
            split_for_permissions("git status; cargo build"),
            vec!["git status", "cargo build"]
        );
        assert_eq!(
            split_for_permissions("git log | head"),
            vec!["git log", "head"]
        );
    }

    #[test]
    fn test_split_perms_newline() {
        assert_eq!(
            split_for_permissions("git status\ncargo build"),
            vec!["git status", "cargo build"]
        );
    }

    #[test]
    fn test_split_perms_background_ampersand() {
        assert_eq!(
            split_for_permissions("git status & rm -rf ~"),
            vec!["git status", "rm -rf ~"]
        );
        assert_eq!(split_for_permissions("sleep 1 &"), vec!["sleep 1"]);
    }

    #[test]
    fn test_split_perms_subshell() {
        assert_eq!(
            split_for_permissions("(git status; cargo build)"),
            vec!["git status", "cargo build"]
        );
        assert_eq!(split_for_permissions("((a; b); c)"), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_split_perms_truncates_at_redirect() {
        assert_eq!(split_for_permissions("git status 2>&1"), vec!["git status"]);
        assert_eq!(split_for_permissions("git log > /tmp/x"), vec!["git log"]);
        assert_eq!(
            split_for_permissions("git push --force 2>&1"),
            vec!["git push --force"]
        );
    }

    #[test]
    fn test_split_perms_newline_inside_quotes_not_split() {
        let segments = split_for_permissions("echo 'line1\nline2'");
        assert_eq!(segments.len(), 1);
        assert!(segments[0].starts_with("echo"));
    }

    #[test]
    fn test_split_perms_empty() {
        assert!(split_for_permissions("").is_empty());
        assert!(split_for_permissions("   ").is_empty());
    }
}
