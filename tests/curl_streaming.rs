//! Curl must forward piped bytes before the transfer finishes, without buffering.
#![cfg(unix)]

use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::Duration;

fn fixture(script: &str) -> (tempfile::TempDir, Command) {
    let dir = tempfile::tempdir().unwrap();
    let curl = dir.path().join("curl");
    fs::write(&curl, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o700)).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bdo"));
    cmd.args(["curl", "https://example.invalid/"])
        .env(
            "PATH",
            std::env::join_paths(std::iter::once(dir.path().to_path_buf()).chain(
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            ))
            .unwrap(),
        )
        .env("BDO_DB_PATH", dir.path().join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1")
        .env("BDO_QUIET", "1")
        .env_remove("BDO_RAW");
    (dir, cmd)
}

#[test]
fn curl_piped_text_is_byte_exact() {
    for body in [b"  {\n  \"ok\": true\n}\n\n".as_slice(), b"no newline", b""] {
        let (dir, mut cmd) = fixture("cat \"$BDO_CURL_BODY\"");
        let path = dir.path().join("body");
        fs::write(&path, body).unwrap();
        let out = cmd.env("BDO_CURL_BODY", path).output().unwrap();
        assert!(out.status.success(), "{:?}", out);
        assert_eq!(out.stdout, body);
    }
}

#[test]
fn curl_piped_large_binary_and_stderr_keep_bytes_and_exit_status() {
    let (dir, mut cmd) = fixture("cat \"$BDO_CURL_BODY\"; cat \"$BDO_CURL_BODY\" >&2; exit 23");
    let mut body = vec![b'x'; 17 * 1024 * 1024];
    body.extend_from_slice(&[0, 0xff, 0xfe]);
    let path = dir.path().join("body");
    fs::write(&path, &body).unwrap();
    let out = cmd.env("BDO_CURL_BODY", path).output().unwrap();
    assert_eq!(out.status.code(), Some(23));
    assert!(
        out.stdout == body,
        "stdout differs ({} bytes)",
        out.stdout.len()
    );
    assert!(
        out.stderr.starts_with(&body),
        "native stderr differs ({} bytes)",
        out.stderr.len()
    );
}

#[test]
fn curl_piped_failure_keeps_wrapper_diagnostic() {
    let (_dir, mut cmd) = fixture("exit 7");
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(7));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("FAILED: curl"));
}

#[test]
fn curl_pipe_receives_prefix_before_child_exit() {
    let (dir, mut cmd) = fixture(
        "printf ready; while [ ! -e \"$BDO_CURL_RELEASE\" ]; do sleep 0.01; done; printf done",
    );
    let release = dir.path().join("release");
    cmd.env("BDO_CURL_RELEASE", &release);
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let (send, recv) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut prefix = [0; 5];
        let result = stdout.read_exact(&mut prefix).map(|()| prefix);
        send.send(result).unwrap();
        let mut rest = Vec::new();
        stdout.read_to_end(&mut rest).unwrap();
        rest
    });
    let early = recv.recv_timeout(Duration::from_secs(10));
    // Release even on failure, so the test cannot leave its child blocked.
    fs::write(release, b"").unwrap();
    let status = child.wait().unwrap();
    let rest = reader.join().unwrap();
    assert_eq!(early.unwrap().unwrap(), *b"ready");
    assert!(status.success());
    assert_eq!(rest, b"done");
}
