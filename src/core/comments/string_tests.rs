//! Strings are where a line scanner goes wrong, in two directions:
//!
//! - it thinks a string closed early, and deletes "comments" that are content;
//! - it thinks a string is still open, and keeps comments (harmless alone, but
//!   the next real string then reads inverted, which is the first problem).
//!
//! So every construct below is sandwiched between real comments: its own lines
//! must all survive, and both comments must go.

use super::*;

/// Lines that look like comment syntax of some language, or that a sloppy scan
/// would collapse. Inside a string each is plain content.
const INTERIOR: &[&str] = &[
    "// slashes",
    "/// doc slashes",
    "/* opener",
    "*/",
    "/* closed */",
    "# hash",
    "#!/bin/sh",
    "=begin",
    "=end",
    "  // indented",
    "",
    "",
    "",
    "  ",
];

fn comment_of(lang: &Language) -> &'static str {
    match lang {
        Language::Python | Language::Ruby | Language::Shell => "# real comment",
        _ => "// real comment",
    }
}

/// Returns a description of the failure, if any.
fn check_construct(lang: &Language, construct: &[&str]) -> Option<String> {
    let comment = comment_of(lang);
    let mut source = vec![comment];
    source.extend_from_slice(construct);
    source.extend([comment, "after();"]);

    let got = strip_comment_lines(&(source.join("\n") + "\n"), lang);
    let got: Vec<&str> = got.lines().collect();
    let mut want: Vec<&str> = construct.to_vec();
    want.push("after();");
    (got != want).then(|| format!("{lang:?} {:?}\n   got: {got:?}", construct[0]))
}

/// Every `(open, close)` pair wrapped around [`INTERIOR`], then `lines` as is.
fn assert_constructs(lang: Language, wrapped: &[(&str, &str)], lines: &[&[&str]]) {
    let mut failures = Vec::new();
    for (open, close) in wrapped {
        let mut construct = vec![*open];
        construct.extend_from_slice(INTERIOR);
        construct.push(close);
        failures.extend(check_construct(&lang, &construct));
    }
    for construct in lines {
        failures.extend(check_construct(&lang, construct));
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn javascript_templates() {
    for lang in [Language::JavaScript, Language::TypeScript] {
        assert_constructs(
            lang,
            &[
                ("const s = `", "`;"),
                ("f(`a`, `", "`);"),
                ("const s = `${a}", "${b}`;"),
                ("const s = `${`nested`}", "`;"),
                ("const s = `\\`", "\\``;"),
                ("const s = `\\\\\\`", "`;"),
                // A quote that closes on this line and one that never does.
                ("const q = '`'; const s = `", "`;"),
                ("const q = \"`\"; const s = `", "`;"),
                ("const q = \"\\\"\"; const s = `", "`;"),
                ("const q = \"\\\\\"; const s = `", "`;"),
                ("/* it's */ const s = `", "`;"),
                ("const s = /* ` */ `", "`;"),
                // A division is not a regex.
                ("const n = a / b; const s = `", "`;"),
                ("const n = a / b / c; const s = `", "`;"),
                // Quotes inside an interpolation belong to it.
                ("const s = `${a ? \"`\" : \"\"}", "`;"),
                ("const s = `${a['b']}", "`;"),
            ],
            &[
                &["const a = \"/* not a comment\";"],
                &["const a = '// not a comment';"],
                &["const a = \"\\\"/*\";"],
                &["const a = `/*`;"],
                &["const a = 1; // don't"],
                &["const a = 1; /* don't */"],
                &["const a = 1; // `"],
                &["const a = 1; /* ` */"],
                &["const a = b /* one */ + c /* two */;"],
                &["<p>don't</p>"],
            ],
        );
    }
}

#[test]
fn javascript_regex_literals() {
    for lang in [Language::JavaScript, Language::TypeScript] {
        assert_constructs(
            lang,
            &[
                ("x.replace(/'/g, `", "`);"),
                ("x.replace(/\"/g, `", "`);"),
                ("x.replace(/`/g, `", "`);"),
                ("x.replace(/'/g, `it's", "`);"),
                ("x.replace(/[/*]/g, `", "`);"),
                ("x.replace(/\\//g, `", "`);"),
                ("x.replace(/[/'`]/g, `", "`);"),
                ("const r = /'/; const s = `", "`;"),
                ("return /`/.test(x) ? `", "` : 0;"),
            ],
            &[
                &["const r = /\\/\\//;"],
                &["const r = /\\/*x/;"],
                &["const r = /[/*]/;"],
                &["const r = /`/;"],
                &["x.split(/'/);"],
            ],
        );
    }
}

#[test]
fn go_strings() {
    assert_constructs(
        Language::Go,
        &[
            ("s := `", "`"),
            // A backslash escapes nothing in a raw string.
            ("s := `a\\", "`"),
            ("s := `\\`; t := `", "`"),
            ("r := '`'; s := `", "`"),
            ("r := '\\''; s := `", "`"),
            ("q := \"`\"; s := `", "`"),
            ("q := \"\\\"\"; s := `", "`"),
        ],
        &[&["q := \"/*\""], &["r := '\"'"], &["x := 1 // don't"]],
    );
}

#[test]
fn rust_strings() {
    assert_constructs(
        Language::Rust,
        &[
            ("let s = \"", "\";"),
            ("let s = b\"", "\";"),
            ("let s = r\"", "\";"),
            ("let s = r#\"", "\"#;"),
            ("let s = br#\"", "\"#;"),
            ("let s = cr#\"", "\"#;"),
            // A close with too few hashes does not close.
            ("let s = r##\"\"#", "\"##;"),
            ("let s = \"\\\"", "\\\"\";"),
            ("let q = \"\\\\\"; let s = \"", "\";"),
            ("let q = '\"'; let s = \"", "\";"),
            ("let q = b'\"'; let s = \"", "\";"),
            ("let q = '\\''; let s = \"", "\";"),
            ("let q = '\\\"'; let s = \"", "\";"),
            ("let q = '\\u{22}'; let s = \"", "\";"),
            ("let q = 'あ'; let s = \"", "\";"),
            ("fn f<'a>(x: &'a str) {} let s = \"", "\";"),
            ("let r#type = 1; let s = \"", "\";"),
            ("/* \" */ let s = \"", "\";"),
        ],
        &[
            &["let a = \"/*\";"],
            &["let a = r\"/*\";"],
            &["let a = r#\"\"/*\"#;"],
            &["let a = 1; // don't"],
            &["let a = 1; /* \" */"],
            &["let a = /* /* nested */ */ 1;"],
            &["#[attr = \"//\"]"],
        ],
    );
}

#[test]
fn python_strings() {
    assert_constructs(
        Language::Python,
        &[
            ("x = \"\"\"", "\"\"\""),
            ("x = '''", "'''"),
            ("x = r\"\"\"", "\"\"\""),
            ("x = f'''", "'''"),
            ("def f(): \"\"\"", "\"\"\""),
            ("x = \"\"\"\\\"\"\"", "\"\"\""),
            ("x = \"\"\"'''", "\"\"\""),
            ("x = '''\"\"\"", "'''"),
            ("q = '\"'; x = \"\"\"", "\"\"\""),
            ("q = \"'\"; x = '''", "'''"),
            ("q = '\"\"\"'; x = '''", "'''"),
            ("q = \"\\\"\"; x = \"\"\"", "\"\"\""),
            ("q = ''; x = \"\"\"", "\"\"\""),
            ("q = \"\"; x = '''", "'''"),
        ],
        &[
            &["x = \"\"\"one line\"\"\""],
            &["x = '''one line'''"],
            &["x = \"# not a comment\""],
            &["x = 1  # don't"],
            &["x = 1  # \"\"\""],
        ],
    );
}

#[test]
fn java_strings() {
    assert_constructs(
        Language::Java,
        &[
            ("String s = \"\"\"", "\"\"\";"),
            ("String s = \"\"\"\\\"\"\"", "\"\"\";"),
            ("char q = '\"'; String s = \"\"\"", "\"\"\";"),
            ("String q = \"\\\"\"; String s = \"\"\"", "\"\"\";"),
        ],
        &[
            &["String a = \"/*\";"],
            &["char a = '\\'';"],
            &["int a = 1; // don't"],
        ],
    );
}

#[test]
fn c_and_cpp_strings() {
    for lang in [Language::C, Language::Cpp] {
        assert_constructs(
            lang,
            &[],
            &[
                &["char *a = \"/*\";"],
                &["char a = '\"';"],
                &["char a = '\\'';"],
                &["char *a = \"\\\"/*\";"],
                &["#define A \"//\""],
                &["int a; // don't"],
                &["int a; /* \" */"],
                // A backslash at the end of the line continues the string.
                &["char *a = \"x\\", "// inside\\", "/* inside\";"],
            ],
        );
    }
    assert_constructs(
        Language::Cpp,
        &[
            ("auto s = R\"(", ")\";"),
            ("auto s = u8R\"(", ")\";"),
            ("auto s = LR\"(", ")\";"),
            ("auto s = R\"x(", ")x\";"),
            // A close with the wrong delimiter does not close.
            ("auto s = R\"x()\"", ")x\";"),
            ("char q = '\"'; auto s = R\"(", ")\";"),
            ("int n = 1'000; auto s = R\"(", ")\";"),
        ],
        &[&["auto a = R\"(/*)\";"]],
    );
}

#[test]
fn ruby_strings() {
    assert_constructs(
        Language::Ruby,
        &[
            ("s = \"", "\""),
            ("s = '", "'"),
            ("s = \"\\\"", "\""),
            ("s = '\\'", "'"),
            ("q = '\"'; s = \"", "\""),
            ("q = \"'\"; s = '", "'"),
            ("s = \"#{\"nested\"}", "\""),
            ("s = \"#{a['b']}", "\""),
            ("x.gsub(/'/, \"", "\")"),
            ("x.gsub(/\"/, '", "')"),
            ("x.gsub(/[\"']/, \"", "\")"),
            ("s = <<X", "X"),
            ("s = <<'X'", "X"),
            ("s = <<\"X\"", "X"),
            ("s = <<-X", "  X"),
            ("s = <<~X", "  X"),
            ("s = <<-\"begin;\"", "  begin;"),
            ("f(<<~X, 1)", "  X"),
        ],
        &[
            &["x = 1 # don't"],
            &["x = \"# not a comment\""],
            &["a = b << c"],
            &["class << self"],
            &["puts \"#{x}\" # it's"],
            &["n = a / b"],
            &["r = /a/ # it's"],
            // A multi-line regex: `#` lines are part of the pattern.
            &["r = /a", "# inside", "  # inside", "b/x"],
            &["r =", "  / a", "# inside", "/x"],
            &["s = <<X", "  X", "# inside", "X"],
            &["a, b = <<A, <<B", "# in a", "A", "# in b", "B"],
        ],
    );
}

/// The body follows the line the string closes on, so the scan stops judging.
#[test]
fn a_heredoc_opened_inside_a_string_keeps_the_rest_of_the_file() {
    for (lang, source) in [
        (
            Language::Ruby,
            "run(\"#{<<-X}\", 1)\n  #{pre}\n  # text\nX\n# kept too\nafter\n",
        ),
        (
            Language::Shell,
            "s=\"$(cat <<X)\"\n# text\nX\n# kept too\nafter\n",
        ),
    ] {
        assert_eq!(strip_comment_lines(source, &lang), source, "{lang:?}");
    }
    // A string merely mentioning `<<` is not one.
    assert_eq!(
        strip_comment_lines("s = \"a << b\"\n# gone\nafter\n", &Language::Ruby),
        "s = \"a << b\"\nafter\n"
    );
}

#[test]
fn ruby_text_after_end_marker_is_data() {
    let source = "# real comment\nputs DATA.read\n__END__\n# data\n// data\n";
    assert_eq!(
        strip_comment_lines(source, &Language::Ruby),
        "puts DATA.read\n__END__\n# data\n// data\n"
    );
}

#[test]
fn shell_strings() {
    assert_constructs(
        Language::Shell,
        &[
            ("s=\"", "\""),
            ("s='", "'"),
            ("s=\"\\\"", "\""),
            // A backslash escapes nothing in single quotes.
            ("s='a\\", "'"),
            ("s='\\'; t='", "'"),
            // ...except in $'...', where it does.
            ("s=$'it\\'s", "'"),
            ("echo it\\'s; s=\"", "\""),
            ("echo \\\"; s=\"", "\""),
            ("q='\"'; s=\"", "\""),
            ("q=\"'\"; s='", "'"),
            ("echo $#; s=\"", "\""),
            ("echo ${#a}; s=\"", "\""),
            ("echo ${a#b}; s=\"", "\""),
            ("echo \"$(echo \"nested\")\"; s=\"", "\""),
            ("cat <<EOF", "EOF"),
            ("cat << EOF", "EOF"),
            ("cat <<'EOF'", "EOF"),
            ("cat <<\"EOF\"", "EOF"),
            ("cat <<\\EOF", "EOF"),
            ("cat <<EOF | grep x", "EOF"),
            ("x=$(cat <<EOF", "EOF"),
            ("cat <<-EOF", "\tEOF"),
        ],
        &[
            &["echo hi # don't"],
            &["echo hi;# don't"],
            &["echo a#b"],
            &["(# don't", "echo hi)"],
            &["echo hi |# don't", "cat"],
            &["cat <<< \"here string\""],
            // Not the terminator: indented without <<-, or only a prefix.
            &["cat <<EOF", "  EOF", "# inside", "EOFX", "# inside", "EOF"],
            &["cat <<-EOF", "  EOF", "# inside", "\tEOF"],
            &["cat <<A <<B", "# in a", "A", "# in b", "B"],
        ],
    );
}

#[test]
fn a_real_comment_right_after_a_string_on_the_same_line_is_kept_with_it() {
    let source = "const s = `\n`; // tail\n// real comment\nafter();\n";
    assert_eq!(
        strip_comment_lines(source, &Language::JavaScript),
        "const s = `\n`; // tail\nafter();\n"
    );
}

#[test]
fn blank_lines_collapse_again_once_a_string_closes() {
    let source = "x = \"\"\"\n\n\n\"\"\"\n\n\n\ny = 1\n";
    assert_eq!(
        strip_comment_lines(source, &Language::Python),
        "x = \"\"\"\n\n\n\"\"\"\n\ny = 1\n"
    );
}

/// Any order of constructs: a state that leaks out of one shows up in the next.
/// The generator is a fixed LCG, so a failure reproduces.
#[test]
fn mixed_constructs_do_not_leak_state_into_each_other() {
    type Pieces = &'static [&'static [&'static str]];
    let languages: &[(Language, Pieces)] = &[
        (
            Language::JavaScript,
            &[
                &["const s = `", "// in", "/* in", "`;"],
                &["const q = \"/*\";"],
                &["const q = '`';"],
                &["x.replace(/'/g, \"\");"],
                &["const n = a / b;"],
                &["const a = b; // don't"],
            ],
        ),
        (
            Language::Rust,
            &[
                &["let s = r#\"", "// in", "\"#;"],
                &["let s = \"", "/* in", "\";"],
                &["let q = '\"';"],
                &["fn f<'a>() {}"],
                &["let a = 1; // \""],
            ],
        ),
        (
            Language::Python,
            &[
                &["x = \"\"\"", "# in", "'''", "\"\"\""],
                &["x = '''", "# in", "\"\"\"", "'''"],
                &["q = '\"\"\"'"],
                &["q = \"'''\""],
                &["a = 1  # don't"],
            ],
        ),
        (
            Language::Shell,
            &[
                &["cat <<EOF", "# in", "don't", "EOF"],
                &["s=\"", "# in", "\""],
                &["s='", "# in", "\"", "'"],
                &["echo $#"],
                &["echo hi # don't"],
            ],
        ),
    ];

    let mut seed: u32 = 0x5EED;
    let mut next = |n: usize| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 16) as usize % n
    };
    for (lang, pieces) in languages {
        let comment = comment_of(lang);
        for _ in 0..2_000 {
            let (mut source, mut want) = (String::new(), String::new());
            for _ in 0..1 + next(6) {
                source.push_str(comment);
                source.push('\n');
                for line in pieces[next(pieces.len())] {
                    source.push_str(line);
                    source.push('\n');
                    want.push_str(line);
                    want.push('\n');
                }
            }
            assert_eq!(
                strip_comment_lines(&source, lang),
                want,
                "source:\n{source}"
            );
        }
    }
}

/// `cat` runs this on whatever is there, so a hostile line must not take
/// quadratic time. Each shape keeps one search failing at the end of the line.
#[test]
fn very_long_lines_stay_linear() {
    let cases = [
        (Language::JavaScript, "=/[".repeat(500_000)),
        (Language::JavaScript, "`${".repeat(500_000)),
        (Language::JavaScript, "\"".repeat(1_000_001)),
        (Language::Ruby, "(/[".repeat(500_000)),
        (Language::Ruby, "<<".repeat(500_000)),
        (Language::Ruby, "\"<<".repeat(300_000)),
        (Language::Cpp, "R\"".repeat(500_000)),
        (Language::Rust, "r#".repeat(500_000)),
        (Language::Rust, "/*".repeat(500_000)),
        (Language::Shell, "$'".repeat(500_000)),
    ];
    for (lang, line) in &cases {
        let source = format!("{line}\n");
        assert_eq!(strip_comment_lines(&source, lang), source, "{lang:?}");
    }
}
