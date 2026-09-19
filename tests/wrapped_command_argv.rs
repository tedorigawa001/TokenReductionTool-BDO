//! `bdo err <cmd...>` and `bdo test <cmd...>` must exec the wrapped command via
//! argv, not by re-joining the arguments into a string for `sh -c`.
//!
//! The shell path had two failure modes, both of which changed *which command
//! ran*: an argument containing a space was split into several, and shell
//! metacharacters inside an argument became executable syntax. These tests pin
//! the argv contract with the real binary.

use std::fs;
use std::path::Path;
use std::process::Command;

/// A script that echoes each argv element bracketed on its own line and exits
/// with a distinctive code, so a test can see both what the child received and
/// whether bdo propagated its status. Only the unix tests use it — without
/// this gate it is dead code on Windows, and `-D warnings` fails the build.
#[cfg(unix)]
fn argv_probe(dir: &Path) -> std::path::PathBuf {
    let p = dir.join("probe.sh");
    fs::write(
        &p,
        "#!/bin/sh\ni=0; for a in \"$@\"; do i=$((i+1)); printf '[%d]=<%s>\\n' \"$i\" \"$a\"; done; exit 42\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

fn bdo(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bdo"))
        .current_dir(dir)
        .args(args)
        .env("BDO_DB_PATH", dir.join("track.db")) // keep the real tracking DB clean
        .env("BDO_TELEMETRY_DISABLED", "1")
        .output()
        .expect("spawn bdo")
}

#[cfg(unix)]
#[test]
fn err_preserves_argument_boundaries_and_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let probe = argv_probe(dir.path());
    let out = bdo(dir.path(), &["err", probe.to_str().unwrap(), "a b", "c"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("[1]=<a b>"),
        "'a b' must stay one argument: {text}"
    );
    assert!(text.contains("[2]=<c>"), "{text}");
    assert!(!text.contains("[3]="), "no third argument: {text}");
    assert_eq!(
        out.status.code(),
        Some(42),
        "child exit code must propagate"
    );
}

#[cfg(unix)]
#[test]
fn test_preserves_argument_boundaries_and_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let probe = argv_probe(dir.path());
    let out = bdo(dir.path(), &["test", probe.to_str().unwrap(), "a b", "c"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("[1]=<a b>"),
        "'a b' must stay one argument: {text}"
    );
    assert!(!text.contains("[3]="), "no third argument: {text}");
    assert_eq!(
        out.status.code(),
        Some(42),
        "child exit code must propagate"
    );
}

#[cfg(unix)]
#[test]
fn shell_metacharacters_inside_an_argument_are_inert() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("INJECTED");
    // Passed to grep as a single pattern argument. Under `sh -c` the `;` would
    // end the grep command and run `touch`.
    let pattern = format!("x; touch {}", marker.display());
    for sub in ["err", "test"] {
        let _ = bdo(dir.path(), &[sub, "grep", &pattern, "/dev/null"]);
        assert!(
            !marker.exists(),
            "bdo {sub}: a metacharacter inside one argument must not execute"
        );
    }
}

#[cfg(unix)]
#[test]
fn explicit_sh_dash_c_still_works_and_propagates_status() {
    // Shell semantics remain available by asking for them; the quoted string
    // now reaches sh as one argument instead of being re-split.
    let dir = tempfile::tempdir().unwrap();
    let out = bdo(dir.path(), &["err", "sh", "-c", "exit 42"]);
    assert_eq!(out.status.code(), Some(42));
    let out = bdo(dir.path(), &["test", "sh", "-c", "exit 7"]);
    assert_eq!(out.status.code(), Some(7));
}

#[test]
fn find_missing_start_path_exits_nonzero_like_native_find() {
    // bdo find walks the tree itself; a missing start path used to look like
    // "no matches" and exit 0, so `bdo find … && next` ran `next` anyway.
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("does-not-exist");
    for args in [
        vec!["find", "*.rs", missing.to_str().unwrap()], // pattern, path
        vec!["find", missing.to_str().unwrap()],         // bare path (hook rewrite of `find <dir>`)
    ] {
        let out = bdo(dir.path(), &args);
        assert_ne!(
            out.status.code(),
            Some(0),
            "{args:?}: missing start path must not exit 0"
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("No such file or directory"), "{args:?}: {err}");
    }
}

#[test]
fn find_bare_path_lists_it_instead_of_treating_it_as_a_pattern() {
    // `find <dir>` is the most common native invocation. The hook rewrites it
    // to `bdo find <dir>`, which used to read <dir> as a glob searched under
    // `.` — a different command that usually found nothing.
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("hello.rs"), "").unwrap();
    let out = bdo(dir.path(), &["find", sub.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("hello.rs"),
        "must list the directory's contents: {text}"
    );
}

#[test]
fn empty_wrapped_command_is_an_error_not_a_silent_success() {
    // `sh -c ""` used to exit 0 with no output, so `bdo err` alone looked fine.
    let dir = tempfile::tempdir().unwrap();
    for sub in ["err", "test"] {
        let out = bdo(dir.path(), &[sub]);
        assert_ne!(
            out.status.code(),
            Some(0),
            "bdo {sub} with no command must fail"
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("no command given"), "bdo {sub}: {err}");
    }
}
