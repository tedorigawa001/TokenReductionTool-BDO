use std::fs;
use std::process::Command;

#[test]
fn internal_analysis_commands_reject_invalid_flags_without_fallback() {
    let temp = tempfile::tempdir().expect("create temporary test directory");
    let empty_path = temp.path().join("empty-path");
    fs::create_dir(&empty_path).expect("create empty PATH directory");
    let db_path = temp.path().join("history.db");

    // `log` matters most here: macOS ships an unrelated /usr/bin/log, so a
    // fallback there runs a different program (and exits 0) rather than
    // failing loudly. The empty PATH below keeps the assertion honest on
    // every platform.
    for command in ["map", "deps", "json", "smart", "telemetry", "read", "log"] {
        let output = Command::new(env!("CARGO_BIN_EXE_bdo"))
            .args([command, "--nonexistent-flag-xyz"])
            .env("PATH", &empty_path)
            .env("BDO_DB_PATH", &db_path)
            .output()
            .unwrap_or_else(|error| panic!("run bdo {command}: {error}"));

        assert_eq!(
            output.status.code(),
            Some(2),
            "bdo {command} must return Clap's usage error instead of executing an external fallback; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
