//! Container lifecycle via the plain `docker compose` CLI:
//! up -> poll health -> resolve dynamic host ports -> (run) -> down -v.

use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use serde::Deserialize;

use crate::compose::GeneratedCompose;
use crate::docker;
use crate::spec::ServiceKind;

#[derive(Debug, Clone)]
pub struct ServiceEndpoint {
    pub service: String,
    pub kind: ServiceKind,
    pub host_port: u16,
    pub container_port: u16,
}

pub struct RunHandles {
    pub project_name: String,
    pub compose_file: String,
    pub endpoints: Vec<ServiceEndpoint>,
    /// Resolved host ports for aux containers (e.g. the Flink JobManager REST
    /// API), keyed by compose service name.
    pub aux_ports: std::collections::BTreeMap<String, u16>,
}

fn base(file: &str, project: &str) -> Vec<String> {
    vec![
        "compose".to_string(),
        "-f".to_string(),
        file.to_string(),
        "-p".to_string(),
        project.to_string(),
    ]
}

/// Fail early with a clear message if docker / compose isn't usable.
pub fn preflight() -> Result<()> {
    let v = docker::capture(&["compose".to_string(), "version".to_string()]);
    if v.code != 0 {
        bail!(
            "`docker compose` is not available. Ensure Docker is installed and running ({}).",
            v.stderr.trim()
        );
    }
    let info = docker::capture(&["info".to_string()]);
    if info.code != 0 {
        bail!("the Docker daemon is not reachable. Is Docker running?");
    }
    Ok(())
}

#[derive(Deserialize, Default)]
struct PsEntry {
    #[serde(default, rename = "Service")]
    service: String,
    #[serde(default, rename = "State")]
    state: String,
    #[serde(default, rename = "Health")]
    health: String,
}

fn parse_ps(stdout: &str) -> Vec<PsEntry> {
    let t = stdout.trim();
    if t.is_empty() {
        return Vec::new();
    }
    if t.starts_with('[') {
        return serde_json::from_str(t).unwrap_or_default();
    }
    let mut v = Vec::new();
    for line in t.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        if let Ok(e) = serde_json::from_str::<PsEntry>(l) {
            v.push(e);
        }
    }
    v
}

fn is_ready(e: &PsEntry, expect_healthcheck: bool) -> bool {
    let state = e.state.to_lowercase();
    if state == "exited" || state == "dead" {
        return false;
    }
    let health = e.health.to_lowercase();
    if expect_healthcheck {
        return health == "healthy";
    }
    if !health.is_empty() {
        return health == "healthy";
    }
    state == "running"
}

fn has_crashed(e: &PsEntry) -> bool {
    let state = e.state.to_lowercase();
    state == "exited" || state == "dead"
}

/// Bring the compose project up, wait for health, resolve host ports.
pub fn up(compose: &GeneratedCompose, file: &str, health_timeout: Duration) -> Result<RunHandles> {
    let project = compose.project_name.clone();

    let mut up_args = base(file, &project);
    up_args.extend([
        "up".to_string(),
        "-d".to_string(),
        "--remove-orphans".to_string(),
    ]);
    let code = docker::stream(&up_args);
    if code != 0 {
        let _ = down(&project, file);
        bail!("docker compose up failed (exit {code}). See output above.");
    }

    let mut want: Vec<(String, bool)> = compose
        .services
        .iter()
        .map(|s| (s.name.clone(), s.has_healthcheck))
        .collect();
    // Aux containers (Flink cluster) have no container healthcheck — wait for
    // "running"; REST readiness is gated separately, host-side, before submit.
    for a in &compose.aux {
        want.push((a.name.clone(), false));
    }

    let deadline = Instant::now() + health_timeout;
    loop {
        let mut ps_args = base(file, &project);
        ps_args.extend([
            "ps".to_string(),
            "--format".to_string(),
            "json".to_string(),
            "-a".to_string(),
        ]);
        let ps = docker::capture(&ps_args);
        let entries = parse_ps(&ps.stdout);

        let crashed: Vec<String> = want
            .iter()
            .filter(|(name, _)| {
                entries
                    .iter()
                    .find(|e| &e.service == name)
                    .map(has_crashed)
                    .unwrap_or(false)
            })
            .map(|(name, _)| name.clone())
            .collect();
        if !crashed.is_empty() {
            dump_logs(file, &project, &crashed);
            let _ = down(&project, file);
            bail!("service(s) exited before becoming healthy: {}", crashed.join(", "));
        }

        let all_ready = want.iter().all(|(name, hc)| {
            entries
                .iter()
                .find(|e| &e.service == name)
                .map(|e| is_ready(e, *hc))
                .unwrap_or(false)
        });
        if all_ready {
            break;
        }

        if Instant::now() > deadline {
            let snap: Vec<String> = entries
                .iter()
                .map(|e| format!("{}={}{}", e.service, e.state, if e.health.is_empty() { String::new() } else { format!("/{}", e.health) }))
                .collect();
            let _ = down(&project, file);
            bail!(
                "containers did not become healthy within {}s (last: {}).",
                health_timeout.as_secs(),
                snap.join(", ")
            );
        }
        sleep(Duration::from_millis(2000));
    }

    let mut endpoints = Vec::new();
    for s in &compose.services {
        // Pre-assigned (fixed) host port for services like Kafka; otherwise ask
        // Docker for the random mapping it chose.
        let host_port = match s.host_port {
            Some(p) => p,
            None => resolve_port(file, &project, &s.name, s.container_port)?,
        };
        endpoints.push(ServiceEndpoint {
            service: s.name.clone(),
            kind: s.kind,
            host_port,
            container_port: s.container_port,
        });
    }

    let mut aux_ports = std::collections::BTreeMap::new();
    for a in &compose.aux {
        if let Some(cp) = a.resolve_port {
            let hp = resolve_port(file, &project, &a.name, cp)?;
            aux_ports.insert(a.name.clone(), hp);
        }
    }

    Ok(RunHandles {
        project_name: project,
        compose_file: file.to_string(),
        endpoints,
        aux_ports,
    })
}

fn resolve_port(file: &str, project: &str, service: &str, container_port: u16) -> Result<u16> {
    let mut args = base(file, project);
    args.extend([
        "port".to_string(),
        service.to_string(),
        container_port.to_string(),
    ]);
    let r = docker::capture(&args);
    if r.code != 0 {
        bail!(
            "could not resolve host port for {service}:{container_port} ({})",
            r.stderr.trim()
        );
    }
    let line = r
        .stdout
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("");
    line.rsplit(':')
        .next()
        .and_then(|p| p.trim().parse::<u16>().ok())
        .ok_or_else(|| anyhow!("unexpected port output for {service}: \"{}\"", r.stdout.trim()))
}

/// Tear the project down and remove volumes. Idempotent; never errors out.
pub fn down(project: &str, file: &str) -> Result<()> {
    let mut args = base(file, project);
    args.extend([
        "down".to_string(),
        "-v".to_string(),
        "--remove-orphans".to_string(),
        "-t".to_string(),
        "10".to_string(),
    ]);
    docker::capture(&args);
    Ok(())
}

fn dump_logs(file: &str, project: &str, services: &[String]) {
    let mut args = base(file, project);
    args.extend(["logs".to_string(), "--tail".to_string(), "40".to_string()]);
    for s in services {
        args.push(s.clone());
    }
    let r = docker::capture(&args);
    let text = format!("{}{}", r.stdout, r.stderr);
    let text = text.trim();
    if !text.is_empty() {
        eprintln!("---- container logs (tail) ----\n{text}\n-------------------------------");
    }
}
