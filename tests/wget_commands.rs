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
        ("https://example.com/f.zip?x=1", "f.zip"),
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
        ["-O", "x.bin", "--tries=1", "https://example.com/a"]
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
        ("ERROR 403: Forbidden.", "403 Forbidden"),
        (
            "wget: server returned error: HTTP/1.1 401 Unauthorized",
            "401 Unauthorized",
        ),
        ("ERROR 500: Internal Server Error.", "500 Server Error"),
        (
            "ERROR 503: Service Unavailable.",
            "ERROR 503: Service Unavailable.",
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

/// A server answering one request per entry, in order, on a thread of its own.
fn serve(responses: Vec<&'static str>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(response.as_bytes());
        }
    });
    address
}

const OK_HELLO: &str = "HTTP/1.0 200 OK\r\nContent-Length: 6\r\n\r\nhello\n";
const NOT_FOUND: &str = "HTTP/1.0 404 Not Found\r\nContent-Length: 0\r\n\r\n";

fn real_wget(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bdo"))
        .arg("wget")
        .args(args)
        .current_dir(cwd)
        .env("BDO_DB_PATH", cwd.join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1")
        .env("no_proxy", "127.0.0.1")
        .output()
        .unwrap()
}

fn real_dir() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().canonicalize().unwrap();
    (tmp, path)
}

#[test]
fn real_wget_download_saves_and_reports_the_file() {
    if !wget_installed() {
        return;
    }
    let (_tmp, dir) = real_dir();
    let base = serve(vec![OK_HELLO]);
    let out = real_wget(&dir, &[&format!("{base}/noextension")]);
    assert!(out.status.success(), "{out:?}");
    assert!(stdout(&out).contains("ok | noextension | 6B"), "{out:?}");
    assert_eq!(
        fs::read_to_string(dir.join("noextension")).unwrap(),
        "hello\n"
    );
}

#[test]
fn real_wget_to_stdout_prints_the_page() {
    if !wget_installed() {
        return;
    }
    let (_tmp, dir) = real_dir();
    let base = serve(vec![OK_HELLO]);
    let out = real_wget(&dir, &[&format!("{base}/x"), "-O", "-"]);
    assert!(out.status.success(), "{out:?}");
    assert!(stdout(&out).contains("1 lines\nhello\n"), "{out:?}");
}

#[test]
fn real_wget_not_found_is_reported_in_both_modes() {
    if !wget_installed() {
        return;
    }
    let (_tmp, dir) = real_dir();
    let base = serve(vec![NOT_FOUND, NOT_FOUND]);
    for args in [
        vec![format!("{base}/gone")],
        vec![format!("{base}/gone"), "-O".into(), "-".into()],
    ] {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = real_wget(&dir, &args);
        assert_eq!(out.status.code(), Some(8), "{out:?}");
        assert!(stdout(&out).contains("FAILED: 404 Not Found"), "{out:?}");
    }
}
