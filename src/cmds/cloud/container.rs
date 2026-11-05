//! Filters Docker and kubectl output into compact summaries.

use crate::core::runner::{self, RunOptions};
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::truncate::{CAP_INVENTORY, CAP_LIST, CAP_WARNINGS};
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use serde_json::Value;
use std::ffi::OsString;
use std::process::Command;

lazy_static! {
    // A build step, plain or on a terminal: `#4 [web 1/3] FROM` or `=> [web 1/3] FROM`.
    // A step's own output comes after a timestamp (`#5 0.172 [INFO] …`), so no match.
    static ref BUILD_STEP_RE: Regex = Regex::new(r"^\s*(?:#\d+|=>)\s+\[([^\]]+)\]").unwrap();
    static ref BUILT_RE: Regex =
        Regex::new(r"^\s*(?:✔\s+)?(?:Service |Image )?(\S+)\s+Built\s*$").unwrap();
}

/// Lines `logs` commands ask for, which bdo says when it may have cut anything.
const LOG_TAIL_LINES: usize = 100;

fn log_tail_note(raw: &str, whole_log: &str) -> String {
    if raw.lines().count() >= LOG_TAIL_LINES {
        format!("\n[only the last {LOG_TAIL_LINES} lines were read — use `{whole_log}` for all]")
    } else {
        String::new()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ContainerCmd {
    DockerPs,
    DockerPsAll,
    DockerImages,
    DockerLogs,
    KubectlPods,
    KubectlServices,
    KubectlLogs,
}

pub fn run(cmd: ContainerCmd, args: &[String], verbose: u8) -> Result<i32> {
    match cmd {
        ContainerCmd::DockerPs => docker_ps(verbose),
        ContainerCmd::DockerPsAll => docker_ps_all(verbose),
        ContainerCmd::DockerImages => docker_images(verbose),
        ContainerCmd::DockerLogs => docker_logs(args, verbose),
        ContainerCmd::KubectlPods => kubectl_pods(args, verbose),
        ContainerCmd::KubectlServices => kubectl_services(args, verbose),
        ContainerCmd::KubectlLogs => kubectl_logs(args, verbose),
    }
}

fn run_kubectl_json<F>(cmd: Command, label: &str, filter_fn: F) -> Result<i32>
where
    F: Fn(&Value) -> String,
{
    runner::run_filtered(
        cmd,
        "kubectl",
        label,
        |stdout| match serde_json::from_str::<Value>(stdout) {
            Ok(json) => filter_fn(&json),
            Err(e) => {
                eprintln!("[bdo] kubectl: JSON parse failed: {}", e);
                stdout.to_string()
            }
        },
        RunOptions::stdout_only()
            .early_exit_on_failure()
            .hide_stdout_when_stderr_explains()
            .no_trailing_newline(),
    )
}

fn docker_ps(_verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let raw = exec_capture(resolved_command("docker").args(["ps"]))
        .map(|r| r.stdout)
        .unwrap_or_default();

    let result = exec_capture(resolved_command("docker").args([
        "ps",
        "--format",
        "{{.ID}}\t{{.Names}}\t{{.Status}}\t{{.Image}}\t{{.Ports}}",
    ]))
    .context("Failed to run docker ps")?;

    if !result.success() {
        eprint!("{}", result.stderr);
        timer.track("docker ps", "bdo docker ps", &raw, &raw);
        return Ok(result.exit_code);
    }

    let stdout = result.stdout;
    let mut rtk = String::new();

    if stdout.trim().is_empty() {
        rtk.push_str("[docker] 0 containers");
        println!("{}", rtk);
        timer.track("docker ps", "bdo docker ps", &raw, &rtk);
        return Ok(0);
    }

    const MAX_CONTAINERS: usize = CAP_LIST;
    let lines: Vec<String> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| format_container_line(line, true))
        .collect();

    rtk.push_str(&format!("[docker] {} containers:\n", lines.len()));
    for entry in lines.iter().take(MAX_CONTAINERS) {
        rtk.push_str(entry);
    }
    if lines.len() > MAX_CONTAINERS {
        rtk.push_str(&format!("  … +{} more\n", lines.len() - MAX_CONTAINERS));
        let full: String = lines.concat();
        if let Some(hint) = crate::core::tee::force_tee_hint(&full, "docker-ps") {
            rtk.push_str(&format!("{}\n", hint));
        }
    }

    print!("{}", rtk);
    timer.track("docker ps", "bdo docker ps", &raw, &rtk);
    Ok(0)
}

fn docker_ps_all(_verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let raw = exec_capture(resolved_command("docker").args(["ps", "-a"]))
        .map(|r| r.stdout)
        .unwrap_or_default();

    let result = exec_capture(resolved_command("docker").args([
        "ps",
        "-a",
        "--format",
        "{{.State}}\t{{.ID}}\t{{.Names}}\t{{.Status}}\t{{.Image}}\t{{.Ports}}",
    ]))
    .context("Failed to run docker ps -a")?;

    if !result.success() {
        eprint!("{}", result.stderr);
        timer.track("docker ps -a", "bdo docker ps -a", &raw, &raw);
        return Ok(result.exit_code);
    }

    let mut running_lines: Vec<String> = Vec::new();
    let mut stopped_lines: Vec<String> = Vec::new();
    for line in result.stdout.lines().filter(|l| !l.trim().is_empty()) {
        let parts: Vec<&str> = line.split('\t').collect();
        let state = parts.first().copied().unwrap_or("");
        let is_running = matches!(state, "running" | "restarting" | "paused");
        if let Some(entry) = format_container_line_from_parts(&parts[1..], is_running) {
            if is_running {
                running_lines.push(entry);
            } else {
                stopped_lines.push(entry);
            }
        }
    }

    const MAX_CONTAINERS: usize = 20;
    let truncated = running_lines.len() > MAX_CONTAINERS || stopped_lines.len() > MAX_CONTAINERS;

    let mut rtk = String::new();
    rtk.push_str(&format!("[docker] {} running:\n", running_lines.len()));
    for l in running_lines.iter().take(MAX_CONTAINERS) {
        rtk.push_str(l);
    }
    if running_lines.len() > MAX_CONTAINERS {
        rtk.push_str(&format!(
            "  … +{} more\n",
            running_lines.len() - MAX_CONTAINERS
        ));
    }
    if !stopped_lines.is_empty() {
        rtk.push_str(&format!(
            "[docker] {} stopped/exited:\n",
            stopped_lines.len()
        ));
        for l in stopped_lines.iter().take(MAX_CONTAINERS) {
            rtk.push_str(l);
        }
        if stopped_lines.len() > MAX_CONTAINERS {
            rtk.push_str(&format!(
                "  … +{} more\n",
                stopped_lines.len() - MAX_CONTAINERS
            ));
        }
    }
    if truncated {
        let full: String = running_lines.iter().chain(stopped_lines.iter()).cloned().collect();
        if let Some(hint) = crate::core::tee::force_tee_hint(&full, "docker-ps-a") {
            rtk.push_str(&format!("{}\n", hint));
        }
    }

    print!("{}", rtk);
    timer.track("docker ps -a", "bdo docker ps -a", &raw, &rtk);
    Ok(0)
}

fn format_container_line(line: &str, with_ports: bool) -> Option<String> {
    let parts: Vec<&str> = line.split('\t').collect();
    format_container_line_from_parts(&parts, with_ports)
}

fn format_container_line_from_parts(parts: &[&str], with_ports: bool) -> Option<String> {
    if parts.len() < 4 {
        return None;
    }
    let id = &parts[0][..12.min(parts[0].len())];
    let name = parts[1];
    let status = parts[2].trim();
    let short_image = parts[3].split('/').next_back().unwrap_or("");
    let port_suffix = if with_ports {
        let ports = compact_ports(parts.get(4).unwrap_or(&""));
        if ports == "-" {
            String::new()
        } else {
            format!(" [{}]", ports)
        }
    } else {
        String::new()
    };
    Some(format!(
        "  {} {} ({}) {}{}\n",
        id, name, short_image, status, port_suffix
    ))
}

fn docker_images(_verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let raw = exec_capture(resolved_command("docker").args(["images"]))
        .map(|r| r.stdout)
        .unwrap_or_default();

    let result = exec_capture(resolved_command("docker").args([
        "images",
        "--format",
        "{{.Repository}}:{{.Tag}}\t{{.Size}}",
    ]))
    .context("Failed to run docker images")?;

    if !result.success() {
        eprint!("{}", result.stderr);
        timer.track("docker images", "bdo docker images", &raw, &raw);
        return Ok(result.exit_code);
    }

    let stdout = result.stdout;
    let lines: Vec<&str> = stdout.lines().collect();
    let mut rtk = String::new();

    if lines.is_empty() {
        rtk.push_str("[docker] 0 images");
        println!("{}", rtk);
        timer.track("docker images", "bdo docker images", &raw, &rtk);
        return Ok(0);
    }

    rtk.push_str(&format!(
        "[docker] {} images ({})\n",
        lines.len(),
        images_total(&lines)
    ));

    // a full image list is an inventory query, like pip list.
    const MAX_IMAGES: usize = CAP_INVENTORY;
    let image_lines: Vec<String> = lines
        .iter()
        .map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            let image = parts.first().copied().unwrap_or("");
            let size = parts.get(1).copied().unwrap_or("");
            format!("  {} [{}]\n", image, size)
        })
        .collect();

    let mut full_rtk = rtk.clone();
    for l in &image_lines {
        full_rtk.push_str(l);
    }

    for l in image_lines.iter().take(MAX_IMAGES) {
        rtk.push_str(l);
    }
    if image_lines.len() > MAX_IMAGES {
        rtk.push_str(&format!("  … +{} more\n", image_lines.len() - MAX_IMAGES));
        if let Some(hint) = crate::core::tee::force_tee_tail_hint(&full_rtk, "docker-images", MAX_IMAGES + 2) {
            rtk.push_str(&format!("{}\n", hint));
        }
    }

    print!("{}", rtk);
    timer.track("docker images", "bdo docker images", &raw, &rtk);
    Ok(0)
}

/// The sum of the sizes in `image<tab>size` lines.
fn images_total(lines: &[&str]) -> String {
    let total_size_mb: f64 = lines
        .iter()
        .filter_map(|line| line.split('\t').nth(1))
        .filter_map(image_size_mb)
        // Not `sum()`: over nothing, that gives -0.0, which prints as `-0kB`.
        .fold(0.0, |total, size| total + size);

    if total_size_mb >= 1000.0 {
        format!("{:.1}GB", total_size_mb / 1000.0)
    } else if total_size_mb >= 1.0 {
        format!("{:.0}MB", total_size_mb)
    } else {
        format!("{:.0}kB", total_size_mb * 1000.0)
    }
}

/// A size as docker prints it, in decimal units: `1.61GB`, `13.3kB`.
fn image_size_mb(size: &str) -> Option<f64> {
    [("TB", 1e6), ("GB", 1e3), ("MB", 1.0), ("kB", 1e-3), ("B", 1e-6)]
        .iter()
        .find_map(|(unit, mb)| {
            let number = size.trim().strip_suffix(unit)?;
            Some(number.trim().parse::<f64>().ok()? * mb)
        })
}

fn docker_logs(args: &[String], _verbose: u8) -> Result<i32> {
    let container = args.first().map(|s| s.as_str()).unwrap_or("");
    if container.is_empty() {
        println!("Usage: bdo docker logs <container>");
        return Ok(0);
    }

    let mut cmd = resolved_command("docker");
    cmd.args(["logs", "--tail", &LOG_TAIL_LINES.to_string(), container]);

    let label = format!("logs {}", container);
    runner::run_filtered(
        cmd,
        "docker",
        &label,
        |raw| {
            format!(
                "[docker] Logs for {}:\n{}{}",
                container,
                crate::log_cmd::run_stdin_str(raw),
                log_tail_note(raw, &format!("bdo proxy docker logs {container}"))
            )
        },
        RunOptions::default().early_exit_on_failure(),
    )
}

fn kubectl_pods(args: &[String], _verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("kubectl");
    cmd.args(["get", "pods", "-o", "json"]);
    for arg in args {
        cmd.arg(arg);
    }
    run_kubectl_json(cmd, "get pods", format_kubectl_pods)
}

/// What came back: `get pods` answers with a list, `get pods web` with the pod.
fn kubectl_items(json: &Value) -> Vec<&Value> {
    match json["items"].as_array() {
        Some(items) => items.iter().collect(),
        None if json["metadata"].is_object() => vec![json],
        None => Vec::new(),
    }
}

fn pod_container_statuses(pod: &Value) -> impl Iterator<Item = &Value> {
    let of = |key: &str| {
        pod["status"][key]
            .as_array()
            .map(|a| a.iter())
            .into_iter()
            .flatten()
    };
    of("initContainerStatuses")
        .chain(of("containerStatuses"))
        .collect::<Vec<_>>()
        .into_iter()
}

/// Why a container cannot run. A pod in `CrashLoopBackOff` keeps the phase
/// `Running`, and one in `ImagePullBackOff` keeps `Pending`, so only the
/// containers tell.
fn pod_fault(pod: &Value) -> Option<String> {
    pod_container_statuses(pod).find_map(|c| {
        if let Some(reason) = c["state"]["waiting"]["reason"].as_str() {
            let starting = matches!(reason, "ContainerCreating" | "PodInitializing");
            return (!starting).then(|| reason.to_string());
        }
        let exit_code = c["state"]["terminated"]["exitCode"].as_i64()?;
        (exit_code != 0).then(|| {
            c["state"]["terminated"]["reason"]
                .as_str()
                .map_or_else(|| format!("exit {exit_code}"), str::to_string)
        })
    })
}

/// Why a pod is still pending, where the status says.
fn pod_pending_reason(pod: &Value) -> Option<String> {
    let waiting = pod_container_statuses(pod)
        .find_map(|c| c["state"]["waiting"]["reason"].as_str().map(str::to_string));
    waiting.or_else(|| {
        pod["status"]["conditions"]
            .as_array()?
            .iter()
            .find(|c| c["status"] == "False" && c["reason"].is_string())?["reason"]
            .as_str()
            .map(str::to_string)
    })
}

fn format_kubectl_pods(json: &Value) -> String {
    let pods = kubectl_items(json);
    if pods.is_empty() {
        return "No pods found\n".to_string();
    }
    let (mut running, mut completed, mut pending, mut not_ready, mut terminating, mut failed) =
        (0, 0, 0, 0, 0, 0);
    let mut restarts_total = 0i64;
    // Faults first: they are what a cut-off list must not lose.
    let mut faults: Vec<String> = Vec::new();
    let mut others: Vec<String> = Vec::new();

    for pod in &pods {
        let ns = pod["metadata"]["namespace"].as_str().unwrap_or("-");
        let name = pod["metadata"]["name"].as_str().unwrap_or("-");
        let phase = pod["status"]["phase"].as_str().unwrap_or("Unknown");
        let fault = pod_fault(pod);

        restarts_total += pod_container_statuses(pod)
            .map(|c| c["restartCount"].as_i64().unwrap_or(0))
            .sum::<i64>();

        if pod["metadata"]["deletionTimestamp"].is_string() {
            terminating += 1;
            others.push(format!("{}/{} Terminating", ns, name));
            continue;
        }
        match (phase, fault) {
            ("Succeeded", _) => completed += 1,
            ("Failed", fault) => {
                failed += 1;
                let reason = pod["status"]["reason"].as_str().map(str::to_string);
                let reason = reason.or(fault).unwrap_or_else(|| phase.to_string());
                faults.push(format!("{}/{} {}", ns, name, reason));
            }
            (_, Some(fault)) => {
                failed += 1;
                faults.push(format!("{}/{} {}", ns, name, fault));
            }
            ("Running", None) => {
                let all_ready = pod_container_statuses(pod)
                    .all(|c| c["ready"].as_bool().unwrap_or(true) || c["state"]["terminated"].is_object());
                if all_ready {
                    running += 1;
                } else {
                    not_ready += 1;
                    others.push(format!("{}/{} not ready", ns, name));
                }
            }
            ("Pending", None) => {
                pending += 1;
                others.push(match pod_pending_reason(pod) {
                    Some(reason) => format!("{}/{} Pending ({})", ns, name, reason),
                    None => format!("{}/{} Pending", ns, name),
                });
            }
            (phase, None) => {
                failed += 1;
                faults.push(format!("{}/{} {}", ns, name, phase));
            }
        }
    }

    let mut parts = Vec::new();
    for (count, label) in [
        (running, "running"),
        (completed, "completed"),
        (pending, "pending"),
        (not_ready, "not ready"),
        (terminating, "terminating"),
        (failed, "[x]"),
    ] {
        if count > 0 {
            parts.push(format!("{} {}", count, label));
        }
    }
    if restarts_total > 0 {
        parts.push(format!("{} restarts", restarts_total));
    }

    let mut out = format!("{} pods: {}\n", pods.len(), parts.join(", "));
    faults.extend(others);
    let issues = faults;
    if !issues.is_empty() {
        const MAX_PODS_ISSUES: usize = CAP_WARNINGS;
        out.push_str("[warn] Issues:\n");
        for issue in issues.iter().take(MAX_PODS_ISSUES) {
            out.push_str(&format!("  {}\n", issue));
        }
        if issues.len() > MAX_PODS_ISSUES {
            out.push_str(&format!("  … +{} more", issues.len() - MAX_PODS_ISSUES));
            let all_issues = issues.join("\n");
            if let Some(hint) =
                crate::core::tee::force_tee_tail_hint(&all_issues, "kubectl-pods", MAX_PODS_ISSUES + 1)
            {
                out.push_str(&format!(" {}", hint));
            }
        }
    }
    out
}

fn kubectl_services(args: &[String], _verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("kubectl");
    cmd.args(["get", "services", "-o", "json"]);
    for arg in args {
        cmd.arg(arg);
    }
    run_kubectl_json(cmd, "get services", format_kubectl_services)
}

fn format_service_port(port: &Value) -> String {
    let number = port["port"].as_i64().unwrap_or(0);
    let exposed = match port["nodePort"].as_i64() {
        Some(node_port) => format!("{}:{}", number, node_port),
        None => number.to_string(),
    };
    // A target is a number or the name of a container's port.
    let target = port["targetPort"]
        .as_i64()
        .map(|n| n.to_string())
        .or_else(|| port["targetPort"].as_str().map(str::to_string));
    match target {
        Some(target) if target != number.to_string() => format!("{}→{}", exposed, target),
        _ => exposed,
    }
}

/// Where a service is reachable from outside the cluster.
fn service_external_addresses(svc: &Value) -> Vec<String> {
    let ingress = svc["status"]["loadBalancer"]["ingress"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| i["ip"].as_str().or_else(|| i["hostname"].as_str()));
    let external_ips = svc["spec"]["externalIPs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    ingress.chain(external_ips).map(str::to_string).collect()
}

fn format_kubectl_services(json: &Value) -> String {
    let services = kubectl_items(json);
    if services.is_empty() {
        return "No services found\n".to_string();
    }
    let mut out = format!("{} services:\n", services.len());

    let all_lines: Vec<String> = services
        .iter()
        .map(|svc| {
            let ns = svc["metadata"]["namespace"].as_str().unwrap_or("-");
            let name = svc["metadata"]["name"].as_str().unwrap_or("-");
            let svc_type = svc["spec"]["type"].as_str().unwrap_or("-");
            let ports: Vec<String> = svc["spec"]["ports"]
                .as_array()
                .map(|arr| arr.iter().map(format_service_port).collect())
                .unwrap_or_default();
            let external = service_external_addresses(svc);
            let external = if external.is_empty() {
                String::new()
            } else {
                format!(" external {}", external.join(","))
            };
            format!("  {}/{} {} [{}]{}", ns, name, svc_type, ports.join(","), external)
        })
        .collect();

    const MAX_KUBECTL_SERVICES: usize = CAP_LIST;
    for line in all_lines.iter().take(MAX_KUBECTL_SERVICES) {
        out.push_str(&format!("{}\n", line));
    }
    if all_lines.len() > MAX_KUBECTL_SERVICES {
        out.push_str(&format!("  … +{} more", all_lines.len() - MAX_KUBECTL_SERVICES));
        let all_text = all_lines.join("\n");
        if let Some(hint) =
            crate::core::tee::force_tee_tail_hint(&all_text, "kubectl-services", MAX_KUBECTL_SERVICES + 1)
        {
            out.push_str(&format!(" {}", hint));
        }
        out.push('\n');
    }
    out
}

fn kubectl_logs(args: &[String], _verbose: u8) -> Result<i32> {
    let pod = args.first().map(|s| s.as_str()).unwrap_or("");
    if pod.is_empty() {
        println!("Usage: bdo kubectl logs <pod>");
        return Ok(0);
    }

    let mut cmd = resolved_command("kubectl");
    cmd.args(["logs", "--tail", &LOG_TAIL_LINES.to_string(), pod]);
    for arg in args.iter().skip(1) {
        cmd.arg(arg);
    }

    let label = format!("logs {}", pod);
    runner::run_filtered(
        cmd,
        "kubectl",
        &label,
        |stdout| {
            format!(
                "Logs for {}:\n{}{}",
                pod,
                crate::log_cmd::run_stdin_str(stdout),
                log_tail_note(stdout, &format!("bdo proxy kubectl logs {pod}"))
            )
        },
        RunOptions::stdout_only().early_exit_on_failure(),
    )
}

/// Format `docker compose ps --format` output into compact form.
/// Expects tab-separated lines: Name\tImage\tStatus\tPorts
/// (no header row — `--format` output is headerless)
pub fn format_compose_ps(raw: &str) -> String {
    const MAX_COMPOSE_SERVICES: usize = CAP_LIST;
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();

    if lines.is_empty() {
        return "[compose] 0 services".to_string();
    }

    let mut result = format!("[compose] {} services:\n", lines.len());

    // Pre-build all formatted lines so the tee file matches what the agent sees.
    let all_formatted: Vec<String> = lines
        .iter()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 4 {
                return None;
            }
            let name = parts[0];
            let image = parts[1];
            let status = parts[2];
            let ports = parts[3];
            let short_image = image.split('/').next_back().unwrap_or(image);
            let port_str = if ports.trim().is_empty() {
                String::new()
            } else {
                let compact = compact_ports(ports.trim());
                if compact == "-" {
                    String::new()
                } else {
                    format!(" [{}]", compact)
                }
            };
            Some(format!("  {} ({}) {}{}", name, short_image, status, port_str))
        })
        .collect();

    for line in all_formatted.iter().take(MAX_COMPOSE_SERVICES) {
        result.push_str(line);
        result.push('\n');
    }
    if all_formatted.len() > MAX_COMPOSE_SERVICES {
        result.push_str(&format!("  … +{} more\n", all_formatted.len() - MAX_COMPOSE_SERVICES));
        let all_text = all_formatted.join("\n");
        if let Some(hint) = crate::core::tee::force_tee_tail_hint(&all_text, "compose-ps", MAX_COMPOSE_SERVICES + 1) {
            result.push_str(&format!("  {}\n", hint));
        }
    }

    result.trim_end().to_string()
}

/// Format `docker compose logs` output into compact form
pub fn format_compose_logs(raw: &str) -> String {
    if raw.trim().is_empty() {
        return "[compose] No logs".to_string();
    }

    // docker compose logs prefixes each line with "service-N  | "
    // Use the existing log deduplication engine
    let analyzed = crate::log_cmd::run_stdin_str(raw);
    format!("[compose] Logs:\n{}", analyzed)
}

/// Format `docker compose build` output into compact summary
pub fn format_compose_build(raw: &str) -> String {
    if raw.trim().is_empty() {
        return "[compose] Build: no output".to_string();
    }

    let mut result = String::new();

    // On a terminal: "[+] Building 12.3s (8/8) FINISHED". Captured output, which
    // is what bdo reads, has no such line.
    let building = || raw.lines().filter(|l| l.trim_start().starts_with("[+] Building"));
    match building().find(|l| l.contains("FINISHED")).or_else(|| building().next()) {
        Some(line) => result.push_str(&format!("[compose] {}\n", line.trim())),
        None => result.push_str("[compose] Build:\n"),
    }

    // The service leads a step: "[web 1/4]", "[web internal]".
    let mut services: Vec<String> = Vec::new();
    let mut steps: Vec<&str> = Vec::new();
    for line in raw.lines() {
        let service = if let Some(found) = BUILD_STEP_RE.captures(line) {
            let bracket = found.get(1).map_or("", |m| m.as_str());
            let mut words = bracket.split_whitespace();
            let first = words.next().unwrap_or("");
            // Numbered steps are the Dockerfile's own, and repeat as they progress.
            if words.next().is_some_and(|w| w.contains('/')) && !steps.contains(&bracket) {
                steps.push(bracket);
            }
            first
        } else if let Some(found) = BUILT_RE.captures(line) {
            found.get(1).map_or("", |m| m.as_str())
        } else {
            continue;
        };
        let names_a_service = !matches!(service, "" | "internal" | "auth");
        if names_a_service && !services.iter().any(|s| s == service) {
            services.push(service.to_string());
        }
    }

    if !services.is_empty() {
        result.push_str(&format!("  Services: {}\n", services.join(", ")));
    }
    if !steps.is_empty() {
        result.push_str(&format!("  Steps: {}", steps.len()));
    }

    result.trim_end().to_string()
}

fn compact_ports(ports: &str) -> String {
    if ports.is_empty() {
        return "-".to_string();
    }

    // Extract just the port numbers
    let mut port_nums: Vec<&str> = Vec::new();
    for port in ports
        .split(',')
        .filter_map(|p| p.split("->").next().and_then(|s| s.split(':').next_back()))
        .map(str::trim)
    {
        // A published port appears once for IPv4 and once for IPv6.
        if !port_nums.contains(&port) {
            port_nums.push(port);
        }
    }

    if port_nums.len() <= 3 {
        port_nums.join(", ")
    } else {
        format!(
            "{}, … +{}",
            port_nums[..2].join(", "),
            port_nums.len() - 2
        )
    }
}

pub fn run_docker_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    crate::core::runner::run_passthrough("docker", args, verbose)
}

/// Run `docker compose ps` (or `docker compose ps -a`) with compact output
pub fn run_compose_ps(all: bool, verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut raw_args: Vec<&str> = vec!["compose", "ps"];
    if all {
        raw_args.push("-a");
    }
    let raw_result = exec_capture(resolved_command("docker").args(&raw_args))
        .context("Failed to run docker compose ps")?;

    if !raw_result.success() {
        eprintln!("{}", raw_result.stderr);
        return Ok(raw_result.exit_code);
    }
    let raw = raw_result.stdout;

    let mut format_args: Vec<&str> = vec!["compose", "ps"];
    if all {
        format_args.push("-a");
    }
    format_args.extend(["--format", "{{.Name}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}"]);
    let result = exec_capture(resolved_command("docker").args(&format_args))
        .context("Failed to run docker compose ps --format")?;

    if !result.success() {
        eprintln!("{}", result.stderr);
        return Ok(result.exit_code);
    }
    let structured = result.stdout;

    if verbose > 0 {
        eprintln!("raw docker compose ps:\n{}", raw);
    }

    let rtk = format_compose_ps(&structured);
    println!("{}", rtk);
    let label = if all { "docker compose ps -a" } else { "docker compose ps" };
    let rtk_label = if all { "bdo docker compose ps -a" } else { "bdo docker compose ps" };
    timer.track(label, rtk_label, &raw, &rtk);
    Ok(0)
}

pub fn run_compose_logs(service: Option<&str>, tail: u32, verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("docker");
    let tail_str = tail.to_string();
    cmd.args(["compose", "logs", "--tail", &tail_str]);
    if let Some(svc) = service {
        cmd.arg(svc);
    }

    let svc_label = service.unwrap_or("all");
    runner::run_filtered(
        cmd,
        "docker",
        &format!("compose logs {}", svc_label),
        |raw| {
            if verbose > 0 {
                eprintln!("raw docker compose logs:\n{}", raw);
            }
            let mut logs = format_compose_logs(raw);
            if raw.lines().count() >= tail as usize && tail > 0 {
                logs.push_str(&format!(
                    "\n[only the last {tail} lines of each service were read — use --tail N for more]"
                ));
            }
            logs
        },
        RunOptions::default().early_exit_on_failure(),
    )
}

pub fn run_compose_build(service: Option<&str>, verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("docker");
    cmd.args(["compose", "build"]);
    if let Some(svc) = service {
        cmd.arg(svc);
    }

    let svc_label = service.unwrap_or("all");
    runner::run_filtered(
        cmd,
        "docker",
        &format!("compose build {}", svc_label),
        |raw| {
            if verbose > 0 {
                eprintln!("raw docker compose build:\n{}", raw);
            }
            format_compose_build(raw)
        },
        RunOptions::default().early_exit_on_failure(),
    )
}

pub fn run_compose_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    let mut combined = vec![OsString::from("compose")];
    combined.extend_from_slice(args);
    crate::core::runner::run_passthrough("docker", &combined, verbose)
}

pub fn run_kubectl_get(args: &[String], verbose: u8) -> Result<i32> {
    match kubectl_get_target(args) {
        Some(("pods", rest)) => run(ContainerCmd::KubectlPods, rest, verbose),
        Some(("services", rest)) => run(ContainerCmd::KubectlServices, rest, verbose),
        _ => run_kubectl_get_passthrough(args, verbose),
    }
}

fn kubectl_get_target(args: &[String]) -> Option<(&'static str, &[String])> {
    let resource = args.first()?.as_str();
    let rest = &args[1..];
    if kubectl_get_requests_raw_output(rest) {
        return None;
    }

    match resource {
        "po" | "pod" | "pods" => Some(("pods", rest)),
        "svc" | "service" | "services" => Some(("services", rest)),
        _ => None,
    }
}

fn kubectl_get_requests_raw_output(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "-o" | "--output"
                | "-w"
                | "--watch"
                | "--show-labels"
                | "--show-kind"
                | "--no-headers"
                | "--sort-by"
                | "--label-columns"
                | "--template"
        ) || arg.starts_with("-o")
            || arg.starts_with("-L")
            || arg.starts_with("--output=")
            || arg.starts_with("--no-headers=")
            || arg.starts_with("--sort-by=")
            || arg.starts_with("--label-columns=")
            || arg.starts_with("--template=")
    })
}

fn run_kubectl_get_passthrough(args: &[String], verbose: u8) -> Result<i32> {
    let passthrough_args: Vec<OsString> = std::iter::once(OsString::from("get"))
        .chain(args.iter().map(|arg| OsString::from(arg.as_str())))
        .collect();
    run_kubectl_passthrough(&passthrough_args, verbose)
}

pub fn run_kubectl_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    crate::core::runner::run_passthrough("kubectl", args, verbose)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── format_compose_ps ──────────────────────────────────

    #[test]
    fn test_format_compose_ps_basic() {
        // Tab-separated --format output: Name\tImage\tStatus\tPorts
        let raw = "web-1\tnginx:latest\tUp 2 hours\t0.0.0.0:80->80/tcp\n\
                   api-1\tnode:20\tUp 2 hours\t0.0.0.0:3000->3000/tcp\n\
                   db-1\tpostgres:16\tUp 2 hours\t0.0.0.0:5432->5432/tcp";
        let out = format_compose_ps(raw);
        assert!(out.contains("3"), "should show container count");
        assert!(out.contains("web"), "should show service name");
        assert!(out.contains("api"), "should show service name");
        assert!(out.contains("db"), "should show service name");
        assert!(out.contains("Up 2 hours"), "should show status");
        assert!(out.len() < raw.len(), "output should be shorter than raw");
    }

    #[test]
    fn test_format_compose_ps_empty() {
        let out = format_compose_ps("");
        assert!(out.contains("0"), "should show zero containers");
    }

    #[test]
    fn test_format_compose_ps_whitespace_only() {
        let out = format_compose_ps("   \n  \n");
        assert!(out.contains("0"), "should show zero containers");
    }

    #[test]
    fn test_format_compose_ps_exited_service() {
        // Tab-separated --format output
        let raw = "worker-1\tpython:3.12\tExited (1) 2 minutes ago\t";
        let out = format_compose_ps(raw);
        assert!(out.contains("worker"), "should show service name");
        assert!(out.contains("Exited"), "should show exited status");
    }

    #[test]
    fn test_format_compose_ps_no_ports() {
        let raw = "redis-1\tredis:7\tUp 5 hours\t";
        let out = format_compose_ps(raw);
        assert!(out.contains("redis"), "should show service name");
        // Should not show port info when no ports (but [compose] prefix is OK)
        let lines: Vec<&str> = out.lines().collect();
        let redis_line = lines.iter().find(|l| l.contains("redis")).unwrap();
        assert!(
            !redis_line.contains("] ["),
            "should not show port brackets when empty"
        );
    }

    #[test]
    fn test_format_compose_ps_long_image_path() {
        let raw = "app-1\tghcr.io/myorg/myapp:latest\tUp 1 hour\t0.0.0.0:8080->8080/tcp";
        let out = format_compose_ps(raw);
        assert!(
            out.contains("myapp:latest"),
            "should shorten image to last segment"
        );
        assert!(
            !out.contains("ghcr.io"),
            "should not show full registry path"
        );
    }

    // ── format_compose_logs ────────────────────────────────

    #[test]
    fn test_format_compose_logs_basic() {
        let raw = "\
web-1  | 192.168.1.1 - GET / 200
web-1  | 192.168.1.1 - GET /favicon.ico 404
api-1  | Server listening on port 3000
api-1  | Connected to database";
        let out = format_compose_logs(raw);
        assert!(out.contains("Logs"), "should have compose logs header");
    }

    #[test]
    fn test_format_compose_logs_empty() {
        let out = format_compose_logs("");
        assert!(out.contains("No logs"), "should indicate no logs");
    }

    // ── format_compose_build ───────────────────────────────

    #[test]
    fn test_format_compose_build_basic() {
        let raw = "\
[+] Building 12.3s (8/8) FINISHED
 => [web internal] load build definition from Dockerfile           0.0s
 => [web internal] load metadata for docker.io/library/node:20     1.2s
 => [web 1/4] FROM docker.io/library/node:20@sha256:abc123         0.0s
 => [web 2/4] WORKDIR /app                                         0.1s
 => [web 3/4] COPY package*.json ./                                0.1s
 => [web 4/4] RUN npm install                                      8.5s
 => [web] exporting to image                                       2.3s
 => => naming to docker.io/library/myapp-web                       0.0s";
        let out = format_compose_build(raw);
        assert!(out.contains("12.3s"), "should show total build time");
        assert!(out.contains("Services: web\n"), "should name the service once: {out}");
        assert!(out.contains("Steps: 4"), "should count the Dockerfile's steps: {out}");
        assert!(out.len() < raw.len(), "should be shorter than raw");
    }

    #[test]
    fn test_format_compose_build_empty() {
        let out = format_compose_build("");
        assert!(
            !out.is_empty(),
            "should produce output even for empty input"
        );
    }

    // ── compact_ports (existing, previously untested) ──────

    #[test]
    fn test_compact_ports_empty() {
        assert_eq!(compact_ports(""), "-");
    }

    #[test]
    fn test_compact_ports_single() {
        let result = compact_ports("0.0.0.0:8080->80/tcp");
        assert!(result.contains("8080"));
    }

    #[test]
    fn test_compact_ports_counts_a_port_once_across_address_families() {
        assert_eq!(
            compact_ports("0.0.0.0:80->80/tcp, [::]:80->80/tcp, :::443->443/tcp, 0.0.0.0:443->443/tcp"),
            "80, 443"
        );
    }

    #[test]
    fn test_images_total_picks_the_unit_that_fits() {
        assert_eq!(images_total(&["hello:1\t13.3kB"]), "13kB");
        assert_eq!(images_total(&["a:1\t187MB", "b:1\t13.3kB"]), "187MB");
        assert_eq!(images_total(&["a:1\t1.61GB", "b:1\t187MB"]), "1.8GB");
        assert_eq!(images_total(&["a:1\tN/A"]), "0kB");
    }

    #[test]
    fn test_image_size_mb_reads_docker_units() {
        for (size, mb) in [
            ("1.61GB", 1610.0),
            ("187MB", 187.0),
            ("13.3kB", 0.0133),
            ("512B", 0.000512),
            ("1.2TB", 1_200_000.0),
        ] {
            let found = image_size_mb(size).unwrap();
            assert!((found - mb).abs() < 1e-6, "{size}: {found}");
        }
        assert_eq!(image_size_mb("N/A"), None);
    }

    #[test]
    fn test_format_compose_build_reads_captured_output() {
        let raw = r#"#0 building with "desktop-linux" instance using docker driver

#1 [web internal] load build definition from Dockerfile
#1 DONE 0.0s

#4 [web 1/3] FROM docker.io/library/node:22-bookworm@sha256:82693da
#4 resolve docker.io/library/node:22-bookworm 1.3s done
#4 DONE 1.3s

#5 [web 2/3] RUN echo "[INFO] building"
#5 0.172 [INFO] building
#5 0.180 Building the docs
#5 DONE 0.2s

#6 [web 3/3] WORKDIR /app
#6 DONE 0.0s

#7 exporting to image
 web  Built"#;
        assert_eq!(
            format_compose_build(raw),
            "[compose] Build:\n  Services: web\n  Steps: 3"
        );
    }

    #[test]
    fn test_format_compose_build_names_cached_services() {
        assert_eq!(
            format_compose_build(" web  Built\n ✔ api  Built\n"),
            "[compose] Build:\n  Services: web, api"
        );
    }

    #[test]
    fn test_compact_ports_many() {
        let result = compact_ports("0.0.0.0:80->80/tcp, 0.0.0.0:443->443/tcp, 0.0.0.0:8080->8080/tcp, 0.0.0.0:9090->9090/tcp");
        assert!(result.contains("…"), "should truncate for >3 ports");
    }

    #[test]
    fn test_kubectl_get_target_pods_aliases() {
        for resource in ["po", "pod", "pods"] {
            let args = vec![resource.to_string(), "-n".to_string(), "default".to_string()];

            assert_eq!(
                kubectl_get_target(&args),
                Some(("pods", &args[1..])),
                "failed for {resource}"
            );
        }
    }

    #[test]
    fn test_kubectl_get_target_services_aliases() {
        for resource in ["svc", "service", "services"] {
            let args = vec![resource.to_string(), "-A".to_string()];

            assert_eq!(
                kubectl_get_target(&args),
                Some(("services", &args[1..])),
                "failed for {resource}"
            );
        }
    }

    #[test]
    fn test_kubectl_get_target_unsupported_resource() {
        let args = vec!["deployments".to_string()];

        assert_eq!(kubectl_get_target(&args), None);
    }

    #[test]
    fn test_kubectl_get_target_respects_output_flags() {
        for output_flag in [
            "-o",
            "-owide",
            "--output",
            "--output=json",
            "-L",
            "-Lapp",
            "--label-columns=app",
            "--no-headers",
            "--sort-by=.metadata.name",
            "--template={{.}}",
        ] {
            let args = vec![
                "pods".to_string(),
                output_flag.to_string(),
                "wide".to_string(),
            ];

            assert_eq!(
                kubectl_get_target(&args),
                None,
                "should pass through {output_flag}"
            );
        }
    }
}
