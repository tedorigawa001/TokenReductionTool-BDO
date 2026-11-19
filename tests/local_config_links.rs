//! Project-scoped setup/removal must not follow repository-controlled links.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _temp: tempfile::TempDir,
    project: PathBuf,
    outside: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let outside = temp.path().join("outside");
        let home = temp.path().join("home");
        for path in [&project, &outside, &home] {
            fs::create_dir(path).unwrap();
        }
        Self {
            _temp: temp,
            project,
            outside,
            home,
        }
    }

    fn init(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bdo"))
            .arg("init")
            .args(args)
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("CLAUDE_CONFIG_DIR", self.home.join("claude"))
            .env("PI_CODING_AGENT_DIR", self.home.join("pi"))
            .env("COPILOT_HOME", self.home.join("copilot"))
            .env("BDO_DB_PATH", self.home.join("history.db"))
            .env("BDO_TELEMETRY_DISABLED", "1")
            .env("BDO_TEE", "0")
            .env_remove("BDO_RAW")
            .output()
            .unwrap()
    }

    fn install_project_rules(&self, agent: &str) -> Output {
        // The CLI requires -g for Windsurf setup even though its rules file
        // lives in the current project (removal has the same local scope).
        if agent == "windsurf" {
            self.init(&["--agent", agent, "--global"])
        } else {
            self.init(&["--agent", agent])
        }
    }
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .map(|entry| {
            let entry = entry.unwrap();
            let path = entry.path();
            let content = if entry.file_type().is_dir() {
                None
            } else {
                Some(fs::read(path).unwrap())
            };
            (path.strip_prefix(root).unwrap().to_path_buf(), content)
        })
        .collect()
}

#[cfg(any(unix, windows))]
fn directory_link(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        // Rebuild with `\` separators: `mklink` reads a `/` inside its link
        // argument (as in `.pi/extensions`) as the start of a switch.
        let link: PathBuf = link.components().collect();
        let output = Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(target)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
}

fn assert_refused(output: &Output) {
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Refusing project write"),
        "{output:?}"
    );
}

#[cfg(unix)]
#[test]
fn local_pi_install_rejects_external_plugin_link() {
    let fixture = Fixture::new();
    let victim = fixture.outside.join("victim.ts");
    fs::write(&victim, "outside sentinel").unwrap();
    let plugin = fixture.project.join(".pi/extensions/bdo.ts");
    fs::create_dir_all(plugin.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&victim, &plugin).unwrap();

    let output = fixture.init(&["--agent", "pi"]);
    assert_eq!(fs::read_to_string(&victim).unwrap(), "outside sentinel");
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Refusing project write"));
    assert!(fs::symlink_metadata(plugin)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(unix)]
#[test]
fn local_pi_uninstall_rejects_external_parent_link() {
    let fixture = Fixture::new();
    let victim = fixture.outside.join("extensions/bdo.ts");
    fs::create_dir_all(victim.parent().unwrap()).unwrap();
    fs::write(&victim, "outside sentinel").unwrap();
    std::os::unix::fs::symlink(&fixture.outside, fixture.project.join(".pi")).unwrap();

    let output = fixture.init(&["--agent", "pi", "--uninstall"]);
    assert!(victim.exists(), "local uninstall removed an external file");
    assert_eq!(fs::read_to_string(&victim).unwrap(), "outside sentinel");
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Refusing project write"));
}

#[cfg(unix)]
#[test]
fn local_copilot_uninstall_preflights_instructions_before_removing_hook() {
    let fixture = Fixture::new();
    let output = fixture.init(&["--copilot"]);
    assert!(output.status.success(), "{output:?}");
    let hook = fixture.project.join(".github/hooks/bdo-rewrite.json");
    let original_hook = fs::read(&hook).unwrap();
    let instructions = fixture.project.join(".github/copilot-instructions.md");
    let victim = fixture.outside.join("instructions.md");
    fs::rename(&instructions, &victim).unwrap();
    let original_instructions = fs::read(&victim).unwrap();
    std::os::unix::fs::symlink(&victim, &instructions).unwrap();

    let output = fixture.init(&["--copilot", "--uninstall"]);
    assert_eq!(fs::read(&victim).unwrap(), original_instructions);
    assert_eq!(fs::read(&hook).unwrap(), original_hook);
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Refusing project write"));
}

const PROJECT_RULES: &[(&str, &str)] = &[
    ("cline", ".clinerules"),
    ("windsurf", ".windsurfrules"),
    ("kilocode", ".kilocode/rules/bdo-rules.md"),
    ("antigravity", ".agents/rules/antigravity-bdo-rules.md"),
];

#[test]
fn normal_project_rule_install_and_uninstall_preserve_user_content() {
    for &(agent, path) in PROJECT_RULES {
        let fixture = Fixture::new();
        let target = fixture.project.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let user = "# My rules\nKeep this.\n";
        fs::write(&target, user).unwrap();
        let output = fixture.install_project_rules(agent);
        assert!(output.status.success(), "{agent}: {output:?}");
        let installed = fs::read(&target).unwrap();
        let output = fixture.init(&["--agent", agent, "--uninstall", "--dry-run"]);
        assert!(output.status.success(), "{agent}: {output:?}");
        assert_eq!(fs::read(&target).unwrap(), installed);
        // These agents stay project-scoped even with --global.
        let output = fixture.init(&["--agent", agent, "--uninstall", "--global"]);
        assert!(output.status.success(), "{agent}: {output:?}");
        assert_eq!(fs::read_to_string(&target).unwrap(), user);
        let output = fixture.init(&["--agent", agent, "--uninstall"]);
        assert!(output.status.success(), "{agent}: {output:?}");
    }
}

#[test]
fn normal_pi_and_copilot_setup_and_removal_preserve_scope_and_user_content() {
    for (args, path) in [
        (vec!["--agent", "pi"], ".pi/extensions/bdo.ts"),
        (vec!["--copilot"], ".github/hooks/bdo-rewrite.json"),
    ] {
        let fixture = Fixture::new();
        let instructions = fixture.project.join(".github/copilot-instructions.md");
        if args == ["--copilot"] {
            fs::create_dir(instructions.parent().unwrap()).unwrap();
            fs::write(&instructions, "# My instructions\n").unwrap();
        }
        let before = snapshot(&fixture.project);
        let mut dry_run = args.clone();
        dry_run.push("--dry-run");
        assert!(fixture.init(&dry_run).status.success());
        assert_eq!(snapshot(&fixture.project), before);
        let output = fixture.init(&args);
        assert!(output.status.success(), "{output:?}");
        assert!(fixture.project.join(path).is_file());
        let installed = snapshot(&fixture.project);
        assert!(fixture.init(&args).status.success());
        assert_eq!(snapshot(&fixture.project), installed);
        dry_run.push("--uninstall");
        assert!(fixture.init(&dry_run).status.success());
        assert_eq!(snapshot(&fixture.project), installed);
        let mut uninstall = args.clone();
        uninstall.push("--uninstall");
        assert!(fixture.init(&uninstall).status.success());
        assert!(!fixture.project.join(path).exists());
        assert!(fixture.init(&uninstall).status.success());
        if args == ["--copilot"] {
            assert_eq!(
                fs::read_to_string(instructions).unwrap(),
                "# My instructions\n"
            );
        }
        assert_eq!(fs::read_dir(&fixture.outside).unwrap().count(), 0);
    }
}

#[cfg(any(unix, windows))]
#[test]
fn local_operations_reject_parent_links_and_windows_junctions() {
    for (args, component) in [
        (vec!["--agent", "pi"], ".pi"),
        (vec!["--agent", "pi"], ".pi/extensions"),
        (vec!["--copilot"], ".github"),
        (vec!["--copilot"], ".github/hooks"),
        (vec!["--agent", "kilocode"], ".kilocode"),
        (vec!["--agent", "kilocode"], ".kilocode/rules"),
        (vec!["--agent", "antigravity"], ".agents"),
        (vec!["--agent", "antigravity"], ".agents/rules"),
    ] {
        let fixture = Fixture::new();
        let output = fixture.init(&args);
        assert!(output.status.success(), "{component}: {output:?}");
        let linked = fixture.project.join(component);
        let destination = fixture.outside.join("linked");
        fs::rename(&linked, &destination).unwrap();
        directory_link(&destination, &linked);
        let original = snapshot(&fixture.outside);
        for uninstall in [false, true] {
            for dry_run in [false, true] {
                let mut operation = args.clone();
                if uninstall {
                    operation.push("--uninstall");
                }
                if dry_run {
                    operation.push("--dry-run");
                }
                assert_refused(&fixture.init(&operation));
                assert_eq!(snapshot(&fixture.outside), original, "{component}");
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn project_rule_file_links_are_rejected_even_with_global_or_dry_run() {
    for &(agent, path) in PROJECT_RULES {
        for dangling in [false, true] {
            let fixture = Fixture::new();
            let target = fixture.project.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            let victim = fixture.outside.join("victim.md");
            if !dangling {
                fs::write(&victim, "# My private rules\n").unwrap();
            }
            std::os::unix::fs::symlink(&victim, &target).unwrap();
            let before = snapshot(&fixture.outside);
            for global in [false, true] {
                for dry_run in [false, true] {
                    let mut args = vec!["--agent", agent, "--uninstall"];
                    if global {
                        args.push("--global");
                    }
                    if dry_run {
                        args.push("--dry-run");
                    }
                    assert_refused(&fixture.init(&args));
                    assert_eq!(snapshot(&fixture.outside), before);
                }
            }
        }
    }
}

#[cfg(any(unix, windows))]
#[test]
fn local_pi_install_checks_links_before_creating_missing_extensions_directory() {
    let fixture = Fixture::new();
    directory_link(&fixture.outside, &fixture.project.join(".pi"));
    for dry_run in [false, true] {
        let mut args = vec!["--agent", "pi"];
        if dry_run {
            args.push("--dry-run");
        }
        assert_refused(&fixture.init(&args));
        assert_eq!(fs::read_dir(&fixture.outside).unwrap().count(), 0);
    }
}

#[cfg(unix)]
#[test]
fn local_pi_and_copilot_reject_final_links_including_dangling_links() {
    for (args, path) in [
        (vec!["--agent", "pi"], ".pi/extensions/bdo.ts"),
        (vec!["--copilot"], ".github/hooks/bdo-rewrite.json"),
        (vec!["--copilot"], ".github/copilot-instructions.md"),
    ] {
        for dangling in [false, true] {
            let fixture = Fixture::new();
            let target = fixture.project.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            let victim = fixture.outside.join("victim");
            if !dangling {
                fs::write(&victim, "outside sentinel").unwrap();
            }
            std::os::unix::fs::symlink(&victim, &target).unwrap();
            let original = snapshot(&fixture.outside);
            for uninstall in [false, true] {
                for dry_run in [false, true] {
                    let mut args = args.clone();
                    if uninstall {
                        args.push("--uninstall");
                    }
                    if dry_run {
                        args.push("--dry-run");
                    }
                    assert_refused(&fixture.init(&args));
                    assert_eq!(snapshot(&fixture.outside), original);
                    assert!(fs::symlink_metadata(&target)
                        .unwrap()
                        .file_type()
                        .is_symlink());
                }
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn global_pi_keeps_intentional_file_symlink_support() {
    let fixture = Fixture::new();
    let victim = fixture.outside.join("plugin.ts");
    fs::write(&victim, "old plugin").unwrap();
    let plugin = fixture.home.join("pi/extensions/bdo.ts");
    fs::create_dir_all(plugin.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&victim, &plugin).unwrap();
    let output = fixture.init(&["--agent", "pi", "--global"]);
    assert!(output.status.success(), "{output:?}");
    assert!(fs::read_to_string(&victim).unwrap().contains("bdo rewrite"));
    assert!(fs::symlink_metadata(&plugin)
        .unwrap()
        .file_type()
        .is_symlink());
    let content = fs::read(&victim).unwrap();
    let output = fixture.init(&["--agent", "pi", "--global", "--uninstall"]);
    assert!(output.status.success(), "{output:?}");
    assert!(!plugin.exists());
    assert_eq!(fs::read(&victim).unwrap(), content);
}

#[cfg(unix)]
#[test]
fn global_copilot_keeps_intentional_instruction_symlink_support() {
    let fixture = Fixture::new();
    let victim = fixture.outside.join("instructions.md");
    fs::write(&victim, "# My global instructions\n").unwrap();
    let instructions = fixture.home.join("copilot/copilot-instructions.md");
    fs::create_dir_all(instructions.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&victim, &instructions).unwrap();
    let output = fixture.init(&["--copilot", "--global"]);
    assert!(output.status.success(), "{output:?}");
    assert!(fs::read_to_string(&victim)
        .unwrap()
        .contains("bdo-instructions"));
    let output = fixture.init(&["--copilot", "--global", "--uninstall"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(&victim).unwrap(),
        "# My global instructions\n"
    );
    assert!(fs::symlink_metadata(instructions)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(unix)]
#[test]
fn project_rule_uninstall_rejects_external_file_links() {
    for &(agent, path) in PROJECT_RULES {
        let fixture = Fixture::new();
        let target = fixture.project.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "# My rules\nKeep this.\n").unwrap();
        let output = fixture.install_project_rules(agent);
        assert!(output.status.success(), "{agent}: {output:?}");
        let original = fs::read(&target).unwrap();
        let victim = fixture.outside.join("victim.md");
        fs::rename(&target, &victim).unwrap();
        std::os::unix::fs::symlink(&victim, &target).unwrap();

        let output = fixture.init(&["--agent", agent, "--uninstall"]);
        assert_eq!(fs::read(&victim).unwrap(), original, "{agent}");
        assert!(!output.status.success(), "{agent}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("Refusing project write"));
    }
}
