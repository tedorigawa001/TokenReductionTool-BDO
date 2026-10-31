//! `bdo read` (what `cat` / `head` / `tail` become under the hook) must not
//! load unbounded input into memory: non-regular files and files over 16 MiB
//! are streamed unfiltered, byte for byte, like the native tools.

use std::fs;
use std::process::{Command, Output};

fn bdo(dir: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bdo"))
        .current_dir(dir)
        .args(args)
        .env("BDO_DB_PATH", dir.join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1")
        .env("BDO_QUIET", "1")
        .env_remove("BDO_RAW")
        .output()
        .expect("run bdo")
}

#[test]
fn a_file_over_the_limit_is_streamed_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    // 17 MiB of numbered lines, plus a non-UTF-8 byte the filter path would
    // reject: streaming copies bytes, it does not decode them.
    let mut body = Vec::new();
    let mut i = 0u64;
    while body.len() < 17 * 1024 * 1024 {
        body.extend_from_slice(format!("line {i} {}\n", "x".repeat(60)).as_bytes());
        i += 1;
    }
    body.extend_from_slice(b"tail \xff end\n");
    fs::write(dir.path().join("big.log"), &body).unwrap();

    let all = bdo(dir.path(), &["read", "big.log"]);
    assert!(
        all.status.success(),
        "{:?}",
        String::from_utf8_lossy(&all.stderr)
    );
    assert!(
        all.stdout == body,
        "full read differs ({} bytes)",
        all.stdout.len()
    );

    let head = bdo(dir.path(), &["read", "big.log", "-m", "2"]);
    let want: Vec<u8> = body
        .split_inclusive(|b| *b == b'\n')
        .take(2)
        .flatten()
        .copied()
        .collect();
    assert_eq!(head.stdout, want);

    let tail = bdo(dir.path(), &["read", "big.log", "--tail-lines", "1"]);
    assert_eq!(tail.stdout, b"tail \xff end\n");
}

#[cfg(unix)]
#[test]
fn a_fifo_is_streamed_before_the_writer_finishes() {
    use std::io::{Read, Write};
    use std::process::Stdio;
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("pipe");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());

    let mut child = Command::new(env!("CARGO_BIN_EXE_bdo"))
        .current_dir(dir.path())
        .args(["read", "pipe"])
        .env("BDO_DB_PATH", dir.path().join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1")
        .env("BDO_QUIET", "1")
        .env_remove("BDO_RAW")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut writer = fs::OpenOptions::new().write(true).open(&fifo).unwrap();
    writer.write_all(b"ready\n").unwrap();
    writer.flush().unwrap();

    // The first line must arrive while the writer is still open: loading the
    // whole input first (read_to_string) would wait for EOF forever.
    let mut stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut first = [0u8; 6];
        let r = stdout.read_exact(&mut first).map(|()| first);
        tx.send(r).unwrap();
        let mut rest = Vec::new();
        stdout.read_to_end(&mut rest).unwrap();
        rest
    });
    let early = rx.recv_timeout(Duration::from_secs(10));
    writer.write_all(b"done\n").unwrap();
    drop(writer);
    let status = child.wait().unwrap();
    let rest = reader.join().unwrap();
    assert_eq!(&early.expect("no output before EOF").unwrap(), b"ready\n");
    assert_eq!(rest, b"done\n");
    assert!(status.success());
}
