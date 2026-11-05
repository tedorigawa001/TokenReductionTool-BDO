//! `bdo docker` and `bdo kubectl` with the real clients, talking to a server of
//! this file's own over `DOCKER_HOST` and `KUBECONFIG`. Nothing is created in a real
//! daemon or cluster, and the clients do their own formatting and report their own
//! errors. Compose runs through a stand-in that replays what real compose printed.
//! The tests skip where a client is missing. Unix only: the stand-in is a shell script.
#![cfg(unix)]

use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn installed(tool: &str, version_args: &[&str]) -> bool {
    let found = Command::new(tool).args(version_args).output().is_ok();
    if !found {
        eprintln!("{tool} is not installed: skipping");
    }
    found
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn temp_dir() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().canonicalize().unwrap();
    (tmp, path)
}

fn bdo_command(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bdo"));
    command
        .current_dir(dir)
        .env("HOME", dir)
        .env("BDO_DB_PATH", dir.join("track.db"))
        .env("BDO_TELEMETRY_DISABLED", "1")
        .env("NO_PROXY", "127.0.0.1")
        .env("no_proxy", "127.0.0.1");
    command
}

struct Response {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

fn json_response(status: &'static str, body: &Value) -> Response {
    Response {
        status,
        content_type: "application/json",
        body: body.to_string().into_bytes(),
    }
}

/// An HTTP server on a thread of its own, one connection per request, each on a
/// thread too: clients ask for several things at once.
fn serve(route: fn(&str) -> Response) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let mut request = [0u8; 8192];
                let read = stream.read(&mut request).unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]).into_owned();
                let target = request.split_whitespace().nth(1).unwrap_or("/");
                let response = route(target);
                let head = format!(
                    "HTTP/1.1 {}\r\nContent-Type: {}\r\nApi-Version: 1.47\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.status,
                    response.content_type,
                    response.body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&response.body);
            });
        }
    });
    address
}

fn query_value<'a>(target: &'a str, key: &str) -> Option<&'a str> {
    target
        .split_once('?')?
        .1
        .split('&')
        .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
}

/// The last `tail` of the numbered lines a server holds, as logs APIs answer.
fn numbered_log(total: usize, tail: Option<&str>) -> Vec<String> {
    let lines: Vec<String> = (1..=total)
        .map(|i| format!("line {i} of the log"))
        .collect();
    let keep = tail
        .and_then(|t| t.parse().ok())
        .unwrap_or(total)
        .min(total);
    lines[total - keep..].to_vec()
}

// ---- kubectl ----

fn pod(name: &str, status: Value) -> Value {
    json!({
        "kind": "Pod",
        "apiVersion": "v1",
        "metadata": {"name": name, "namespace": "default"},
        "spec": {"containers": [{"name": "app", "image": "app:1"}]},
        "status": status,
    })
}

fn ready_container() -> Value {
    json!({"name": "app", "ready": true, "restartCount": 0, "state": {"running": {}}})
}

fn waiting_container(reason: &str, restarts: i64) -> Value {
    json!({"name": "app", "ready": false, "restartCount": restarts, "state": {"waiting": {"reason": reason}}})
}

fn pod_named(name: &str) -> Option<Value> {
    Some(match name {
        "web" => pod(
            name,
            json!({"phase": "Running", "containerStatuses": [ready_container()]}),
        ),
        // These keep the phases `Running` and `Pending` while they cannot run.
        "crash" => pod(
            name,
            json!({"phase": "Running", "containerStatuses": [waiting_container("CrashLoopBackOff", 12)]}),
        ),
        "pull" => pod(
            name,
            json!({"phase": "Pending", "containerStatuses": [waiting_container("ImagePullBackOff", 0)]}),
        ),
        "init" => pod(
            name,
            json!({"phase": "Pending", "initContainerStatuses": [waiting_container("CrashLoopBackOff", 3)]}),
        ),
        "oom" => pod(
            name,
            json!({"phase": "Running", "containerStatuses": [{
                "name": "app", "ready": false, "restartCount": 1,
                "state": {"terminated": {"reason": "OOMKilled", "exitCode": 137}},
            }]}),
        ),
        "notready" => pod(
            name,
            json!({"phase": "Running", "containerStatuses": [{
                "name": "app", "ready": false, "restartCount": 0, "state": {"running": {}},
            }]}),
        ),
        "creating" => pod(
            name,
            json!({"phase": "Pending", "containerStatuses": [waiting_container("ContainerCreating", 0)]}),
        ),
        "unscheduled" => pod(
            name,
            json!({"phase": "Pending", "conditions": [
                {"type": "PodScheduled", "status": "False", "reason": "Unschedulable"},
            ]}),
        ),
        "job" => pod(
            name,
            json!({"phase": "Succeeded", "containerStatuses": [{
                "name": "app", "ready": false, "restartCount": 0,
                "state": {"terminated": {"reason": "Completed", "exitCode": 0}},
            }]}),
        ),
        "evicted" => pod(name, json!({"phase": "Failed", "reason": "Evicted"})),
        "gone" => {
            let mut gone = pod(
                name,
                json!({"phase": "Running", "containerStatuses": [ready_container()]}),
            );
            gone["metadata"]["deletionTimestamp"] = json!("2026-11-05T00:00:00Z");
            gone
        }
        _ => return None,
    })
}

const ALL_PODS: [&str; 11] = [
    "web",
    "crash",
    "pull",
    "init",
    "oom",
    "notready",
    "creating",
    "unscheduled",
    "job",
    "evicted",
    "gone",
];

fn service_named(name: &str) -> Option<Value> {
    let spec = match name {
        "same" => json!({"type": "ClusterIP", "ports": [{"port": 80, "targetPort": 80}]}),
        "mapped" => json!({"type": "ClusterIP", "ports": [{"port": 80, "targetPort": 8080}]}),
        "named" => json!({"type": "ClusterIP", "ports": [{"port": 80, "targetPort": "http"}]}),
        "node" => {
            json!({"type": "NodePort", "ports": [{"port": 443, "targetPort": 8443, "nodePort": 30443}]})
        }
        "lb" => json!({"type": "LoadBalancer", "ports": [{"port": 80, "targetPort": 80}]}),
        _ => return None,
    };
    let mut service = json!({
        "kind": "Service",
        "apiVersion": "v1",
        "metadata": {"name": name, "namespace": "default"},
        "spec": spec,
        "status": {},
    });
    if name == "lb" {
        service["status"] = json!({"loadBalancer": {"ingress": [{"ip": "203.0.113.9"}]}});
    }
    Some(service)
}

fn kubernetes_status(status: &'static str, code: u16, reason: &str, message: &str) -> Response {
    json_response(
        status,
        &json!({"kind": "Status", "apiVersion": "v1", "status": "Failure",
                "reason": reason, "message": message, "code": code}),
    )
}

fn list(kind: &str, items: Vec<Value>) -> Value {
    json!({"kind": kind, "apiVersion": "v1", "metadata": {}, "items": items})
}

fn kubernetes_api(target: &str) -> Response {
    let path = target.split('?').next().unwrap_or(target);
    let ok = "200 OK";
    match path {
        "/api" => {
            return json_response(
                ok,
                &json!({"kind": "APIVersions", "versions": ["v1"], "serverAddressByClientCIDRs": []}),
            )
        }
        "/apis" => {
            return json_response(
                ok,
                &json!({"kind": "APIGroupList", "apiVersion": "v1", "groups": []}),
            )
        }
        "/api/v1" => {
            return json_response(
                ok,
                &json!({"kind": "APIResourceList", "groupVersion": "v1", "resources": [
                    {"name": "pods", "singularName": "pod", "namespaced": true, "kind": "Pod",
                     "verbs": ["get", "list"], "shortNames": ["po"]},
                    {"name": "services", "singularName": "service", "namespaced": true, "kind": "Service",
                     "verbs": ["get", "list"], "shortNames": ["svc"]},
                ]}),
            )
        }
        "/api/v1/pods" | "/api/v1/namespaces/default/pods" => {
            let items = ALL_PODS.iter().filter_map(|name| pod_named(name)).collect();
            return json_response(ok, &list("PodList", items));
        }
        "/api/v1/namespaces/healthy/pods" => {
            let mut web = pod_named("web").unwrap_or_default();
            web["metadata"]["namespace"] = json!("healthy");
            return json_response(ok, &list("PodList", vec![web]));
        }
        "/api/v1/namespaces/empty/pods" => return json_response(ok, &list("PodList", vec![])),
        "/api/v1/namespaces/empty/services" => {
            return json_response(ok, &list("ServiceList", vec![]))
        }
        "/api/v1/services" | "/api/v1/namespaces/default/services" => {
            let names = ["same", "mapped", "named", "node", "lb"];
            let items = names
                .iter()
                .filter_map(|name| service_named(name))
                .collect();
            return json_response(ok, &list("ServiceList", items));
        }
        _ => {}
    }
    if path.starts_with("/api/v1/namespaces/secret/") {
        return kubernetes_status(
            "403 Forbidden",
            403,
            "Forbidden",
            "pods is forbidden: User \"me\" cannot list resource \"pods\" in the namespace \"secret\"",
        );
    }
    if let Some(rest) = path.strip_prefix("/api/v1/namespaces/default/pods/") {
        if let Some(name) = rest.strip_suffix("/log") {
            let total = if name == "short" { 3 } else { 150 };
            let body = numbered_log(total, query_value(target, "tailLines")).join("\n") + "\n";
            return Response {
                status: ok,
                content_type: "text/plain",
                body: body.into_bytes(),
            };
        }
        if rest == "short" {
            let mut short = pod_named("web").unwrap_or_default();
            short["metadata"]["name"] = json!("short");
            return json_response(ok, &short);
        }
        if let Some(found) = pod_named(rest) {
            return json_response(ok, &found);
        }
        return kubernetes_status(
            "404 Not Found",
            404,
            "NotFound",
            &format!("pods \"{rest}\" not found"),
        );
    }
    if let Some(name) = path.strip_prefix("/api/v1/namespaces/default/services/") {
        if let Some(found) = service_named(name) {
            return json_response(ok, &found);
        }
    }
    kubernetes_status("404 Not Found", 404, "NotFound", "not found")
}

struct Kube {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
}

impl Kube {
    /// `None` where kubectl is missing.
    fn start() -> Option<Self> {
        if !installed("kubectl", &["version", "--client"]) {
            return None;
        }
        let (tmp, dir) = temp_dir();
        let address = serve(kubernetes_api);
        fs::write(
            dir.join("kubeconfig"),
            format!(
                "apiVersion: v1\nkind: Config\n\
                 clusters: [{{name: fake, cluster: {{server: \"http://{address}\"}}}}]\n\
                 users: [{{name: me, user: {{}}}}]\n\
                 contexts: [{{name: fake, context: {{cluster: fake, user: me, namespace: default}}}}]\n\
                 current-context: fake\n"
            ),
        )
        .unwrap();
        Some(Self { _tmp: tmp, dir })
    }

    fn bdo(&self, args: &[&str]) -> Output {
        bdo_command(&self.dir)
            .arg("kubectl")
            .args(args)
            .env("KUBECONFIG", self.dir.join("kubeconfig"))
            .output()
            .unwrap()
    }
}

/// Every pod is counted exactly once, and every one that needs a look is named
/// with its reason. The phase alone would call `crash` and `oom` running.
#[test]
fn kubectl_pods_counts_every_pod_and_names_what_is_wrong() {
    let Some(kube) = Kube::start() else { return };
    let expected = "\
11 pods: 1 running, 1 completed, 2 pending, 1 not ready, 1 terminating, 5 [x], 16 restarts
[warn] Issues:
  default/crash CrashLoopBackOff
  default/pull ImagePullBackOff
  default/init CrashLoopBackOff
  default/oom OOMKilled
  default/evicted Evicted
  default/notready not ready
  default/creating Pending (ContainerCreating)
  default/unscheduled Pending (Unschedulable)
  default/gone Terminating
";
    for args in [
        vec!["pods"],
        vec!["pods", "-A"],
        vec!["get", "pods"],
        vec!["get", "po"],
        vec!["get", "pod", "-A"],
    ] {
        let out = kube.bdo(&args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        assert_eq!(stdout(&out), expected, "{args:?}");
    }
}

/// `get pods web` answers with the pod, not a list.
#[test]
fn kubectl_get_one_pod_reports_it() {
    let Some(kube) = Kube::start() else { return };
    for (name, expected) in [
        ("web", "1 pods: 1 running\n"),
        (
            "crash",
            "1 pods: 1 [x], 12 restarts\n[warn] Issues:\n  default/crash CrashLoopBackOff\n",
        ),
    ] {
        let out = kube.bdo(&["get", "pods", name]);
        assert!(out.status.success(), "{name}: {out:?}");
        assert_eq!(stdout(&out), expected, "{name}");
    }
    let out = kube.bdo(&["get", "pods", "web", "crash"]);
    assert_eq!(
        stdout(&out),
        "2 pods: 1 running, 1 [x], 12 restarts\n[warn] Issues:\n  default/crash CrashLoopBackOff\n"
    );
}

#[test]
fn kubectl_healthy_and_empty_namespaces_are_short() {
    let Some(kube) = Kube::start() else { return };
    let healthy = kube.bdo(&["pods", "-n", "healthy"]);
    assert_eq!(stdout(&healthy), "1 pods: 1 running\n", "{healthy:?}");
    assert_eq!(
        stdout(&kube.bdo(&["pods", "-n", "empty"])),
        "No pods found\n"
    );
    assert_eq!(
        stdout(&kube.bdo(&["services", "-n", "empty"])),
        "No services found\n"
    );
}

/// bdo added `-o json`, so the empty List kubectl prints with it is not the user's.
#[test]
fn kubectl_refusals_show_only_the_reason() {
    let Some(kube) = Kube::start() else { return };
    let out = kube.bdo(&["get", "pods", "-n", "secret"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(stdout(&out), "");
    assert!(stderr(&out).contains("Forbidden"), "{out:?}");

    let out = kube.bdo(&["get", "pods", "nosuch"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(stderr(&out).contains("\"nosuch\" not found"), "{out:?}");
}

#[test]
fn kubectl_services_keep_ports_and_outside_addresses() {
    let Some(kube) = Kube::start() else { return };
    let expected = "\
5 services:
  default/same ClusterIP [80]
  default/mapped ClusterIP [80→8080]
  default/named ClusterIP [80→http]
  default/node NodePort [443:30443→8443]
  default/lb LoadBalancer [80] external 203.0.113.9
";
    for args in [
        vec!["services"],
        vec!["get", "svc"],
        vec!["get", "services", "-A"],
    ] {
        let out = kube.bdo(&args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        assert_eq!(stdout(&out), expected, "{args:?}");
    }
    assert_eq!(
        stdout(&kube.bdo(&["get", "svc", "lb"])),
        "1 services:\n  default/lb LoadBalancer [80] external 203.0.113.9\n"
    );
}

/// What these ask for is in the table, which a summary would lose.
#[test]
fn kubectl_get_passes_through_what_a_summary_would_lose() {
    let Some(kube) = Kube::start() else { return };
    let out = kube.bdo(&["get", "pods", "-o", "name"]);
    assert!(stdout(&out).starts_with("pod/web\npod/crash\n"), "{out:?}");
    for args in [
        vec!["get", "pods", "--no-headers"],
        vec!["get", "pods", "-L", "app"],
        vec!["get", "pods", "--sort-by=.metadata.name"],
    ] {
        let text = stdout(&kube.bdo(&args));
        assert!(!text.contains("pods:"), "{args:?}: {text}");
        assert!(text.contains("crash"), "{args:?}: {text}");
    }
}

#[test]
fn kubectl_logs_say_when_only_the_end_was_read() {
    let Some(kube) = Kube::start() else { return };
    let text = stdout(&kube.bdo(&["logs", "web"]));
    assert!(text.starts_with("Logs for web:\n"), "{text}");
    assert!(text.contains("Log Summary (100 lines)"), "{text}");
    assert!(text.contains("line 150 of the log"), "{text}");
    assert!(
        text.contains(
            "[only the last 100 lines were read — use `bdo proxy kubectl logs web` for all]"
        ),
        "{text}"
    );

    let out = kube.bdo(&["logs", "short"]);
    assert_eq!(
        stdout(&out),
        "Logs for short:\nline 1 of the log\nline 2 of the log\nline 3 of the log\n"
    );
}

/// The hint has to work.
#[test]
fn kubectl_logs_hint_gets_the_whole_log() {
    let Some(kube) = Kube::start() else { return };
    let out = bdo_command(&kube.dir)
        .args(["proxy", "kubectl", "logs", "web"])
        .env("KUBECONFIG", kube.dir.join("kubeconfig"))
        .output()
        .unwrap();
    let text = stdout(&out);
    assert!(text.contains("line 1 of the log\n"), "{out:?}");
    assert!(text.contains("line 150 of the log\n"), "{out:?}");
}

#[test]
fn kubectl_logs_flags_pass_through() {
    let Some(kube) = Kube::start() else { return };
    let text = stdout(&kube.bdo(&["logs", "web", "--tail=2"]));
    assert_eq!(text, "line 149 of the log\nline 150 of the log\n");
}

#[test]
fn kubectl_without_a_cluster_keeps_the_error() {
    if !installed("kubectl", &["version", "--client"]) {
        return;
    }
    let (_tmp, dir) = temp_dir();
    let out = bdo_command(&dir)
        .args(["kubectl", "get", "pods"])
        .env("KUBECONFIG", dir.join("none"))
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0), "{out:?}");
    assert!(stderr(&out).contains("connection refused"), "{out:?}");
}

// ---- docker ----

const LONG_ID: &str = "df32f90844ee0000000000000000000000000000000000000000000000000000";

fn container(name: &str, state: &str, status: &str, image: &str, ports: Value) -> Value {
    json!({
        "Id": format!("{}{}", &name[..4], &LONG_ID[4..]),
        "Names": [format!("/{name}")],
        "Image": image,
        "ImageID": "sha256:abc",
        "Command": "sleep 300",
        "Created": 1_700_000_000,
        "State": state,
        "Status": status,
        "Ports": ports,
        "Labels": {},
        "HostConfig": {},
        "NetworkSettings": {"Networks": {}},
        "Mounts": [],
    })
}

fn image(tag: &str, size: u64) -> Value {
    json!({
        "Id": format!("sha256:{tag}"),
        "RepoTags": [tag],
        "RepoDigests": [],
        "ParentId": "",
        "Created": 1_700_000_000,
        "Size": size,
        "SharedSize": -1,
        "VirtualSize": size,
        "Labels": {},
        "Containers": -1,
    })
}

/// One log line per frame, in the stream the API multiplexes logs into.
fn docker_log_frames(lines: &[String]) -> Vec<u8> {
    let mut body = Vec::new();
    for line in lines {
        let payload = format!("{line}\n");
        body.extend([1u8, 0, 0, 0]);
        body.extend((payload.len() as u32).to_be_bytes());
        body.extend(payload.as_bytes());
    }
    body
}

fn docker_api(target: &str) -> Response {
    let ok = "200 OK";
    let path = target.split('?').next().unwrap_or(target);
    // Versioned or not: /v1.47/containers/json, /_ping.
    let route = match path.strip_prefix("/v") {
        Some(rest) => rest.split_once('/').map_or("", |(_, r)| r),
        None => path.trim_start_matches('/'),
    };
    let up = || {
        container(
            "bdo-up",
            "running",
            "Up 2 hours (healthy)",
            "ghcr.io/org/app:1",
            json!([
                {"IP": "0.0.0.0", "PrivatePort": 80, "PublicPort": 18080, "Type": "tcp"},
                {"IP": "::", "PrivatePort": 80, "PublicPort": 18080, "Type": "tcp"},
                {"PrivatePort": 9000, "Type": "tcp"},
            ]),
        )
    };
    let paused = || {
        container(
            "bdo-paused",
            "paused",
            "Up 1 hour (Paused)",
            "node:22",
            json!([
                {"IP": "0.0.0.0", "PrivatePort": 3000, "PublicPort": 3000, "Type": "tcp"},
            ]),
        )
    };
    match route {
        "_ping" => Response {
            status: ok,
            content_type: "text/plain",
            body: b"OK".to_vec(),
        },
        "containers/json" => {
            let mut all = vec![up(), paused()];
            if query_value(target, "all") == Some("1") {
                all.push(container(
                    "bdo-exit",
                    "exited",
                    "Exited (3) 1 hour ago",
                    "node:22",
                    json!([]),
                ));
                all.push(container(
                    "bdo-new",
                    "created",
                    "Created",
                    "node:22",
                    json!([]),
                ));
            }
            json_response(ok, &Value::Array(all))
        }
        "images/json" => json_response(
            ok,
            &json!([
                image("node:22", 1_610_000_000),
                image("redis:7", 187_000_000),
                image("hello:1", 13_300),
            ]),
        ),
        "containers/long/json" | "containers/short/json" => json_response(
            ok,
            // The client asks for logs by this id.
            &json!({"Id": route.split('/').nth(1), "Name": "/x", "Config": {"Tty": false}, "State": {"Running": true}}),
        ),
        "containers/long/logs" | "containers/short/logs" => {
            let total = if route.contains("short") { 3 } else { 150 };
            Response {
                status: ok,
                content_type: "application/vnd.docker.raw-stream",
                body: docker_log_frames(&numbered_log(total, query_value(target, "tail"))),
            }
        }
        _ => json_response(
            "404 Not Found",
            &json!({"message": format!("No such container: {route}")}),
        ),
    }
}

struct Docker {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    host: String,
}

impl Docker {
    /// `None` where docker is missing.
    fn start() -> Option<Self> {
        if !installed("docker", &["--version"]) {
            return None;
        }
        let (tmp, dir) = temp_dir();
        let host = format!("tcp://{}", serve(docker_api));
        Some(Self {
            _tmp: tmp,
            dir,
            host,
        })
    }

    fn bdo(&self, args: &[&str]) -> Output {
        bdo_command(&self.dir)
            .arg("docker")
            .args(args)
            .env("DOCKER_HOST", &self.host)
            .env_remove("DOCKER_CONTEXT")
            .env_remove("DOCKER_TLS_VERIFY")
            .output()
            .unwrap()
    }
}

/// docker prints a port published on both IPv4 and IPv6 in one go or twice,
/// depending on its version, so this checks the count rather than the raw text.
#[test]
fn docker_ps_is_compact_and_counts_a_port_once() {
    let Some(docker) = Docker::start() else {
        return;
    };
    let out = docker.bdo(&["ps"]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    assert!(text.starts_with("[docker] 2 containers:\n"), "{text}");
    assert!(
        text.contains("bdo-up (app:1) Up 2 hours (healthy) ["),
        "{text}"
    );
    assert_eq!(text.matches("18080").count(), 1, "{text}");
    assert!(text.contains("9000"), "{text}");
    assert!(
        text.contains("bdo-paused (node:22) Up 1 hour (Paused) [3000]"),
        "{text}"
    );
}

/// `docker ps` lists a paused container, so `-a` cannot call it stopped.
#[test]
fn docker_ps_all_separates_running_from_stopped() {
    let Some(docker) = Docker::start() else {
        return;
    };
    let out = docker.bdo(&["ps", "-a"]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    let (running, stopped) = text
        .split_once("[docker] 2 stopped/exited:\n")
        .unwrap_or(("", ""));
    assert!(running.starts_with("[docker] 2 running:\n"), "{text}");
    assert!(running.contains("bdo-up"), "{text}");
    assert!(running.contains("bdo-paused"), "{text}");
    assert!(
        stopped.contains("bdo-exit (node:22) Exited (3) 1 hour ago"),
        "{text}"
    );
    assert!(stopped.contains("bdo-new (node:22) Created"), "{text}");
}

/// Small images count: a total of `0MB` would be wrong for these three.
#[test]
fn docker_images_totals_every_unit() {
    let Some(docker) = Docker::start() else {
        return;
    };
    let out = docker.bdo(&["images"]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    assert!(text.starts_with("[docker] 3 images (1.8GB)\n"), "{text}");
    assert!(text.contains("  hello:1 [13.3kB]\n"), "{text}");
    assert!(text.contains("  node:22 [1.61GB]\n"), "{text}");
}

#[test]
fn docker_flags_a_summary_cannot_honour_pass_through() {
    let Some(docker) = Docker::start() else {
        return;
    };
    assert_eq!(
        stdout(&docker.bdo(&["ps", "--format", "{{.Names}}"])),
        "bdo-up\nbdo-paused\n"
    );
    let out = docker.bdo(&["ps", "-q"]);
    assert_eq!(stdout(&out).lines().count(), 2, "{out:?}");
    assert!(stdout(&docker.bdo(&["images", "--format", "{{.Size}}"])).contains("13.3kB"));
}

#[test]
fn docker_logs_show_a_short_log_whole_and_the_end_of_a_long_one() {
    let Some(docker) = Docker::start() else {
        return;
    };
    let short = docker.bdo(&["logs", "short"]);
    assert_eq!(
        stdout(&short),
        "[docker] Logs for short:\nline 1 of the log\nline 2 of the log\nline 3 of the log\n",
        "{short:?}"
    );

    let text = stdout(&docker.bdo(&["logs", "long"]));
    assert!(
        text.starts_with("[docker] Logs for long:\nLog Summary (100 lines)"),
        "{text}"
    );
    assert!(text.contains("[LAST 10 LINES]"), "{text}");
    assert!(text.contains("line 150 of the log"), "{text}");
    assert!(!text.contains("line 100 of the log"), "{text}");
    assert!(
        text.contains(
            "[only the last 100 lines were read — use `bdo proxy docker logs long` for all]"
        ),
        "{text}"
    );
}

#[test]
fn docker_logs_of_a_missing_container_keep_the_error() {
    let Some(docker) = Docker::start() else {
        return;
    };
    let out = docker.bdo(&["logs", "nosuch"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(stderr(&out).contains("No such container"), "{out:?}");
}

#[test]
fn docker_without_a_daemon_keeps_the_error() {
    if !installed("docker", &["--version"]) {
        return;
    }
    let (_tmp, dir) = temp_dir();
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    for args in [["docker", "ps"], ["docker", "images"]] {
        let out = bdo_command(&dir)
            .args(args)
            .env("DOCKER_HOST", format!("tcp://{address}"))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}: {out:?}");
        assert!(stderr(&out).contains("Cannot connect"), "{args:?}: {out:?}");
    }
}

// ---- docker compose, replayed ----

/// A `docker` that logs its arguments, and answers `compose <name>` from the file
/// `<name>.out` and `<name>.err`, exiting with what `<name>.code` holds.
struct Compose {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Compose {
    fn new() -> Self {
        let (tmp, root) = temp_dir();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        let script = bin.join("docker");
        fs::write(
            &script,
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> \"$ROOT/args\"\n\
             [ -f \"$ROOT/$2.err\" ] && cat \"$ROOT/$2.err\" >&2\n\
             [ -f \"$ROOT/$2.out\" ] && cat \"$ROOT/$2.out\"\n\
             [ -f \"$ROOT/$2.code\" ] && exit \"$(cat \"$ROOT/$2.code\")\"\n\
             exit 0\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        Self { _tmp: tmp, root }
    }

    fn answer(self, command: &str, stdout: &str, stderr: &str, code: i32) -> Self {
        fs::write(self.root.join(format!("{command}.out")), stdout).unwrap();
        fs::write(self.root.join(format!("{command}.err")), stderr).unwrap();
        fs::write(self.root.join(format!("{command}.code")), code.to_string()).unwrap();
        self
    }

    fn bdo(&self, args: &[&str]) -> Output {
        bdo_command(&self.root)
            .args(["docker", "compose"])
            .args(args)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("ROOT", &self.root)
            .output()
            .unwrap()
    }

    fn args(&self) -> String {
        fs::read_to_string(self.root.join("args")).unwrap_or_default()
    }
}

/// Compose 2.33's captured output for a service built from scratch: on a pipe it
/// has no `[+] Building … FINISHED` line, and a step's output can hold brackets.
const COMPOSE_BUILD_STDOUT: &str = r#"#0 building with "desktop-linux" instance using docker driver

#1 [web internal] load build definition from Dockerfile
#1 transferring dockerfile: 160B done
#1 DONE 0.0s

#2 [web internal] load metadata for docker.io/library/node:22-bookworm
#2 DONE 0.1s

#3 [web internal] load .dockerignore
#3 transferring context: 2B done
#3 DONE 0.0s

#4 [web 1/3] FROM docker.io/library/node:22-bookworm@sha256:82693da96a4e
#4 resolve docker.io/library/node:22-bookworm@sha256:82693da96a4e 1.3s done
#4 DONE 1.3s

#4 [web 1/3] FROM docker.io/library/node:22-bookworm@sha256:82693da96a4e
#4 CACHED

#5 [web 2/3] RUN echo "[INFO] building" && echo "[notice] step"
#5 0.172 [INFO] building
#5 0.173 [notice] step
#5 DONE 0.2s

#6 [web 3/3] WORKDIR /app
#6 DONE 0.0s

#7 [web] exporting to image
#7 DONE 0.1s
"#;

#[test]
fn compose_build_summarises_captured_output() {
    let compose = Compose::new().answer("build", COMPOSE_BUILD_STDOUT, " web  Built\n", 0);
    let out = compose.bdo(&["build"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        stdout(&out),
        "[compose] Build:\n  Services: web\n  Steps: 3\n"
    );
}

#[test]
fn compose_build_failure_keeps_the_reason_and_exit_code() {
    let compose = Compose::new().answer(
        "build",
        "#5 [bad 2/2] RUN echo about to fail && exit 7\n#5 0.172 about to fail\n\
         #5 ERROR: process did not complete successfully: exit code: 7\n",
        "failed to solve: process did not complete successfully: exit code: 7\n",
        1,
    );
    let out = compose.bdo(&["build"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(stdout(&out).contains("about to fail"), "{out:?}");
    assert!(stderr(&out).contains("failed to solve"), "{out:?}");
}

#[test]
fn compose_ps_reads_real_columns_and_counts_a_port_once() {
    let compose = Compose::new().answer(
        "ps",
        "bdo-probe-web-1\tbdo-probe-web\tUp 2 minutes\t0.0.0.0:18081->80/tcp, [::]:18081->80/tcp\n\
         bdo-probe-once-1\tnode:22-bookworm\tExited (2) 1 minute ago\t\n",
        "",
        0,
    );
    let out = compose.bdo(&["ps", "-a"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        stdout(&out),
        "[compose] 2 services:\n  bdo-probe-web-1 (bdo-probe-web) Up 2 minutes [18081]\n  \
         bdo-probe-once-1 (node:22-bookworm) Exited (2) 1 minute ago\n"
    );
    assert!(
        compose.args().contains("compose ps -a --format"),
        "{}",
        compose.args()
    );
}

#[test]
fn compose_ps_outside_a_project_keeps_the_error() {
    let compose = Compose::new().answer("ps", "", "no configuration file provided: not found\n", 1);
    let out = compose.bdo(&["ps"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        stderr(&out).contains("no configuration file provided"),
        "{out:?}"
    );
}

/// A service's only output must not disappear for not looking like an error.
#[test]
fn compose_logs_keep_ordinary_output() {
    let compose = Compose::new().answer("logs", "once-1  | once ran\n", "", 0);
    let out = compose.bdo(&["logs", "once"]);
    assert_eq!(stdout(&out), "[compose] Logs:\nonce-1  | once ran\n");
    assert!(
        compose.args().contains("compose logs --tail 100 once"),
        "{}",
        compose.args()
    );
}

#[test]
fn compose_logs_say_when_only_the_end_was_read() {
    let lines: String = (1..=100).map(|i| format!("web-1  | line {i}\n")).collect();
    let compose = Compose::new().answer("logs", &lines, "", 0);
    let text = stdout(&compose.bdo(&["logs"]));
    assert!(text.contains("web-1  | line 100"), "{text}");
    assert!(
        text.contains(
            "[only the last 100 lines of each service were read — use --tail N for more]"
        ),
        "{text}"
    );

    let compose = Compose::new().answer("logs", "web-1  | a\nweb-1  | b\n", "", 0);
    let text = stdout(&compose.bdo(&["logs", "--tail", "2"]));
    assert!(text.contains("the last 2 lines"), "{text}");
    assert!(
        compose.args().contains("logs --tail 2"),
        "{}",
        compose.args()
    );

    let compose = Compose::new().answer("logs", "", "", 0);
    assert_eq!(stdout(&compose.bdo(&["logs"])), "[compose] No logs\n");
}

#[test]
fn compose_commands_it_does_not_filter_pass_through() {
    let compose = Compose::new().answer("up", "started\n", "", 3);
    let out = compose.bdo(&["up", "-d"]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert_eq!(stdout(&out), "started\n");
    assert!(
        compose.args().contains("compose up -d"),
        "{}",
        compose.args()
    );
}
