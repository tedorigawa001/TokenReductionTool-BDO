//! Removes whole-line comments from source code without touching anything else.
//!
//! `bdo read` output is what an agent believes the file contains, so deleting a
//! line that is not a comment is far worse than keeping a comment. The rule here
//! is therefore one-sided: a line goes only when it is entirely comment and it
//! starts outside any string. A comment sharing a line with code stays, as do
//! lines inside multi-line strings and heredocs, and a `/*` that never closes.
//!
//! This is a line scanner, not a parser. When it misreads something (a JS regex
//! holding a backtick, say) it believes a string is still open and keeps lines,
//! which costs tokens and nothing else.

use crate::core::filter::{CommentPatterns, Language};
use std::collections::VecDeque;

#[derive(Clone, PartialEq)]
enum HeredocIndent {
    Exact,
    Tabs,
    SpacesAndTabs,
}

#[derive(Clone, PartialEq)]
struct Heredoc {
    terminator: Vec<u8>,
    indent: HeredocIndent,
}

impl Heredoc {
    fn ends_on(&self, line: &[u8]) -> bool {
        let skip = line
            .iter()
            .take_while(|&&b| match self.indent {
                HeredocIndent::Exact => false,
                HeredocIndent::Tabs => b == b'\t',
                HeredocIndent::SpacesAndTabs => matches!(b, b' ' | b'\t'),
            })
            .count();
        line[skip..] == self.terminator
    }
}

#[derive(Clone, PartialEq)]
enum State {
    Code,
    /// In a string opened on an earlier line.
    Str {
        close: &'static str,
        escapes: bool,
    },
    /// In a Rust `r#"…"#` or C++ `R"x(…)x"`: no escapes, closes on exact text.
    RawStr(Vec<u8>),
    /// Heredocs are consumed in declaration order, each with its own indent rules.
    Heredoc(VecDeque<Heredoc>),
    /// Ruby percent syntax is deliberately not parsed: retain the rest rather
    /// than guessing where nested delimiters/interpolations end.
    Verbatim,
    /// In a block comment that is being kept: a doc block, or one opened after
    /// code on the same line.
    KeptComment(usize),
}

/// String delimiters of a language: those that may span lines (checked first,
/// so `"""` wins over `"`) and those a line end terminates.
fn string_delimiters(lang: &Language) -> (&'static [&'static str], &'static [&'static str]) {
    match lang {
        Language::Rust => (&["\""], &[]),
        Language::Python => (&["\"\"\"", "'''"], &["\"", "'"]),
        Language::JavaScript | Language::TypeScript | Language::Go => (&["`"], &["\"", "'"]),
        Language::C | Language::Cpp => (&[], &["\"", "'"]),
        Language::Java => (&["\"\"\""], &["\"", "'"]),
        Language::Ruby | Language::Shell => (&["\"", "'"], &[]),
        Language::Data | Language::Unknown => (&[], &[]),
    }
}

/// Backslash escapes the next byte, except in a shell `'…'` and a Go `` `…` ``.
fn delimiter_has_escapes(lang: &Language, delimiter: &str) -> bool {
    !matches!(
        (lang, delimiter),
        (Language::Shell, "'") | (Language::Go, "`")
    )
}

/// Index just past the first `close` at or after `from`.
fn find_close(line: &[u8], from: usize, close: &[u8], escapes: bool) -> Option<usize> {
    let mut i = from;
    while i < line.len() {
        if escapes && line[i] == b'\\' {
            i += 2;
        } else if line[i..].starts_with(close) {
            return Some(i + close.len());
        } else {
            i += 1;
        }
    }
    None
}

/// Scan an already-open C-style comment. Rust comments nest; other supported
/// C-style comment syntaxes stop at the first close. Return the remaining depth
/// when a line ends so kept/doc comments use the same rules as removed comments.
fn scan_block_comment(
    line: &[u8],
    from: usize,
    mut depth: usize,
    nested: bool,
) -> (Option<usize>, usize) {
    let mut i = from;
    while i < line.len() {
        if nested && line[i..].starts_with(b"/*") {
            depth += 1;
            i += 2;
        } else if line[i..].starts_with(b"*/") {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return (Some(i), 0);
            }
        } else {
            i += 1;
        }
    }
    (None, depth)
}

/// Index just past the backtick closing a JS template, walking `${…}` so that
/// quotes and backticks inside an interpolation are not mistaken for the close.
/// One pass with an explicit stack: a template can be a whole minified line.
fn find_template_close(line: &[u8], from: usize) -> Option<usize> {
    const TEMPLATE: u8 = b'`';
    const BRACE: u8 = b'{';
    let mut stack = vec![TEMPLATE];
    let mut i = from;
    while i < line.len() {
        let in_template = stack.last() == Some(&TEMPLATE);
        match line[i] {
            b'\\' => i += 2,
            b'`' if in_template => {
                stack.pop();
                i += 1;
                if stack.is_empty() {
                    return Some(i);
                }
            }
            b'$' if in_template && line.get(i + 1) == Some(&b'{') => {
                stack.push(BRACE);
                i += 2;
            }
            b'{' if !in_template => {
                stack.push(BRACE);
                i += 1;
            }
            b'}' if !in_template => {
                stack.pop();
                i += 1;
            }
            b'`' => {
                stack.push(TEMPLATE);
                i += 1;
            }
            quote @ (b'"' | b'\'') if !in_template => {
                i = find_close(line, i + 1, &[quote], true).unwrap_or(i + 1);
            }
            _ => i += 1,
        }
    }
    None
}

/// Whether a `/` after `before` opens a regex rather than divides: a literal
/// cannot follow a value. On a wrong guess the quotes in between are read as
/// code, as they would be without this check.
fn regex_can_follow(before: &[u8], lang: &Language) -> bool {
    let before = before.trim_ascii_end();
    match before.last() {
        None => true,
        Some(
            b'(' | b',' | b'=' | b':' | b'[' | b'!' | b'&' | b'|' | b'?' | b'{' | b';' | b'+'
            | b'-' | b'*' | b'%' | b'~' | b'^',
        ) => true,
        Some(_) => {
            let keywords: &[&[u8]] = match lang {
                Language::Ruby => &[b"if", b"unless", b"when", b"and", b"or", b"not", b"return"],
                _ => &[
                    b"return", b"typeof", b"case", b"in", b"of", b"void", b"throw", b"yield",
                ],
            };
            let word_len = before
                .iter()
                .rev()
                .take_while(|b| is_identifier_byte(**b))
                .count();
            keywords.contains(&&before[before.len() - word_len..])
        }
    }
}

/// Index just past a regex literal whose opening `/` is at `line[i]`. Regex
/// literals do not span lines.
fn regex_literal_end(line: &[u8], i: usize) -> Option<usize> {
    let mut in_class = false;
    let mut j = i + 1;
    while j < line.len() {
        match line[j] {
            b'\\' => j += 1,
            b'[' => in_class = true,
            b']' => in_class = false,
            b'/' if !in_class => return Some(j + 1),
            _ => {}
        }
        j += 1;
    }
    None
}

fn is_identifier_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// A heredoc opening at `line[i..]`, which starts with `<<`: its terminator and
/// the index past it. A false positive only means keeping lines.
fn heredoc_at(line: &[u8], i: usize, lang: &Language) -> Option<(Heredoc, usize)> {
    let mut j = i + 2;
    if line.get(j) == Some(&b'<') || (i > 0 && line[i - 1] == b'<') {
        return None; // here-string
    }
    let indent = match (lang, line.get(j)) {
        (Language::Shell, Some(b'-')) => HeredocIndent::Tabs,
        (Language::Ruby, Some(b'-' | b'~')) => HeredocIndent::SpacesAndTabs,
        _ => HeredocIndent::Exact,
    };
    if indent != HeredocIndent::Exact {
        j += 1;
    }
    if *lang == Language::Shell {
        // Ruby cannot allow this: `class << self` and `list << item`.
        while matches!(line.get(j), Some(b' ' | b'\t')) {
            j += 1;
        }
        if line.get(j) == Some(&b'\\') {
            j += 1;
        }
    }
    let quote = line.get(j).copied().filter(|b| matches!(b, b'\'' | b'"'));
    let terminator = if let Some(q) = quote {
        // Quoted, a terminator may be any text: `<<-"begin;"`.
        let start = j + 1;
        let len = line[start..].iter().position(|b| *b == q)?;
        j = start + len + 1;
        line[start..start + len].to_vec()
    } else {
        let start = j;
        while line.get(j).copied().is_some_and(is_identifier_byte) {
            j += 1;
        }
        line[start..j].to_vec()
    };
    if terminator.is_empty() {
        return None;
    }
    Some((Heredoc { terminator, indent }, j))
}

/// The closing text of a raw string opening at `line[i..]`, and the index past
/// the opener.
fn raw_string_at(line: &[u8], i: usize, lang: &Language) -> Option<(Vec<u8>, usize)> {
    match lang {
        Language::Rust if line[i] == b'r' => {
            if i > 0 && is_identifier_byte(line[i - 1]) && line[i - 1] != b'b' {
                return None;
            }
            let mut j = i + 1;
            while line.get(j) == Some(&b'#') {
                j += 1;
            }
            if line.get(j) != Some(&b'"') {
                return None;
            }
            let mut close = vec![b'"'];
            close.extend_from_slice(&line[i + 1..j]);
            Some((close, j + 1))
        }
        Language::Cpp if line[i] == b'R' && line.get(i + 1) == Some(&b'"') => {
            // The standard caps the delimiter at 16 characters, which also keeps
            // a line of repeated `R"` linear.
            let open = line[i + 2..].iter().take(17).position(|&b| b == b'(')?;
            let mut close = vec![b')'];
            close.extend_from_slice(&line[i + 2..i + 2 + open]);
            close.push(b'"');
            Some((close, i + 2 + open + 1))
        }
        _ => None,
    }
}

/// Bytes of a Rust char literal at `line[i..]`, or `None` for a lifetime.
fn rust_char_literal_len(line: &[u8], i: usize) -> Option<usize> {
    let next = *line.get(i + 1)?;
    if next == b'\\' {
        return find_close(line, i + 1, b"'", true).map(|end| end - i);
    }
    let width = match next {
        0xF0.. => 4,
        0xE0.. => 3,
        0xC0.. => 2,
        _ => 1,
    };
    (line.get(i + 1 + width) == Some(&b'\'')).then_some(width + 2)
}

/// Walks one line from `from` and returns the state the next line starts in.
fn scan_line(
    line: &[u8],
    from: usize,
    mut state: State,
    lang: &Language,
    patterns: &CommentPatterns,
) -> State {
    if let State::Heredoc(pending) = &mut state {
        if pending.front().is_some_and(|doc| doc.ends_on(line)) {
            pending.pop_front();
        }
        return if pending.is_empty() {
            State::Code
        } else {
            state
        };
    }

    // Everything after Ruby's `__END__` is data for `DATA.read`, not source.
    if *lang == Language::Ruby && state == State::Code && line == b"__END__" {
        return State::Verbatim;
    }

    // `"#{<<~X}"` and `"$(cat <<X)"` open a heredoc from inside a string, and its
    // body follows the line the string may close on. Tracking both is not worth
    // it: keep the rest of the file. A line that merely looks like this costs
    // savings, nothing else.
    let contains = |needle: &[u8]| line.windows(needle.len()).any(|w| w == needle);
    let heredoc_in_string = match lang {
        Language::Ruby => contains(b"#{<<"),
        Language::Shell => contains(b"\"$(") && contains(b"<<"),
        _ => false,
    };
    if heredoc_in_string {
        return State::Verbatim;
    }

    let (multi, single) = string_delimiters(lang);
    let line_comment = patterns.line.map(str::as_bytes);
    // Mid-line block comments exist only for `/*`: Ruby's `=begin` is column-0.
    let block_start = patterns
        .block_start
        .filter(|s| *s == "/*")
        .map(str::as_bytes);
    // `$#`, `${#a}` and `?#` are not comments in these two.
    let hash_needs_word_start = matches!(lang, Language::Shell | Language::Ruby);
    let is_js = matches!(lang, Language::JavaScript | Language::TypeScript);
    let mut regex_may_close = is_js || *lang == Language::Ruby;
    let mut pending_heredocs = VecDeque::new();

    let mut i = from;
    'scan: while i < line.len() {
        match &state {
            State::Str { close, escapes } => {
                let end = if is_js && *close == "`" {
                    find_template_close(line, i)
                } else {
                    find_close(line, i, close.as_bytes(), *escapes)
                };
                match end {
                    Some(end) => {
                        i = end;
                        state = State::Code;
                    }
                    None => return state,
                }
            }
            State::RawStr(close) => match find_close(line, i, close, false) {
                Some(end) => {
                    i = end;
                    state = State::Code;
                }
                None => return state,
            },
            State::KeptComment(depth) => {
                let (end, remaining) = scan_block_comment(line, i, *depth, *lang == Language::Rust);
                if let Some(end) = end {
                    i = end;
                    state = State::Code;
                } else {
                    return State::KeptComment(remaining);
                }
            }
            State::Heredoc(_) | State::Verbatim => return state,
            State::Code => {
                let rest = &line[i..];
                if rest[0] == b'\\' {
                    i += 2;
                    continue;
                }
                if let Some(comment) = line_comment {
                    let at_word_start = i == 0
                        || !hash_needs_word_start
                        || matches!(line[i - 1], b' ' | b'\t' | b';' | b'(' | b')' | b'|' | b'&');
                    if rest.starts_with(comment) && at_word_start {
                        break;
                    }
                }
                // `%` also means modulo in Ruby. Conservatively keeping the
                // remainder in that case only costs savings, never source data.
                if *lang == Language::Ruby && rest[0] == b'%' {
                    return State::Verbatim;
                }
                if block_start.is_some_and(|s| rest.starts_with(s)) {
                    state = State::KeptComment(1);
                    i += 2;
                    continue;
                }
                if hash_needs_word_start && rest.starts_with(b"<<") {
                    if let Some((heredoc, end)) = heredoc_at(line, i, lang) {
                        pending_heredocs.push_back(heredoc);
                        i = end;
                        continue;
                    }
                }
                if let Some((close, end)) = raw_string_at(line, i, lang) {
                    state = State::RawStr(close);
                    i = end;
                    continue;
                }
                if *lang == Language::Rust && rest[0] == b'\'' {
                    i += rust_char_literal_len(line, i).unwrap_or(1);
                    continue;
                }
                if regex_may_close && rest[0] == b'/' && regex_can_follow(&line[..i], lang) {
                    match regex_literal_end(line, i) {
                        Some(end) => {
                            i = end;
                            continue;
                        }
                        // Unlike a JS one, a Ruby literal can span lines, and a `#`
                        // line inside it is part of the pattern.
                        None if *lang == Language::Ruby => {
                            return State::Str {
                                close: "/",
                                escapes: true,
                            };
                        }
                        // Nothing closes later on this line either: do not rescan.
                        None => regex_may_close = false,
                    }
                }
                // `$'…'` is the one shell single quote that honours backslashes.
                if *lang == Language::Shell && rest.starts_with(b"$'") {
                    state = State::Str {
                        close: "'",
                        escapes: true,
                    };
                    i += 2;
                    continue;
                }
                for &delimiter in multi {
                    if rest.starts_with(delimiter.as_bytes()) {
                        state = State::Str {
                            close: delimiter,
                            escapes: delimiter_has_escapes(lang, delimiter),
                        };
                        i += delimiter.len();
                        continue 'scan;
                    }
                }
                for &delimiter in single {
                    if rest.starts_with(delimiter.as_bytes()) {
                        let open_end = i + delimiter.len();
                        match find_close(line, open_end, delimiter.as_bytes(), true) {
                            Some(end) => {
                                i = end;
                                continue 'scan;
                            }
                            // `\` at the end of the line continues the string.
                            None if line.last() == Some(&b'\\') => {
                                return State::Str {
                                    close: delimiter,
                                    escapes: true,
                                };
                            }
                            // A stray quote, as in JSX text: `<p>don't</p>`. A
                            // string may still open later on this line.
                            None => {
                                i = open_end;
                                continue 'scan;
                            }
                        }
                    }
                }
                i += 1;
            }
        }
    }

    if state == State::Code && !pending_heredocs.is_empty() {
        State::Heredoc(pending_heredocs)
    } else {
        state
    }
}

/// Where a block comment opened on `lines[start]` ends: the line it closes on
/// and the byte offset just past the close.
fn find_block_end(
    lines: &[&str],
    start: usize,
    open_at: usize,
    block_start: &str,
    block_end: &str,
    nested: bool,
) -> Option<(usize, usize)> {
    // Ruby's `=end` only counts at the start of a line.
    if block_start == "=begin" {
        return lines[start + 1..]
            .iter()
            .position(|l| l.starts_with(block_end))
            .map(|n| (start + 1 + n, lines[start + 1 + n].len()));
    }
    let mut depth = 1;
    for (index, line) in lines.iter().enumerate().skip(start) {
        let from = if index == start {
            open_at + block_start.len()
        } else {
            0
        };
        let (end, remaining) = scan_block_comment(line.as_bytes(), from, depth, nested);
        if let Some(end) = end {
            return Some((index, end));
        }
        depth = remaining;
    }
    None
}

/// `content` with its whole-line comments removed and runs of blank lines
/// collapsed to one, each only outside strings: a blank line inside a string is
/// part of its value. Kept lines are verbatim and each ends in `\n`.
pub fn strip_comment_lines(content: &str, lang: &Language) -> String {
    let patterns = lang.comment_patterns();
    let mut out = String::with_capacity(content.len());
    let lines: Vec<&str> = content.lines().collect();
    let mut state = State::Code;
    // Set once a search reached EOF, so a file full of unclosed `/*` stays linear.
    let mut no_block_end_ahead = false;
    let mut last_was_blank = false;

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if state == State::Code {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                if !last_was_blank {
                    out.push('\n');
                }
                last_was_blank = true;
                i += 1;
                continue;
            }
            // A shebang is what runs the file, not a comment on it.
            let shebang = i == 0 && trimmed.starts_with("#!");
            if patterns.line.is_some_and(|c| trimmed.starts_with(c)) && !shebang {
                if patterns.doc_line.is_some_and(|d| trimmed.starts_with(d)) {
                    out.push_str(line);
                    out.push('\n');
                    last_was_blank = false;
                }
                i += 1;
                continue;
            }

            if let (Some(start), Some(end)) = (patterns.block_start, patterns.block_end) {
                let opens = if start == "=begin" {
                    line.starts_with(start)
                } else {
                    trimmed.starts_with(start)
                };
                let is_doc = patterns
                    .doc_block_start
                    .is_some_and(|d| trimmed.starts_with(d));
                if opens && !is_doc && !no_block_end_ahead {
                    let open_at = line.len() - line.trim_start().len();
                    match find_block_end(&lines, i, open_at, start, end, *lang == Language::Rust) {
                        Some((close_line, after_close)) => {
                            let tail = lines[close_line].as_bytes()[after_close..].trim_ascii();
                            if !tail.is_empty() {
                                // Code follows the comment, so its last line
                                // stays. The rest of the comment stays with it:
                                // a lone `*/` would not be valid source.
                                for kept in &lines[i..=close_line] {
                                    out.push_str(kept);
                                    out.push('\n');
                                }
                                last_was_blank = false;
                                state = scan_line(
                                    lines[close_line].as_bytes(),
                                    after_close,
                                    State::Code,
                                    lang,
                                    &patterns,
                                );
                            }
                            i = close_line + 1;
                            continue;
                        }
                        None => no_block_end_ahead = true,
                    }
                }
            }
        }

        out.push_str(line);
        out.push('\n');
        last_was_blank = false;
        state = scan_line(line.as_bytes(), 0, state, lang, &patterns);
        i += 1;
    }
    out
}

#[cfg(test)]
mod string_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(code: &str, lang: Language) -> Vec<String> {
        strip_comment_lines(code, &lang)
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Every line of `code` must survive.
    fn assert_all_kept(code: &str, lang: Language) {
        let expected: Vec<&str> = code.lines().collect();
        assert_eq!(kept(code, lang), expected);
    }

    #[test]
    fn a_comment_opener_inside_a_string_is_not_a_comment() {
        assert_all_kept(
            "const a = 1;\nconst glob = \"src/*.ts\";\nconst b = 2;\nfunction keep() {}\n",
            Language::TypeScript,
        );
    }

    #[test]
    fn a_comment_sharing_a_line_with_code_stays() {
        let code =
            "const a = x /* inline */ + y;\n/* leading */ const b = 2;\nconst c = 3; // tail\n";
        assert_all_kept(code, Language::TypeScript);
    }

    #[test]
    fn whole_line_comments_go() {
        let code = "// one\nlet a = 1;\n/* two */\nlet b = 2;\n/* three\n   four */\nlet c = 3;\n";
        assert_eq!(
            kept(code, Language::JavaScript),
            ["let a = 1;", "let b = 2;", "let c = 3;"]
        );
    }

    #[test]
    fn code_after_a_multi_line_comment_stays_and_is_scanned() {
        let code = "/* a\n   b */ let s = `\n// in template\n`;\n// real\nlet c = 3;\n";
        assert_eq!(
            kept(code, Language::JavaScript),
            [
                "/* a",
                "   b */ let s = `",
                "// in template",
                "`;",
                "let c = 3;"
            ]
        );
    }

    #[test]
    fn an_unclosed_comment_opener_loses_nothing() {
        assert_all_kept("let a = 1;\n/* never closed\nlet b = 2;\n", Language::C);
    }

    #[test]
    fn doc_comments_are_kept() {
        assert_all_kept(
            "/** doc\n * more\n */\nfn f() {}\n/// line doc\nfn g() {}\n",
            Language::Rust,
        );
    }

    #[test]
    fn multi_line_strings_keep_their_lines() {
        assert_all_kept(
            "const s = `\n// inside\n/* inside\n`;\nconst after = 1;\n",
            Language::JavaScript,
        );
        assert_all_kept("s := `\n// inside\n/* inside\n`\nx := 1\n", Language::Go);
        assert_all_kept(
            "let s = \"\n// inside\n/* inside\n\";\nlet after = 1;\n",
            Language::Rust,
        );
        assert_all_kept(
            "let s = r##\"\n\"# // inside\n/* inside\n\"##;\nlet after = 1;\n",
            Language::Rust,
        );
        assert_all_kept(
            "auto s = R\"x(\n// inside\n)\" still inside\n)x\";\nint after = 1;\n",
            Language::Cpp,
        );
        assert_all_kept(
            "String s = \"\"\"\n// inside\n\"\"\";\nint after = 1;\n",
            Language::Java,
        );
    }

    #[test]
    fn python_triple_quoted_strings_are_not_comments() {
        assert_all_kept(
            "SQL = \"\"\"\nselect 1\n# not a comment\n\"\"\"\none = \"\"\"x\"\"\"\nafter = 1\n",
            Language::Python,
        );
        assert_all_kept(
            "def f():\n    '''doc\n    # text\n    '''\n    return 1\n",
            Language::Python,
        );
        assert_eq!(
            kept("# real\nx = 1  # tail\n# real\ny = 2\n", Language::Python),
            ["x = 1  # tail", "y = 2"]
        );
    }

    #[test]
    fn quotes_inside_comments_do_not_open_strings() {
        let code =
            "let a = 1; // don't \"stop\n// gone\nlet b = 2; /* it's */\n// gone\nlet c = 3;\n";
        assert_eq!(
            kept(code, Language::Rust),
            [
                "let a = 1; // don't \"stop",
                "let b = 2; /* it's */",
                "let c = 3;"
            ]
        );
    }

    #[test]
    fn rust_char_literals_and_lifetimes_do_not_confuse_the_scan() {
        let code = "let q = '\"';\n// gone\nfn f<'a>(x: &'a str) {}\n// gone\nlet e = '\\'';\n// gone\nlet u = 'あ';\n// gone\n";
        assert_eq!(
            kept(code, Language::Rust),
            [
                "let q = '\"';",
                "fn f<'a>(x: &'a str) {}",
                "let e = '\\'';",
                "let u = 'あ';"
            ]
        );
    }

    #[test]
    fn a_stray_quote_does_not_leak_into_later_lines() {
        let code = "<p>don't</p>\n// gone\nconst a = 1;\n";
        assert_eq!(
            kept(code, Language::JavaScript),
            ["<p>don't</p>", "const a = 1;"]
        );
    }

    #[test]
    fn a_string_continued_with_a_backslash_keeps_its_next_line() {
        assert_all_kept("char *s = \"a\\\n// inside\";\nint b;\n", Language::C);
    }

    #[test]
    fn heredocs_keep_their_lines() {
        assert_all_kept("cat <<EOF\n# inside\nEOF\necho done\n", Language::Shell);
        assert_all_kept(
            "cat << 'EOF' > out\n# inside\nEOF\necho done\n",
            Language::Shell,
        );
        assert_all_kept(
            "sql = <<~SQL\n  # inside\n  SQL\nrun(sql)\n",
            Language::Ruby,
        );
        assert_eq!(
            kept("cat <<EOF\nx\nEOF\n# gone\necho done\n", Language::Shell),
            ["cat <<EOF", "x", "EOF", "echo done"]
        );
    }

    #[test]
    fn shell_hash_inside_a_word_is_not_a_comment() {
        // `$#` must not end the scan: the `'` after it opens a real string.
        let code = "echo $# '\n# inside\n'\n# gone\necho done\n";
        assert_eq!(
            kept(code, Language::Shell),
            ["echo $# '", "# inside", "'", "echo done"]
        );
    }

    #[test]
    fn a_shebang_stays_but_later_hash_lines_go() {
        assert_eq!(
            kept("#!/bin/sh\n# gone\necho hi\n", Language::Shell),
            ["#!/bin/sh", "echo hi"]
        );
    }

    #[test]
    fn ruby_block_comments_need_column_zero() {
        assert_eq!(
            kept("=begin\ntext\n=end\nputs 1\n", Language::Ruby),
            ["puts 1"]
        );
        assert_all_kept("x =begin\nputs 1\n", Language::Ruby);
    }

    #[test]
    fn unknown_and_data_files_are_never_stripped() {
        for lang in [Language::Unknown, Language::Data] {
            assert_all_kept(
                "first\n// a log line\n/* not closed\n# heading\nlast\n",
                lang,
            );
        }
    }

    #[test]
    fn multi_byte_text_around_delimiters_is_safe() {
        let code = "const s = \"日本語/*\"; // 説明 ✓\n// 消える\nconst t = `あ\n// 残る\nい`;\n";
        assert_eq!(
            kept(code, Language::TypeScript),
            [
                "const s = \"日本語/*\"; // 説明 ✓",
                "const t = `あ",
                "// 残る",
                "い`;"
            ]
        );
    }

    #[test]
    fn blank_runs_collapse_only_outside_strings() {
        assert_eq!(
            kept("a = 1\n\n\n# gone\n  \n\nb = 2\n", Language::Python),
            ["a = 1", "", "b = 2"]
        );
        assert_all_kept("s = \"\"\"x\n\n\n\ny\n\"\"\"\nb = 2\n", Language::Python);
    }

    #[test]
    fn a_file_of_unclosed_openers_is_linear() {
        let code = "/* x\n".repeat(200_000);
        assert_eq!(strip_comment_lines(&code, &Language::C).len(), code.len());
    }
}
