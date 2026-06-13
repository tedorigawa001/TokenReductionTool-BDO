//! Code "outline" view: keep declarations (doc comments, signatures, type/field
//! shapes) and elide *function bodies*, replacing them with `{ … }`.
//!
//! This sits between `read` (near-full source) and `smart` (a 2-line summary):
//! it gives an LLM the API surface of a file — every signature, struct field,
//! enum variant, trait method, and doc comment — at a fraction of the tokens,
//! without the implementation noise.
//!
//! The extractor is a deliberately small heuristic, not a parser:
//!   * Brace languages (Rust, Go, JS, TS, C, C++, Java): a block whose header
//!     contains `()` is treated as a function/method and its body is elided;
//!     any other block (`struct`/`enum`/`impl`/`trait`/`mod`/`class` …) is kept
//!     and we recurse into it, so nested fields and signatures survive.
//!   * Python: indentation-based — `def`/`class` headers and decorators are
//!     kept, indented bodies are elided.
//!   * Anything else returns `None` so the caller can fall back gracefully.
//!
//! It is intentionally conservative: when in doubt it keeps a line rather than
//! dropping it, and `read` already falls back to raw content if the result is
//! empty.

use crate::core::filter::Language;

/// Produce an outline of `content`, or `None` if the language is unsupported.
pub fn outline(content: &str, lang: &Language) -> Option<String> {
    match lang {
        Language::Rust
        | Language::Go
        | Language::JavaScript
        | Language::TypeScript
        | Language::C
        | Language::Cpp
        | Language::Java => Some(outline_braces(content, matches!(lang, Language::Rust))),
        Language::Python => Some(outline_python(content)),
        Language::Ruby | Language::Shell | Language::Data | Language::Unknown => None,
    }
}

/// Returns `true` for lines worth keeping verbatim as part of a declaration:
/// doc comments and attributes/decorators that annotate the following item.
fn is_annotation(trimmed: &str) -> bool {
    trimmed.starts_with("///")        // Rust outer doc
        || trimmed.starts_with("//!") // Rust inner doc
        || trimmed.starts_with("/**") // block doc
        || trimmed.starts_with("* ")  // continuation of a block doc
        || trimmed == "*"
        || trimmed.starts_with("*/")
        || trimmed.starts_with("#[")  // Rust attribute
        || trimmed.starts_with("#!")  // Rust inner attribute
        || trimmed.starts_with('@') // JS/TS/Java decorator/annotation
}

/// A plain (non-doc) comment or an import/use line — dropped for compactness.
fn is_dropped_noise(trimmed: &str) -> bool {
    (trimmed.starts_with("//") && !trimmed.starts_with("///") && !trimmed.starts_with("//!"))
        || trimmed.starts_with("use ")
        || trimmed.starts_with("import ")
        || trimmed.starts_with("#include")
        || trimmed.starts_with("package ")
        || trimmed.starts_with("extern crate")
}

/// Is the `'` at `i` the start of a char literal (`'x'`, `'\n'`) rather than a
/// Rust lifetime (`'a`, `'static`)? Char literals must be treated as literals so
/// braces inside them (e.g. `'{'`) are not counted; lifetimes must be ignored so
/// their lone `'` doesn't swallow the rest of the line.
fn is_char_literal_start(bytes: &[u8], i: usize) -> bool {
    bytes.get(i + 1) == Some(&b'\\') || bytes.get(i + 2) == Some(&b'\'')
}

/// Counts the net `{`/`}` balance on a line, ignoring braces inside string/char
/// literals and after a `//` line comment. `rust` enables lifetime-aware `'`
/// handling. Good enough for well-formed source; the outline is a best-effort
/// view, not a compiler.
fn brace_delta(line: &str, in_block_comment: &mut bool, rust: bool) -> i32 {
    let bytes = line.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    let mut in_str: Option<u8> = None; // Some(quote) when inside a string/char
    while i < bytes.len() {
        let c = bytes[i];
        if *in_block_comment {
            if c == b'*' && bytes.get(i + 1) == Some(&b'/') {
                *in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if let Some(q) = in_str {
            if c == b'\\' {
                i += 2; // skip escaped char
                continue;
            }
            if c == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => in_str = Some(c),
            b'\'' if rust && !is_char_literal_start(bytes, i) => {} // Rust lifetime: ignore
            b'\'' => in_str = Some(c),
            b'/' if bytes.get(i + 1) == Some(&b'/') => break, // line comment: rest is text
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                *in_block_comment = true;
                i += 2;
                continue;
            }
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    depth
}

/// Index of the first `{` on the line that is real code (not in a string/comment).
fn first_code_brace(line: &str, rust: bool) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut in_str: Option<u8> = None;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = in_str {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => in_str = Some(c),
            b'\'' if rust && !is_char_literal_start(bytes, i) => {} // Rust lifetime: ignore
            b'\'' => in_str = Some(c),
            b'/' if bytes.get(i + 1) == Some(&b'/') => return None,
            b'{' => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

fn outline_braces(content: &str, rust: bool) -> String {
    let mut out = String::new();
    let mut in_block_comment = false;

    // When `Some(d)`, we are inside an elided function body and drop lines until
    // the brace depth falls back to `d`.
    let mut skip_to_depth: Option<i32> = None;
    let mut depth: i32 = 0;

    // Header of the current declaration accumulated since the last statement
    // boundary, used to decide function (`()`) vs structural block.
    let mut header = String::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if let Some(target) = skip_to_depth {
            // Inside an elided body: only track braces to find its end.
            depth += brace_delta(line, &mut in_block_comment, rust);
            if depth <= target {
                skip_to_depth = None;
            }
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }
        if is_annotation(trimmed) {
            out.push_str(line);
            out.push('\n');
            // block-doc lines never change brace depth meaningfully
            continue;
        }
        if is_dropped_noise(trimmed) {
            continue;
        }

        let delta = brace_delta(line, &mut in_block_comment, rust);

        if delta > 0 {
            // This line opens at least one block.
            header.push_str(trimmed);
            header.push(' ');
            // A block whose header carries a parameter list is a function/method.
            let is_fn = header.contains('(') && header.contains(')');
            let brace_at = first_code_brace(line, rust).unwrap_or(line.len());

            if is_fn {
                // Function/method: keep the signature, elide the body.
                let sig = line[..brace_at].trim_end();
                out.push_str(sig);
                out.push_str(" { … }\n");
                let depth_before = depth;
                depth += delta;
                // Drop lines until we return to the depth before this body opened.
                // (If the body opened and closed on one line, delta is 0 and we
                // don't enter skip mode.)
                if depth > depth_before {
                    skip_to_depth = Some(depth_before);
                }
            } else {
                // Structural block (struct/enum/impl/trait/mod/class …): keep the
                // line and recurse into it.
                out.push_str(line);
                out.push('\n');
                depth += delta;
            }
            header.clear();
        } else {
            // No new block on this line.
            out.push_str(line);
            out.push('\n');
            depth += delta;
            // Accumulate the current declaration's header so a `{` on a later
            // line (multi-line signature) can still see its `(` … `)`. A trailing
            // `,` means a parameter/field continues, so it is NOT a boundary;
            // only `;` and `}` end the current declaration.
            if trimmed.ends_with(';') || trimmed.ends_with('}') {
                header.clear();
            } else {
                header.push_str(trimmed);
                header.push(' ');
            }
        }
    }

    collapse_blank_runs(&out)
}

fn outline_python(content: &str) -> String {
    let mut out = String::new();
    // When `Some(n)`, we are inside the body of a `def`/`class` whose header was
    // at indent `n`; its body is everything indented deeper than `n`.
    let mut body_indent: Option<usize> = None;
    let mut pending_decorators: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let indent = line.len() - trimmed.len();

        // A line at or below the current header's indent ends that body.
        if let Some(b) = body_indent {
            if indent <= b {
                body_indent = None;
            }
        }

        // Decorators annotate the next def/class at any nesting level.
        if trimmed.starts_with('@') {
            pending_decorators.push(line.to_string());
            continue;
        }

        if trimmed.starts_with("def ") || trimmed.starts_with("class ") {
            // Keep the header (top-level or nested method/class); flush decorators.
            for d in pending_decorators.drain(..) {
                out.push_str(&d);
                out.push('\n');
            }
            out.push_str(line);
            out.push('\n');
            body_indent = Some(indent);
            continue;
        }

        // Not a decorator or def/class header.
        pending_decorators.clear();
        if body_indent.is_none() {
            // Module top level: keep imports, constants, assignments.
            out.push_str(line);
            out.push('\n');
        }
        // Otherwise we're inside a body → elide.
    }

    collapse_blank_runs(&out)
}

/// Collapse runs of 2+ blank lines into a single blank line (defensive; the
/// passes above already skip blanks, but doc-block handling can leave gaps).
fn collapse_blank_runs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_blank = false;
    for line in s.lines() {
        let blank = line.trim().is_empty();
        if blank && prev_blank {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        prev_blank = blank;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_fn_body_elided() {
        let src = "\
/// Doc for foo.
pub fn foo(x: u32) -> u32 {
    let y = x + 1;
    y * 2
}
";
        let o = outline(src, &Language::Rust).unwrap();
        assert!(o.contains("/// Doc for foo."), "doc kept: {o}");
        assert!(o.contains("pub fn foo(x: u32) -> u32 { … }"), "sig+elision: {o}");
        assert!(!o.contains("let y"), "body dropped: {o}");
    }

    #[test]
    fn test_rust_struct_fields_kept() {
        let src = "\
pub struct Point {
    pub x: i32,
    pub y: i32,
}
";
        let o = outline(src, &Language::Rust).unwrap();
        assert!(o.contains("pub struct Point {"), "{o}");
        assert!(o.contains("pub x: i32,"), "fields kept: {o}");
        assert!(o.contains("pub y: i32,"), "{o}");
    }

    #[test]
    fn test_rust_impl_keeps_method_sigs_elides_bodies() {
        let src = "\
impl Point {
    pub fn norm(&self) -> f64 {
        ((self.x * self.x + self.y * self.y) as f64).sqrt()
    }
    pub fn zero() -> Self {
        Self { x: 0, y: 0 }
    }
}
";
        let o = outline(src, &Language::Rust).unwrap();
        assert!(o.contains("impl Point {"), "{o}");
        assert!(o.contains("pub fn norm(&self) -> f64 { … }"), "{o}");
        assert!(o.contains("pub fn zero() -> Self { … }"), "{o}");
        assert!(!o.contains("sqrt()"), "body dropped: {o}");
        assert!(!o.contains("x: 0"), "body dropped: {o}");
    }

    #[test]
    fn test_rust_trait_method_sig_kept() {
        let src = "\
pub trait Shape {
    fn area(&self) -> f64;
    fn name(&self) -> &str {
        \"shape\"
    }
}
";
        let o = outline(src, &Language::Rust).unwrap();
        assert!(o.contains("pub trait Shape {"), "{o}");
        assert!(o.contains("fn area(&self) -> f64;"), "decl-only kept: {o}");
        assert!(o.contains("fn name(&self) -> &str { … }"), "default elided: {o}");
        assert!(!o.contains("\"shape\""), "body dropped: {o}");
    }

    #[test]
    fn test_brace_in_string_does_not_break_counting() {
        let src = "\
pub fn render() -> String {
    format!(\"{{ not a real brace }}\")
}
pub fn after() -> u32 {
    1
}
";
        let o = outline(src, &Language::Rust).unwrap();
        // If string braces were miscounted, `after` would be swallowed.
        assert!(o.contains("pub fn render() -> String { … }"), "{o}");
        assert!(o.contains("pub fn after() -> u32 { … }"), "{o}");
    }

    // Rust lifetimes use a lone `'` that must NOT be read as a char-literal
    // opener, or the brace counter swallows the rest of the line and the method
    // body is never elided. Regression for the `&'a str` case.
    #[test]
    fn test_rust_lifetime_in_signature() {
        let src = "\
impl<'a> Opts<'a> {
    pub fn with(label: &'a str) -> Self {
        Self { label }
    }
}
";
        let o = outline(src, &Language::Rust).unwrap();
        assert!(o.contains("pub fn with(label: &'a str) -> Self { … }"), "{o}");
        assert!(!o.contains("Self { label }"), "body must be elided: {o}");
    }

    // A char literal containing a brace (`'{'`) must be treated as a literal so
    // its brace is not counted.
    #[test]
    fn test_rust_char_literal_brace_not_counted() {
        let src = "\
pub fn open() -> char {
    '{'
}
pub fn after() -> u32 {
    1
}
";
        let o = outline(src, &Language::Rust).unwrap();
        assert!(o.contains("pub fn open() -> char { … }"), "{o}");
        assert!(o.contains("pub fn after() -> u32 { … }"), "after survived: {o}");
    }

    #[test]
    fn test_multiline_signature() {
        let src = "\
pub fn run_streaming(
    cmd: &mut Command,
    mode: FilterMode,
) -> Result<StreamResult> {
    do_work();
}
";
        let o = outline(src, &Language::Rust).unwrap();
        assert!(o.contains("cmd: &mut Command,"), "params kept: {o}");
        assert!(o.contains("-> Result<StreamResult> { … }"), "body elided: {o}");
        assert!(!o.contains("do_work"), "body dropped: {o}");
    }

    #[test]
    fn test_imports_and_plain_comments_dropped() {
        let src = "\
use std::fmt;
// a plain comment
/// kept doc
pub const N: usize = 4;
";
        let o = outline(src, &Language::Rust).unwrap();
        assert!(!o.contains("use std::fmt"), "import dropped: {o}");
        assert!(!o.contains("a plain comment"), "comment dropped: {o}");
        assert!(o.contains("/// kept doc"), "{o}");
        assert!(o.contains("pub const N: usize = 4;"), "const kept: {o}");
    }

    #[test]
    fn test_python_outline() {
        let src = "\
import os


class Greeter:
    def __init__(self, name):
        self.name = name

    def greet(self):
        print(f\"hi {self.name}\")


def top_level():
    return 42
";
        let o = outline(src, &Language::Python).unwrap();
        assert!(o.contains("class Greeter:"), "{o}");
        assert!(o.contains("def __init__(self, name):"), "method kept: {o}");
        assert!(o.contains("def greet(self):"), "{o}");
        assert!(o.contains("def top_level():"), "{o}");
        assert!(!o.contains("self.name = name"), "body dropped: {o}");
        assert!(!o.contains("print("), "body dropped: {o}");
        assert!(o.contains("import os"), "module import kept: {o}");
    }

    #[test]
    fn test_python_decorators_kept() {
        let src = "\
@dataclass
class Config:
    timeout: int = 30

    @property
    def ready(self):
        return True
";
        let o = outline(src, &Language::Python).unwrap();
        assert!(o.contains("@dataclass"), "{o}");
        assert!(o.contains("class Config:"), "{o}");
        assert!(o.contains("@property"), "method decorator kept: {o}");
        assert!(o.contains("def ready(self):"), "{o}");
        assert!(!o.contains("return True"), "body dropped: {o}");
    }

    #[test]
    fn test_unsupported_language_returns_none() {
        assert!(outline("a: 1\nb: 2\n", &Language::Data).is_none());
        assert!(outline("echo hi\n", &Language::Shell).is_none());
        assert!(outline("anything", &Language::Unknown).is_none());
    }

    #[test]
    fn test_does_not_swallow_following_items() {
        // Two top-level fns: the second must survive the first's body elision.
        let src = "fn a() {\n    work();\n}\nfn b() {\n    work();\n}\n";
        let o = outline(src, &Language::Rust).unwrap();
        assert!(o.contains("fn a() { … }"), "{o}");
        assert!(o.contains("fn b() { … }"), "{o}");
    }
}
