use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    // Where wget and BusyBox state a server's answer. A bare number will not do:
    // `500` also appears in host names, paths, sizes and ports.
    static ref HTTP_STATUS_RE: Regex =
        Regex::new(r"(?:\bERROR |awaiting response\.\.\. |HTTP/\d(?:\.\d)? )(\d{3})\b:?[ ]*(.*)")
            .unwrap();
    // `-nv`: 2026-01-01 10:00:00 URL:http://x/a [6/6] -> "dir/a" [1]
    static ref TERSE_SAVED_RE: Regex = Regex::new(r#"URL:.* -> "(.+)" \[\d+\]$"#).unwrap();
}

/// Short options whose value is the rest of the argument or the next one.
const SHORT_OPTIONS_WITH_VALUE: &str = "aABDeIilOoPQRtTUwXY";
const MAX_FILES_SHOWN: usize = 5;
const MAX_REASON_CHARS: usize = 200;

/// Whether wget was asked to write the page to stdout, in any spelling.
pub fn writes_to_stdout(args: &[String]) -> bool {
    option_value(args, 'O', "output-document") == Some("-")
}

/// Compact wget - strips progress bars, shows only result
pub fn run(url: &str, args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("wget: {}", url);
    }

    let result = exec_capture(untranslated_wget().arg(url).args(args))?;

    let raw_output = format!("{}\n{}", result.stderr, result.stdout);
    let label = target_label(url, args);

    if result.success() {
        let msg = if args.iter().any(|a| a == "--spider") {
            format!("{} ok | exists", label)
        } else {
            let files = saved_files(&result.stderr, url, args);
            format!("{} ok | {}", label, describe_files(&files))
        };
        println!("{}", msg);
        timer.track(&format!("wget {}", url), "bdo wget", &raw_output, &msg);
    } else {
        let mut msg = format!(
            "{} FAILED: {}",
            label,
            parse_error(&result.stderr, result.exit_code)
        );
        // With several URLs, some can succeed before the run fails.
        let files = announced_files(&result.stderr);
        if !files.is_empty() {
            msg.push_str(&format!(" | saved: {}", describe_files(&files)));
        }
        println!("{}", msg);
        timer.track(&format!("wget {}", url), "bdo wget", &raw_output, &msg);
        return Ok(result.exit_code);
    }

    Ok(0)
}

/// Run wget with its page on stdout, and show the page. `args` holds the `-O -`.
pub fn run_stdout(url: &str, args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("wget: {} -> stdout", url);
    }

    let result = exec_capture(untranslated_wget().arg(url).args(args))?;
    let label = target_label(url, args);

    if result.success() {
        let lines: Vec<&str> = result.stdout.lines().collect();
        let total = lines.len();

        let mut rtk_output = String::new();
        if result.stdout.contains('\0') {
            rtk_output.push_str(&format!(
                "{} ok | binary data — use `bdo proxy wget -O - URL` to get it\n",
                label
            ));
        } else if total > 20 {
            rtk_output.push_str(&format!(
                "{} ok | {} lines | {}\n",
                label,
                total,
                format_size(result.stdout.len() as u64)
            ));
            rtk_output.push_str("first 10 lines:\n");
            for line in lines.iter().take(10) {
                rtk_output.push_str(&format!("{}\n", truncate_line(line, 100)));
            }
            rtk_output.push_str(&format!(
                "... +{} more lines — use `bdo proxy wget -O - URL` for all\n",
                total - 10
            ));
        } else {
            rtk_output.push_str(&format!("{} ok | {} lines\n", label, total));
            for line in &lines {
                rtk_output.push_str(&format!("{}\n", line));
            }
        }
        print!("{}", rtk_output);
        timer.track(
            &format!("wget -O - {}", url),
            "bdo wget -o",
            &result.stdout,
            &rtk_output,
        );
    } else {
        let error = parse_error(&result.stderr, result.exit_code);
        let msg = format!("{} FAILED: {}", label, error);
        println!("{}", msg);
        timer.track(
            &format!("wget -O - {}", url),
            "bdo wget -o",
            &result.stderr,
            &msg,
        );
        return Ok(result.exit_code);
    }

    Ok(0)
}

/// wget whose messages can be parsed. `LANGUAGE` changes only the messages:
/// `LC_ALL` could also change how wget names a file it saves.
fn untranslated_wget() -> std::process::Command {
    let mut cmd = resolved_command("wget");
    cmd.env("LANGUAGE", "C");
    cmd
}

/// The value of an option, however it is spelled: `-O x`, `-Ox`, `-qO-`, `-nvO x`,
/// `--output-document x` or `--output-document=x`.
fn option_value<'a>(args: &'a [String], short: char, long: &str) -> Option<&'a str> {
    let mut skip_next = false;
    for (i, arg) in args.iter().enumerate() {
        if std::mem::take(&mut skip_next) {
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--") {
            if rest == long {
                return args.get(i + 1).map(String::as_str);
            }
            if let Some(value) = rest
                .strip_prefix(long)
                .and_then(|after| after.strip_prefix('='))
            {
                return Some(value);
            }
            continue;
        }
        let Some(cluster) = arg.strip_prefix('-') else {
            continue;
        };
        let mut chars = cluster.char_indices();
        while let Some((at, option)) = chars.next() {
            if option == 'n' {
                // `-nv`, `-nc`: two letters, one option.
                chars.next();
                continue;
            }
            if SHORT_OPTIONS_WITH_VALUE.contains(option) {
                let inline = &cluster[at + option.len_utf8()..];
                if option == short {
                    return if inline.is_empty() {
                        args.get(i + 1).map(String::as_str)
                    } else {
                        Some(inline)
                    };
                }
                skip_next = inline.is_empty();
                break;
            }
        }
    }
    None
}

/// The URLs in `args`, past the first one, which reaches `run` separately.
fn other_urls(args: &[String]) -> impl Iterator<Item = &str> {
    args.iter()
        .map(String::as_str)
        .filter(|arg| !arg.starts_with('-') && arg.contains("://"))
}

/// What a result is about. One URL's name would misreport a run over several.
fn target_label(url: &str, args: &[String]) -> String {
    match other_urls(args).count() {
        0 => compact_url(url),
        others => format!("{} URLs", others + 1),
    }
}

/// The name between the quotes wget puts around a file.
fn quoted_name(line: &str) -> Option<String> {
    // Use char-based parsing to handle Unicode properly
    let chars: Vec<char> = line.chars().collect();
    let mut start_idx = None;
    let mut end_idx = None;

    for (i, c) in chars.iter().enumerate() {
        if matches!(*c, '«' | '‘') || (*c == '\'' && start_idx.is_none()) {
            start_idx = Some(i);
        }
        if matches!(*c, '»' | '’') || (*c == '\'' && start_idx.is_some()) {
            end_idx = Some(i);
        }
    }

    let (s, e) = (start_idx?, end_idx?);
    if e > s + 1 {
        let filename: String = chars[s + 1..e].iter().collect();
        Some(filename.trim().to_string())
    } else {
        None
    }
}

/// The files wget says it wrote.
fn announced_files(stderr: &str) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for line in stderr.lines() {
        // French: Sauvegarde en : « filename »
        let name = if line.contains("Sauvegarde en") || line.contains("Saving to") {
            quoted_name(line)
        } else {
            TERSE_SAVED_RE.captures(line).map(|c| c[1].to_string())
        };
        if let Some(name) = name {
            if !files.contains(&name) {
                files.push(name);
            }
        }
    }
    files
}

fn saved_files(stderr: &str, url: &str, args: &[String]) -> Vec<String> {
    if let Some(name) = option_value(args, 'O', "output-document") {
        return vec![name.to_string()];
    }
    let announced = announced_files(stderr);
    if !announced.is_empty() {
        return announced;
    }
    // `-q`, `-o` or an unknown wget: named as wget names them.
    let prefix = option_value(args, 'P', "directory-prefix");
    std::iter::once(url)
        .chain(other_urls(args))
        .map(|url| match prefix {
            Some(dir) => format!("{}/{}", dir.trim_end_matches('/'), file_name_of(url)),
            None => file_name_of(url),
        })
        .collect()
}

/// The last part of the URL's path, query included: wget keeps it in the name.
/// `https://example.com` has no path at all.
fn file_name_of(url: &str) -> String {
    let without_scheme = url.rsplit("://").next().unwrap_or(url);
    let name = without_scheme
        .split_once('/')
        .map(|(_, path)| path.rsplit('/').next().unwrap_or(""))
        .unwrap_or("");
    if name.is_empty() || name.starts_with('?') {
        format!("index.html{}", name)
    } else {
        name.to_string()
    }
}

fn describe_files(files: &[String]) -> String {
    if let [file] = files {
        return format!("{} | {}", file, file_size(file));
    }
    let shown: Vec<String> = files
        .iter()
        .take(MAX_FILES_SHOWN)
        .map(|file| format!("{} {}", file, file_size(file)))
        .collect();
    let more = match files.len().saturating_sub(MAX_FILES_SHOWN) {
        0 => String::new(),
        more => format!(", +{} more", more),
    };
    format!("{} files: {}{}", files.len(), shown.join(", "), more)
}

fn file_size(filename: &str) -> String {
    std::fs::metadata(filename).map_or("?".to_string(), |m| format_size(m.len()))
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn compact_url(url: &str) -> String {
    // Remove protocol
    let without_proto = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);

    // Truncate if too long
    let chars: Vec<char> = without_proto.chars().collect();
    if chars.len() <= 50 {
        without_proto.to_string()
    } else {
        let prefix: String = chars[..25].iter().collect();
        let suffix: String = chars[chars.len() - 20..].iter().collect();
        format!("{}...{}", prefix, suffix)
    }
}

/// What GNU wget's exit codes mean, for when `-q` or `-o` leaves stderr empty.
fn exit_code_meaning(exit_code: i32) -> String {
    let meaning = match exit_code {
        2 => "bad command line",
        3 => "file error",
        4 => "network failure",
        5 => "SSL verification failed",
        6 => "authentication failed",
        7 => "protocol error",
        8 => "the server returned an error",
        _ => "error",
    };
    format!("{} (wget exit {})", meaning, exit_code)
}

fn parse_error(stderr: &str, exit_code: i32) -> String {
    // `--date--  URL` lines repeat the URL, which can contain any of these words.
    let messages: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("--"))
        .collect();

    // Exit 2 is a mistyped command, whose message quotes the mistake.
    if exit_code != 2 {
        if let Some(known) = known_failure(&messages) {
            return known;
        }
    }

    // The line that states the error. wget's output opens with progress such
    // as "Resolving…", so without one the last line is the best guess.
    let stated = messages.iter().find(|l| {
        let lower = l.to_lowercase();
        lower.contains("error") || lower.contains("failed") || lower.contains("wget:")
    });
    match stated.or(messages.last()) {
        Some(line) if line.chars().count() > MAX_REASON_CHARS => {
            let t: String = line.chars().take(MAX_REASON_CHARS).collect();
            format!("{}...", t)
        }
        Some(line) => line.to_string(),
        None => exit_code_meaning(exit_code),
    }
}

fn known_failure(messages: &[&str]) -> Option<String> {
    // Every failure the server gave: an earlier `302` is not one, and each URL
    // of several can have its own.
    let mut statuses: Vec<(&str, &str)> = Vec::new();
    for line in messages {
        for found in HTTP_STATUS_RE.captures_iter(line) {
            let (Some(code), Some(reason)) = (found.get(1), found.get(2)) else {
                continue;
            };
            let code = code.as_str();
            if code >= "400" && !statuses.iter().any(|(seen, _)| *seen == code) {
                statuses.push((code, reason.as_str().trim_end_matches('.')));
            }
        }
    }
    if !statuses.is_empty() {
        let statuses: Vec<String> = statuses
            .iter()
            .map(|(code, reason)| format!("{} {}", code, reason).trim().to_string())
            .collect();
        return Some(statuses.join(", "));
    }

    let any = |needle: &str| messages.iter().any(|l| l.contains(needle));
    if any("Connection refused") {
        return Some("Connection refused".to_string());
    }
    if any("unable to resolve") || any("Name or service not known") {
        return Some("DNS lookup failed".to_string());
    }
    if any("timed out") {
        return Some("Connection timed out".to_string());
    }
    // These lines name the host or the next URL, which can contain the words.
    let about_tls = messages.iter().any(|l| {
        !(l.starts_with("Resolving ") || l.starts_with("Connecting to ") || l.starts_with("Location: "))
            && (l.contains("SSL") || l.contains("certificate"))
    });
    about_tls.then(|| "SSL/TLS error".to_string())
}

fn truncate_line(line: &str, max: usize) -> String {
    if line.len() <= max {
        line.to_string()
    } else {
        let t: String = line.chars().take(max.saturating_sub(3)).collect();
        format!("{}...", t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_url_strips_protocol() {
        assert_eq!(compact_url("https://example.com/file.zip"), "example.com/file.zip");
        assert_eq!(compact_url("http://example.com/file.zip"), "example.com/file.zip");
    }

    #[test]
    fn test_compact_url_truncates_long_url() {
        let long = "https://example.com/very/long/path/that/exceeds/fifty/characters/file.zip";
        let result = compact_url(long);
        assert!(result.contains("..."), "Long URL should be truncated with ...");
        assert!(result.len() < long.len());
    }

    #[test]
    fn test_compact_url_short_unchanged() {
        let short = "https://x.com/f";
        assert_eq!(compact_url(short), "x.com/f");
    }

    #[test]
    fn test_format_size_zero() {
        assert_eq!(format_size(0), "0B");
    }

    #[test]
    fn test_format_size_bytes() {
        assert_eq!(format_size(512), "512B");
    }

    #[test]
    fn test_format_size_kilobytes() {
        let result = format_size(2048);
        assert!(result.ends_with("KB"), "Expected KB, got {}", result);
    }

    #[test]
    fn test_format_size_megabytes() {
        let result = format_size(2 * 1024 * 1024);
        assert!(result.ends_with("MB"), "Expected MB, got {}", result);
    }

    #[test]
    fn test_parse_error_404() {
        assert_eq!(
            parse_error("HTTP request sent, awaiting response... 404 Not Found", 8),
            "404 Not Found"
        );
        assert_eq!(parse_error("ERROR 404: Not Found.", 8), "404 Not Found");
        // BusyBox.
        assert_eq!(
            parse_error("wget: server returned error: HTTP/1.1 404 Not Found", 1),
            "404 Not Found"
        );
    }

    #[test]
    fn test_parse_error_dns() {
        assert_eq!(
            parse_error("unable to resolve host example.com", 4),
            "DNS lookup failed"
        );
    }

    #[test]
    fn test_parse_error_ssl() {
        assert_eq!(
            parse_error("SSL certificate verification failed", 5),
            "SSL/TLS error"
        );
    }

    #[test]
    fn test_parse_error_without_a_message_says_what_the_exit_code_means() {
        assert_eq!(
            parse_error("", 8),
            "the server returned an error (wget exit 8)"
        );
        assert_eq!(parse_error("", 99), "error (wget exit 99)");
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn test_option_value_reads_every_spelling() {
        for (args, expected) in [
            (vec!["-O", "x"], Some("x")),
            (vec!["-Ox"], Some("x")),
            (vec!["-O-"], Some("-")),
            (vec!["-qO-"], Some("-")),
            (vec!["-qO", "-"], Some("-")),
            (vec!["-nvO-"], Some("-")),
            (vec!["--output-document=x"], Some("x")),
            (vec!["--output-document", "x"], Some("x")),
            (vec!["-o", "log", "-O", "-"], Some("-")),
            (vec!["--tries=1", "-O", "x"], Some("x")),
            (vec![], None),
            (vec!["-o", "log"], None),
            (vec!["--output-file=log"], None),
            // These `O`s belong to another option's value.
            (vec!["-UOpera"], None),
            (vec!["-U", "-O"], None),
            (vec!["--output-documents=x"], None),
        ] {
            assert_eq!(
                option_value(&strings(&args), 'O', "output-document"),
                expected,
                "{args:?}"
            );
        }
    }

    #[test]
    fn test_truncate_line_short() {
        assert_eq!(truncate_line("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_line_exact() {
        assert_eq!(truncate_line("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_line_long() {
        let result = truncate_line("hello world this is long", 10);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 10);
    }

    #[test]
    fn test_saved_files_from_the_output_flag() {
        assert_eq!(
            saved_files("", "https://example.com/x", &strings(&["-O", "myfile.zip"])),
            ["myfile.zip"]
        );
    }

    #[test]
    fn test_saved_files_from_the_url() {
        for (url, name) in [
            ("https://example.com/file.tar.gz", "file.tar.gz"),
            ("https://example.com/", "index.html"),
            ("https://example.com", "index.html"),
            ("https://example.com/f.zip?x=1", "f.zip?x=1"),
            ("https://example.com/?x=1", "index.html?x=1"),
        ] {
            assert_eq!(saved_files("", url, &[]), [name], "{url}");
        }
    }

    #[test]
    fn test_saved_files_from_the_url_honour_the_directory() {
        assert_eq!(
            saved_files("", "https://example.com/a", &strings(&["-q", "-P", "sub/"])),
            ["sub/a"]
        );
    }
}
