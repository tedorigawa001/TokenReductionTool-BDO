//! Summarizes project dependencies from lock files and manifests.

use crate::core::redact::redact_secrets;
use crate::core::tracking;
use crate::core::truncate::{reduced, CAP_WARNINGS};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

const MAX_DEPS: usize = CAP_WARNINGS;
// dev deps are secondary to prod — show fewer.
const MAX_DEV_DEPS: usize = reduced(CAP_WARNINGS, 5);

/// Each manifest this understands: its file name, its label, and its summarizer.
type Summarizer = fn(&str) -> Result<String>;
const MANIFESTS: &[(&str, &str, Summarizer)] = &[
    ("Cargo.toml", "Rust", summarize_cargo),
    ("package.json", "Node.js", summarize_package_json),
    ("requirements.txt", "Python", summarize_requirements),
    ("pyproject.toml", "Python", summarize_pyproject),
    ("go.mod", "Go", summarize_gomod),
];

/// Summarize project dependencies
pub fn run(path: &Path, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if !path.exists() {
        bail!("{} does not exist", path.display());
    }
    let dir = if path.is_file() {
        match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        }
    } else {
        path
    };

    if verbose > 0 {
        eprintln!("Scanning dependencies in: {}", dir.display());
    }

    let mut found = false;
    let mut out = String::new();
    let mut raw = String::new();

    for (file, label, summarize) in MANIFESTS {
        let manifest = dir.join(file);
        if !manifest.exists() {
            continue;
        }
        found = true;
        out.push_str(&format!("{} ({}):\n", label, file));
        // One broken manifest must not hide what the others say.
        let summary = fs::read_to_string(&manifest)
            .context("could not read it")
            .and_then(|content| {
                raw.push_str(&content);
                summarize(&content)
            });
        match summary {
            Ok(summary) => out.push_str(&summary),
            Err(e) => out.push_str(&format!("  could not parse: {:#}\n", e)),
        }
    }

    if !found {
        out.push_str(&format!(
            "No dependency files found in {}\n",
            dir.display()
        ));
    }

    print!("{}", out);
    timer.track("cat */deps", "bdo deps", &raw, &out);
    Ok(())
}

fn push_list(out: &mut String, title: &str, items: &[String], max: usize) {
    if items.is_empty() {
        return;
    }
    out.push_str(&format!("  {} ({}):\n", title, items.len()));
    for item in items.iter().take(max) {
        out.push_str(&format!("    {}\n", item));
    }
    if items.len() > max {
        out.push_str(&format!("    ... +{} more\n", items.len() - max));
    }
}

/// How a Cargo or Poetry dependency is specified: a version, or where it comes from.
fn dependency_source(spec: &toml::Value) -> &str {
    if let Some(version) = spec.as_str() {
        return version;
    }
    if let Some(version) = spec.get("version").and_then(|v| v.as_str()) {
        return version;
    }
    ["workspace", "path", "git"]
        .into_iter()
        .find(|key| spec.get(key).is_some())
        .unwrap_or("*")
}

fn dependency_table(table: Option<&toml::Value>) -> Vec<String> {
    table
        .and_then(|t| t.as_table())
        .into_iter()
        .flatten()
        .map(|(name, spec)| format!("{} ({})", name, dependency_source(spec)))
        .collect()
}

fn summarize_cargo(content: &str) -> Result<String> {
    let manifest: toml::Value = toml::from_str(content)?;
    let mut out = String::new();

    for (key, title, max) in [
        ("dependencies", "Dependencies", MAX_DEPS),
        ("build-dependencies", "Build", MAX_DEV_DEPS),
        ("dev-dependencies", "Dev", MAX_DEV_DEPS),
    ] {
        let mut deps = dependency_table(manifest.get(key));
        // `[target.'cfg(unix)'.dependencies]` are dependencies like any other.
        if let Some(targets) = manifest.get("target").and_then(|t| t.as_table()) {
            for target in targets.values() {
                deps.extend(dependency_table(target.get(key)));
            }
        }
        deps.sort();
        deps.dedup();
        push_list(&mut out, title, &deps, max);
    }
    let workspace = manifest.get("workspace").and_then(|w| w.get("dependencies"));
    push_list(
        &mut out,
        "Workspace",
        &dependency_table(workspace),
        MAX_DEPS,
    );
    Ok(out)
}

fn json_section(json: &serde_json::Value, key: &str) -> Vec<String> {
    json.get(key)
        .and_then(|v| v.as_object())
        .into_iter()
        .flatten()
        .map(|(name, version)| format!("{} ({})", name, version.as_str().unwrap_or("*")))
        .collect()
}

fn summarize_package_json(content: &str) -> Result<String> {
    let json: serde_json::Value = serde_json::from_str(content)?;
    let mut out = String::new();

    if let Some(name) = json.get("name").and_then(|v| v.as_str()) {
        let version = json.get("version").and_then(|v| v.as_str()).unwrap_or("?");
        out.push_str(&format!("  {} @ {}\n", name, version));
    }
    for (key, title, max) in [
        ("dependencies", "Dependencies", MAX_DEPS),
        ("peerDependencies", "Peer", MAX_DEV_DEPS),
        ("optionalDependencies", "Optional", MAX_DEV_DEPS),
        ("devDependencies", "Dev Dependencies", MAX_DEV_DEPS),
    ] {
        push_list(&mut out, title, &json_section(&json, key), max);
    }
    Ok(out)
}

/// A requirements line without its comment, or `None` if it names no dependency.
/// Pip options are skipped: `--index-url` is not a dependency and can hold a password.
fn requirement(line: &str) -> Option<String> {
    let line = line.trim();
    let line = line.split(" #").next().unwrap_or(line);
    let line = line.trim_end_matches('\\').trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with("--") {
        return None;
    }
    Some(redact_secrets(line).into_owned())
}

fn summarize_requirements(content: &str) -> Result<String> {
    let deps: Vec<String> = content.lines().filter_map(requirement).collect();
    let mut out = String::new();
    if deps.is_empty() {
        out.push_str("  Packages (0)\n");
    }
    push_list(&mut out, "Packages", &deps, MAX_DEPS);
    Ok(out)
}

fn string_array(value: Option<&toml::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

fn summarize_pyproject(content: &str) -> Result<String> {
    let manifest: toml::Value = toml::from_str(content)?;
    let project = manifest.get("project");
    let poetry = manifest.get("tool").and_then(|t| t.get("poetry"));

    let mut deps = string_array(project.and_then(|p| p.get("dependencies")));
    deps.extend(dependency_table(poetry.and_then(|p| p.get("dependencies"))));

    // Everything optional or dev-only, however this project's tool spells it.
    let mut optional = dependency_table(poetry.and_then(|p| p.get("dev-dependencies")));
    let groups = [
        project.and_then(|p| p.get("optional-dependencies")),
        manifest.get("dependency-groups"),
    ];
    for group in groups.into_iter().flatten() {
        for members in group.as_table().into_iter().flat_map(|t| t.values()) {
            optional.extend(string_array(Some(members)));
        }
    }
    let poetry_groups = poetry
        .and_then(|p| p.get("group"))
        .and_then(|g| g.as_table());
    for group in poetry_groups.into_iter().flat_map(|t| t.values()) {
        optional.extend(dependency_table(group.get("dependencies")));
    }

    let mut out = String::new();
    push_list(&mut out, "Dependencies", &deps, MAX_DEPS);
    push_list(&mut out, "Optional/dev", &optional, MAX_DEV_DEPS);
    Ok(out)
}

/// `path version`, marked if Go only needs it for another dependency.
fn go_requirement(line: &str) -> Option<String> {
    let mut parts = line.split_whitespace();
    let (path, version) = (parts.next()?, parts.next()?);
    let indirect = line.contains("// indirect");
    Some(format!(
        "{} {}{}",
        path,
        version,
        if indirect { " (indirect)" } else { "" }
    ))
}

fn summarize_gomod(content: &str) -> Result<String> {
    let mut module_name = String::new();
    let mut go_version = String::new();
    let mut deps = Vec::new();
    let mut in_require = false;
    let mut out = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("module ") {
            module_name = line.trim_start_matches("module ").to_string();
        } else if line.starts_with("go ") {
            go_version = line.trim_start_matches("go ").to_string();
        } else if line == "require (" {
            in_require = true;
        } else if line == ")" {
            in_require = false;
        } else if in_require && !line.starts_with("//") {
            deps.extend(go_requirement(line));
        } else if let Some(requirement) = line.strip_prefix("require ") {
            deps.extend(go_requirement(requirement));
        }
    }

    if !module_name.is_empty() {
        out.push_str(&format!("  {} (go {})\n", module_name, go_version));
    }
    push_list(&mut out, "Dependencies", &deps, MAX_DEPS);
    Ok(out)
}
