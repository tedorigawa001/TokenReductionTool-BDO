//! `bdo wget` against a stand-in, and against real wget and a local server where
//! wget is installed. The stand-in pins what bdo does with each message; the real
//! one checks that those are the messages wget writes. Unix only: the stand-in is
//! a shell script.
#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fake {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Fake {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        let script = bin.join("wget");
        fs::write(
            &script,
            "#!/bin/sh\n\
             printf 'LANGUAGE=%s\\n' \"$LANGUAGE\" >> \"$ROOT/log\"\n\
             for a in \"$@\"; do printf '%s\\n' \"$a\" >> \"$ROOT/log\"; done\n\
             [ -f \"$ROOT/stderr\" ] && cat \"$ROOT/stderr\" >&2\n\
             [ -f \"$ROOT/stdout\" ] && cat \"$ROOT/stdout\"\n\
             exit \"${FAKE_EXIT:-0}\"\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        Self { _tmp: tmp, root }
    }

    fn stderr(self, text: &str) -> Self {
        fs::write(self.root.join("stderr"), text).unwrap();
        self
    }

    fn stdout(self, text: &str) -> Self {
        fs::write(self.root.join("stdout"), text).unwrap();
        self
    }

    fn bdo(&self, exit: i32, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bdo"))
            .arg("wget")
            .args(args)
            .current_dir(&self.root)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("ROOT", &self.root)
            .env("FAKE_EXIT", exit.to_string())
            .env("BDO_DB_PATH", self.root.join("track.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .output()
            .unwrap()
    }

    fn log(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("log"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn reports_the_saved_file_with_either_kind_of_quote() {
    for line in ["Saving to: 'dl.bin'", "Saving to: ‘dl.bin’"] {
        let fake = Fake::new().stderr(&format!("{line}\n"));
        fs::write(fake.root.join("dl.bin"), "data").unwrap();
        let out = fake.bdo(0, &["https://example.com/download"]);
        assert_eq!(
            stdout(&out),
            "example.com/download ok | dl.bin | 4B\n",
            "{line}"
        );
    }
}

#[test]
fn asks_for_untranslated_messages_without_changing_the_locale() {
    let fake = Fake::new();
    fake.bdo(0, &["https://example.com/a"]);
    assert_eq!(fake.log()[0], "LANGUAGE=C");
}

#[test]
fn names_the_file_from_the_url_when_wget_is_quiet() {
    for (url, name) in [
        ("https://example.com/download", "download"),
        ("https://example.com/f.zip?x=1", "f.zip?x=1"),
        ("https://example.com/", "index.html"),
        ("https://example.com", "index.html"),
    ] {
        let out = Fake::new().bdo(0, &[url, "-q"]);
        assert!(
            stdout(&out).contains(&format!("ok | {name} |")),
            "{url}: {out:?}"
        );
    }
}

#[test]
fn passes_the_output_file_and_other_arguments_on() {
    let fake = Fake::new();
    fs::write(fake.root.join("x.bin"), "data").unwrap();
    let out = fake.bdo(0, &["https://example.com/a", "-O", "x.bin", "--tries=1"]);
    assert_eq!(stdout(&out), "example.com/a ok | x.bin | 4B\n");
    assert_eq!(
        fake.log()[1..],
        ["https://example.com/a", "-O", "x.bin", "--tries=1"]
    );
}

/// `500` in a host name, or `404` in a path, says nothing about the server's answer.
#[test]
fn failure_does_not_read_a_status_out_of_the_url() {
    let fake = Fake::new().stderr(
        "--2026-11-04--  https://host500.example/a404\n\
         Resolving host500.example... failed: Name or service not known.\n\
         wget: unable to resolve host address 'host500.example'\n",
    );
    let out = fake.bdo(4, &["https://host500.example/a404"]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(
        stdout(&out),
        "host500.example/a404 FAILED: DNS lookup failed\n"
    );
}

#[test]
fn failure_reports_the_server_status() {
    for (line, expected) in [
        (
            "HTTP request sent, awaiting response... 404 Not Found",
            "404 Not Found",
        ),
        ("2026-11-05 08:14:22 ERROR 403: Forbidden.", "403 Forbidden"),
        (
            "wget: server returned error: HTTP/1.1 401 Unauthorized",
            "401 Unauthorized",
        ),
        (
            "ERROR 500: Internal Server Error.",
            "500 Internal Server Error",
        ),
        (
            "2026-11-05 08:14:22 ERROR 503: Service Unavailable.",
            "503 Service Unavailable",
        ),
    ] {
        let fake = Fake::new().stderr(&format!(
            "Connecting to example.com:5000... connected.\n{line}\n"
        ));
        let out = fake.bdo(8, &["https://example.com/a"]);
        assert_eq!(out.status.code(), Some(8));
        assert_eq!(stdout(&out), format!("example.com/a FAILED: {expected}\n"));
    }
}

#[test]
fn failure_prefers_the_line_stating_the_error_over_progress() {
    let fake = Fake::new().stderr("Resolving example.com... 1.2.3.4\nwget: something odd\n");
    let out = fake.bdo(1, &["https://example.com/a"]);
    assert_eq!(stdout(&out), "example.com/a FAILED: wget: something odd\n");

    let fake = Fake::new().stderr("Resolving example.com... 1.2.3.4\nthen it stopped\n");
    let out = fake.bdo(1, &["https://example.com/a"]);
    assert_eq!(stdout(&out), "example.com/a FAILED: then it stopped\n");
}

/// wget names the host on these lines, and a host can contain the word.
#[test]
fn failure_does_not_read_tls_trouble_out_of_a_host_name() {
    let fake = Fake::new().stderr(
        "Resolving certificate.example (certificate.example)... 1.2.3.4\n\
         Connecting to certificate.example|1.2.3.4|:443... failed: No route to host.\n",
    );
    let out = fake.bdo(4, &["https://certificate.example/a"]);
    assert_eq!(
        stdout(&out),
        "certificate.example/a FAILED: Connecting to certificate.example|1.2.3.4|:443... \
         failed: No route to host.\n"
    );
}

#[test]
fn stdout_mode_shows_a_short_page_whole() {
    let fake = Fake::new().stdout("one\ntwo\n");
    let out = fake.bdo(0, &["https://example.com/a", "-O", "-"]);
    assert_eq!(stdout(&out), "example.com/a ok | 2 lines\none\ntwo\n");
}

#[test]
fn stdout_mode_says_how_to_get_the_rest_of_a_long_page() {
    let page: String = (1..=30).map(|i| format!("line{i}\n")).collect();
    let out = Fake::new()
        .stdout(&page)
        .bdo(0, &["https://example.com/a", "-O", "-"]);
    let text = stdout(&out);
    assert!(text.contains("line10\n"), "{text}");
    assert!(!text.contains("line11\n"), "{text}");
    assert!(text.contains("+20 more lines"), "{text}");
    assert!(text.contains("bdo proxy wget"), "{text}");
}

#[test]
fn stdout_mode_does_not_print_binary_data() {
    let out = Fake::new()
        .stdout("PNG\0\x01\x02 not text\n")
        .bdo(0, &["https://example.com/a.png", "-O", "-"]);
    let text = stdout(&out);
    assert!(
        text.contains("example.com/a.png ok | binary data"),
        "{text}"
    );
    assert!(text.contains("bdo proxy wget"), "{text}");
    assert!(!text.contains("not text"), "{text}");
}

/// `-q` would silence wget, leaving nothing but "Unknown error" to report.
#[test]
fn stdout_mode_keeps_the_reason_for_a_failure() {
    let fake = Fake::new().stderr("ERROR 404: Not Found.\n");
    let out = fake.bdo(8, &["https://example.com/a", "-O", "-"]);
    assert_eq!(out.status.code(), Some(8));
    assert_eq!(stdout(&out), "example.com/a FAILED: 404 Not Found\n");
    assert!(!fake.log().contains(&"-q".to_string()));
}

// ---- real wget, where installed ----

fn wget_installed() -> bool {
    let found = Command::new("wget").arg("--version").output().is_ok();
    if !found {
        eprintln!("wget is not installed: skipping");
    }
    found
}

fn answer(path: &str) -> String {
    let plain = |status: &str, headers: &str, body: &str| {
        format!(
            "HTTP/1.0 {status}\r\n{headers}Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
    };
    match path.split('?').next().unwrap_or(path) {
        "/to-gone" => plain("302 Found", "Location: /gone\r\n", ""),
        "/SSL-loop" => plain("302 Found", "Location: /SSL-loop\r\n", ""),
        "/gone" => plain("404 Not Found", "", ""),
        "/forbidden" => plain("403 Forbidden", "", ""),
        "/login" => plain(
            "401 Unauthorized",
            "WWW-Authenticate: Basic realm=\"x\"\r\n",
            "",
        ),
        "/broken" => plain("500 Internal Server Error", "", ""),
        "/busy" => plain("503 Service Unavailable", "", ""),
        "/empty" => plain("200 OK", "", ""),
        "/slow" => {
            std::thread::sleep(std::time::Duration::from_secs(3));
            plain("200 OK", "", "late\n")
        }
        "/long" => {
            let page: String = (1..=30).map(|i| format!("line{i}\n")).collect();
            plain("200 OK", "", &page)
        }
        "/site" => plain(
            "200 OK",
            "Content-Type: text/html\r\n",
            "<a href=\"/a.html\">a</a> <a href=\"/b.html\">b</a>",
        ),
        _ => plain("200 OK", "", "hello\n"),
    }
}

/// A server on a thread of its own, answering by path until the test ends.
fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..read]).into_owned();
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
            let _ = stream.write_all(answer(&path).as_bytes());
        }
    });
    address
}

fn real_bdo(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bdo"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", cwd)
        .env("BDO_DB_PATH", cwd.join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1")
        .env("no_proxy", "127.0.0.1")
        .output()
        .unwrap()
}

/// `bdo wget` in a new directory, which is returned for checking what was saved.
fn real_wget(args: &[&str]) -> (tempfile::TempDir, Output) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let mut command = vec!["wget"];
    command.extend(args);
    let out = real_bdo(&dir, &command);
    (tmp, out)
}

fn in_dir(tmp: &tempfile::TempDir, file: &str) -> PathBuf {
    tmp.path().join(file)
}

/// The line bdo prints, without the server's changing address.
fn result_line(out: &Output, base: &str) -> String {
    stdout(out).replace(&base.replace("http://", ""), "SERVER")
}

fn assert_saved(args: &[&str], base: &str, expected: &str, file: &str) {
    let (tmp, out) = real_wget(args);
    assert!(out.status.success(), "{args:?}: {out:?}");
    assert_eq!(result_line(&out, base), expected, "{args:?}");
    assert_eq!(
        fs::read_to_string(in_dir(&tmp, file)).unwrap(),
        "hello\n",
        "{args:?}"
    );
}

#[test]
fn real_wget_reports_the_file_however_it_was_named() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    let url = format!("{base}/a.txt");
    let url = url.as_str();
    for (args, name) in [
        (vec![url], "a.txt"),
        (vec![url, "-O", "b.txt"], "b.txt"),
        (vec![url, "-Ob.txt"], "b.txt"),
        (vec![url, "--output-document=b.txt"], "b.txt"),
        (vec![url, "--output-document", "b.txt"], "b.txt"),
        (vec![url, "-qOb.txt"], "b.txt"),
        (vec![url, "-q"], "a.txt"),
        (vec![url, "-nv"], "a.txt"),
        (vec![url, "-o", "w.log"], "a.txt"),
        (vec![url, "--tries=1", "--timeout=5"], "a.txt"),
    ] {
        let expected = format!("SERVER/a.txt ok | {name} | 6B\n");
        assert_saved(&args, &base, &expected, name);
    }
}

/// `-P` changes where the file is, which matters when wget does not say so.
#[test]
fn real_wget_reports_the_directory_the_file_went_to() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    let url = format!("{base}/a.txt");
    let url = url.as_str();
    for args in [
        vec![url, "-P", "sub"],
        vec![url, "-Psub"],
        vec![url, "--directory-prefix=sub"],
        vec![url, "-q", "-P", "sub"],
        vec![url, "-nv", "-P", "sub"],
        vec![url, "-o", "w.log", "-P", "sub"],
    ] {
        assert_saved(
            &args,
            &base,
            "SERVER/a.txt ok | sub/a.txt | 6B\n",
            "sub/a.txt",
        );
    }
}

#[test]
fn real_wget_names_files_as_wget_does() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    let quiet = format!("{base}/page.cgi?a=1");
    assert_saved(
        &[&quiet, "-q"],
        &base,
        "SERVER/page.cgi?a=1 ok | page.cgi?a=1 | 6B\n",
        "page.cgi?a=1",
    );
    let no_extension = format!("{base}/noextension");
    assert_saved(
        &[&no_extension],
        &base,
        "SERVER/noextension ok | noextension | 6B\n",
        "noextension",
    );

    // A name already taken: wget picks another, and that is the one to report.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    fs::write(dir.join("a.txt"), "older, and a different length\n").unwrap();
    let url = format!("{base}/a.txt");
    let out = real_bdo(&dir, &["wget", &url]);
    assert_eq!(result_line(&out, &base), "SERVER/a.txt ok | a.txt.1 | 6B\n");

    // `-nv` words it differently, and the URL alone would give the wrong file.
    let out = real_bdo(&dir, &["wget", &url, "-nv"]);
    assert_eq!(result_line(&out, &base), "SERVER/a.txt ok | a.txt.2 | 6B\n");
}

#[test]
fn real_wget_tells_an_empty_file_from_an_unknown_size() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    let (_tmp, out) = real_wget(&[&format!("{base}/empty")]);
    assert_eq!(result_line(&out, &base), "SERVER/empty ok | empty | 0B\n");
}

/// A file that was not written must not be reported as if it had been.
#[test]
fn real_wget_spider_reports_existence_and_saves_nothing() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    let url = format!("{base}/s.txt");
    for args in [
        vec![url.as_str(), "--spider"],
        vec![url.as_str(), "--spider", "-q"],
    ] {
        let (tmp, out) = real_wget(&args);
        assert!(out.status.success(), "{out:?}");
        assert_eq!(result_line(&out, &base), "SERVER/s.txt ok | exists\n");
        assert!(!in_dir(&tmp, "s.txt").exists());
    }
    let gone = format!("{base}/gone");
    let (_tmp, out) = real_wget(&[&gone, "--spider"]);
    assert_eq!(out.status.code(), Some(8), "{out:?}");
    assert_eq!(
        result_line(&out, &base),
        "SERVER/gone FAILED: 404 Not Found\n"
    );
}

/// Every way of asking for stdout has to show the page, not swallow it.
#[test]
fn real_wget_shows_the_page_for_every_spelling_of_stdout() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    let url = format!("{base}/a.txt");
    let url = url.as_str();
    for args in [
        vec![url, "-O", "-"],
        vec![url, "-O-"],
        vec![url, "-qO-"],
        vec![url, "-qO", "-"],
        vec![url, "-q", "-O", "-"],
        vec![url, "--output-document=-"],
        vec![url, "--output-document", "-"],
        vec![url, "--tries=1", "-O", "-"],
    ] {
        let (tmp, out) = real_wget(&args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        assert_eq!(
            result_line(&out, &base),
            "SERVER/a.txt ok | 1 lines\nhello\n",
            "{args:?}"
        );
        assert!(!in_dir(&tmp, "a.txt").exists(), "{args:?}");
        assert!(!in_dir(&tmp, "-").exists(), "{args:?}");
    }
    // With `-o`, the log goes to its file and the page still comes back.
    let (tmp, out) = real_wget(&[url, "-o", "w.log", "-O", "-"]);
    assert_eq!(
        result_line(&out, &base),
        "SERVER/a.txt ok | 1 lines\nhello\n"
    );
    assert!(in_dir(&tmp, "w.log").exists());
}

/// The hint has to work: following it gets the lines that were left out.
#[test]
fn real_wget_long_page_hint_gets_the_rest() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    let url = format!("{base}/long");
    let (tmp, out) = real_wget(&[&url, "-O", "-"]);
    let text = stdout(&out);
    assert!(text.contains("30 lines"), "{text}");
    assert!(text.contains("line10\n"), "{text}");
    assert!(!text.contains("line11\n"), "{text}");
    assert!(text.contains("+20 more lines"), "{text}");

    let dir = tmp.path().canonicalize().unwrap();
    let all = real_bdo(&dir, &["proxy", "wget", "-q", "-O", "-", &url]);
    assert!(stdout(&all).contains("line30\n"), "{all:?}");
}

#[test]
fn real_wget_several_urls_are_all_reported() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    let one = format!("{base}/one.txt");
    let two = format!("{base}/two.txt");
    for extra in [None, Some("-q")] {
        let mut args = vec![one.as_str(), two.as_str()];
        args.extend(extra);
        let (tmp, out) = real_wget(&args);
        assert!(out.status.success(), "{out:?}");
        let text = stdout(&out);
        assert!(text.starts_with("2 URLs ok | 2 files:"), "{text}");
        assert!(text.contains("one.txt 6B"), "{text}");
        assert!(text.contains("two.txt 6B"), "{text}");
        assert!(in_dir(&tmp, "one.txt").exists());
        assert!(in_dir(&tmp, "two.txt").exists());
    }
}

/// Pages reach stdout in the order they were asked for.
#[test]
fn real_wget_several_urls_to_stdout_keep_their_order() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    let (_tmp, out) = real_wget(&[&format!("{base}/long"), &format!("{base}/a.txt"), "-O", "-"]);
    let text = stdout(&out);
    assert!(text.contains("2 URLs ok | 31 lines"), "{text}");
    assert!(text.contains("first 10 lines:\nline1\n"), "{text}");
}

#[test]
fn real_wget_recursive_counts_every_file() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    let (tmp, out) = real_wget(&[&format!("{base}/site"), "-r", "-l", "1", "-nd"]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    assert!(text.contains("4 files:"), "{text}");
    assert!(text.contains("a.html"), "{text}");
    assert!(text.contains("b.html"), "{text}");
    assert!(in_dir(&tmp, "b.html").exists());
}

/// The URL shown first is not the one that failed, and what did arrive is listed.
#[test]
fn real_wget_partial_failure_reports_both_outcomes() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    let good = format!("{base}/good.txt");
    let gone = format!("{base}/gone");
    for args in [
        [good.as_str(), gone.as_str()],
        [gone.as_str(), good.as_str()],
    ] {
        let (tmp, out) = real_wget(&args);
        assert_eq!(out.status.code(), Some(8), "{out:?}");
        assert_eq!(
            result_line(&out, &base),
            "2 URLs FAILED: 404 Not Found | saved: good.txt | 6B\n",
            "{args:?}"
        );
        assert!(in_dir(&tmp, "good.txt").exists());
    }
}

#[test]
fn real_wget_reports_each_failure_of_several() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    let (_tmp, out) = real_wget(&[&format!("{base}/gone"), &format!("{base}/broken")]);
    assert_eq!(out.status.code(), Some(8), "{out:?}");
    assert_eq!(
        result_line(&out, &base),
        "2 URLs FAILED: 404 Not Found, 500 Internal Server Error\n"
    );
}

#[test]
fn real_wget_server_errors_are_reported_in_both_modes() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    for (path, code, status) in [
        ("gone", 8, "404 Not Found"),
        ("forbidden", 8, "403 Forbidden"),
        ("login", 6, "401 Unauthorized"),
        ("broken", 8, "500 Internal Server Error"),
        ("busy", 8, "503 Service Unavailable"),
        // The first answer is a redirect; the last one is the failure.
        ("to-gone", 8, "404 Not Found"),
    ] {
        let url = format!("{base}/{path}");
        for args in [vec![url.as_str()], vec![url.as_str(), "-O", "-"]] {
            let (_tmp, out) = real_wget(&args);
            assert_eq!(out.status.code(), Some(code), "{args:?}: {out:?}");
            assert_eq!(
                result_line(&out, &base),
                format!("SERVER/{path} FAILED: {status}\n"),
                "{args:?}"
            );
        }
    }
}

#[test]
fn real_wget_connection_failures_are_named() {
    if !wget_installed() {
        return;
    }
    // A port nothing listens on: bound, then released.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let refused = format!("http://127.0.0.1:{port}/x");

    let base = serve();
    let slow = format!("{base}/slow");
    let tls = base.replace("http://", "https://") + "/x";
    for (args, reason) in [
        (vec![refused.as_str(), "--tries=1"], "Connection refused"),
        (
            vec![refused.as_str(), "--tries=1", "-O", "-"],
            "Connection refused",
        ),
        (
            vec!["http://no-such-host.invalid/x", "--tries=1"],
            "DNS lookup failed",
        ),
        (
            vec![slow.as_str(), "--tries=1", "--timeout=1"],
            "Connection timed out",
        ),
        (vec![tls.as_str(), "--tries=1"], "SSL/TLS error"),
    ] {
        let (_tmp, out) = real_wget(&args);
        assert_eq!(out.status.code(), Some(4), "{args:?}: {out:?}");
        assert!(
            stdout(&out).contains(&format!("FAILED: {reason}\n")),
            "{args:?}: {out:?}"
        );
    }
}

/// `SSL` in a URL is not a TLS failure, nor is the word in a mistyped option.
#[test]
fn real_wget_words_in_the_url_or_command_are_not_a_diagnosis() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    let (_tmp, out) = real_wget(&[&format!("{base}/SSL-loop"), "--max-redirect=1"]);
    assert_eq!(out.status.code(), Some(8), "{out:?}");
    assert_eq!(
        result_line(&out, &base),
        "SERVER/SSL-loop FAILED: 1 redirections exceeded.\n"
    );

    let (_tmp, out) = real_wget(&[&format!("{base}/a"), "--no-such-certificate"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let text = stdout(&out);
    assert!(text.contains("unrecognized option"), "{text}");
    assert!(text.contains("--no-such-certificate"), "{text}");
}

#[test]
fn real_wget_failures_with_nothing_on_stderr_still_say_something() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    let url = format!("{base}/gone");
    for args in [
        vec![url.as_str(), "-q"],
        vec![url.as_str(), "-o", "w.log"],
        vec![url.as_str(), "-qO-"],
    ] {
        let (_tmp, out) = real_wget(&args);
        assert_eq!(out.status.code(), Some(8), "{args:?}: {out:?}");
        assert_eq!(
            result_line(&out, &base),
            "SERVER/gone FAILED: the server returned an error (wget exit 8)\n",
            "{args:?}"
        );
    }
}

#[test]
fn real_wget_problems_writing_the_file_are_reported() {
    if !wget_installed() {
        return;
    }
    let base = serve();
    let (_tmp, out) = real_wget(&[&format!("{base}/a"), "-O", "/no/such/dir/x"]);
    assert_ne!(out.status.code(), Some(0), "{out:?}");
    let text = stdout(&out);
    assert!(text.contains("FAILED"), "{text}");
    assert!(text.contains("/no/such/dir/x"), "{text}");
}
