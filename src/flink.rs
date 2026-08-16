//! Apache Flink pipeline support.
//!
//! Two responsibilities live here, kept together because they share the same
//! networking contract:
//!
//! 1. **Cluster compose fragment** — [`cluster_services`] generates an ephemeral
//!    Flink *session cluster* (one `jobmanager` + one `taskmanager`) on the same
//!    isolated `noworries` network as the declared services, so a submitted job
//!    reaches Kafka/Postgres/Elastic by their in-network compose names
//!    (`kafka:9092`, `postgres:5432`, `elastic:9200`, …) — never the random host
//!    ports the host-side app model uses.
//! 2. **Job submission** — [`submit_all`] builds each job (optional), uploads the
//!    jar over the JobManager REST API, runs it, and polls until it reaches
//!    `RUNNING`, before the checks exercise the pipeline end to end.
//!
//! The cluster is destroyed by the run's `docker compose down -v` teardown, so
//! jobs need no explicit cancellation.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;

use crate::spec::{CheckSpec, FlinkJob, FlinkSpec, ServiceKind};

/// JobManager REST / web-UI container port.
pub const REST_PORT: u16 = 8081;
pub const JOBMANAGER: &str = "jobmanager";
pub const TASKMANAGER: &str = "taskmanager";

const DEFAULT_IMAGE: &str = "flink:1.19";
const DEFAULT_SLOTS: u32 = 2;
const DEFAULT_SUBMIT_TIMEOUT_SEC: u64 = 120;

/// A generated cluster container: its compose name and body, plus whether the
/// runner should resolve a host port mapping for it (only the JobManager REST).
pub struct ClusterContainer {
    pub name: String,
    pub body: serde_yaml::Value,
    pub resolve_port: Option<u16>,
}

/// In-network coordinates (compose service name + container port) a job uses to
/// reach a declared service. Deliberately a fixed per-kind table rather than the
/// service's `container_port`, because Kafka's `container_port` is its *external*
/// listener (9093) while in-network clients must use the internal one (9092).
pub fn in_network_coords(kind: ServiceKind) -> (&'static str, u16) {
    match kind {
        ServiceKind::Postgres => ("postgres", 5432),
        ServiceKind::Mysql => ("mysql", 3306),
        ServiceKind::Mongodb => ("mongodb", 27017),
        ServiceKind::Redis => ("redis", 6379),
        ServiceKind::Elastic => ("elastic", 9200),
        ServiceKind::Kafka => ("kafka", 9092),
        ServiceKind::Cockroach => ("cockroach", 26257),
        ServiceKind::Opensearch => ("opensearch", 9200),
        ServiceKind::Mssql => ("mssql", 1433),
        ServiceKind::Rabbitmq => ("rabbitmq", 5672),
        ServiceKind::Clickhouse => ("clickhouse", 8123),
        ServiceKind::Cassandra => ("cassandra", 9042),
    }
}

/// Topic names the pipeline likely sources/sinks: those referenced in checks'
/// `kafka.produce` / `kafka.expect_message`, plus any listed in `flink.topics`.
/// Deduplicated and sorted for stable output.
pub fn referenced_topics(fs: &FlinkSpec, checks: &[CheckSpec]) -> Vec<String> {
    let mut set: BTreeSet<String> = fs.topics.iter().cloned().collect();
    for c in checks {
        if let Some(k) = &c.kafka {
            if let Some(p) = &k.produce {
                set.insert(p.topic.clone());
            }
            if let Some(e) = &k.expect_message {
                set.insert(e.topic.clone());
            }
        }
    }
    set.into_iter().collect()
}

/// Pre-create Kafka topics (idempotently) before a job's `KafkaSource` starts.
///
/// Flink's `KafkaSource` resolves partitions via `AdminClient.describeTopics`,
/// which does **not** trigger broker auto-create (even with
/// `KAFKA_AUTO_CREATE_TOPICS_ENABLE=true`), so a missing source topic hard-fails
/// the job before it can run. We create them with the image's `kafka-topics.sh`;
/// one partition / one replica is enough for the single-broker harness. Returns
/// the topics successfully ensured (best-effort: a failure is left for the job
/// to surface with its own clearer error).
pub fn ensure_kafka_topics(compose_file: &str, project: &str, topics: &[String]) -> Vec<String> {
    let mut ensured = Vec::new();
    for t in topics {
        let out = crate::lifecycle::compose_exec(
            compose_file,
            project,
            "kafka",
            &[
                "/opt/kafka/bin/kafka-topics.sh",
                "--create",
                "--if-not-exists",
                "--topic",
                t,
                "--bootstrap-server",
                "localhost:9092",
                "--partitions",
                "1",
                "--replication-factor",
                "1",
            ],
        );
        if out.code == 0 {
            ensured.push(t.clone());
        }
    }
    ensured
}

#[derive(Serialize)]
struct JobManagerBody {
    image: String,
    command: String,
    ports: Vec<String>,
    networks: Vec<String>,
    environment: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct TaskManagerBody {
    image: String,
    command: String,
    depends_on: Vec<String>,
    networks: Vec<String>,
    environment: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deploy: Option<Deploy>,
}

#[derive(Serialize)]
struct Deploy {
    replicas: u32,
}

/// Build the jobmanager + taskmanager compose fragments. `service_kinds` are the
/// declared services, wired into the cluster env as `NOWORRIES_<KIND>_HOST/PORT`
/// pointing at in-network coordinates so env-driven jobs connect correctly.
pub fn cluster_services(
    flink: &FlinkSpec,
    service_kinds: &[ServiceKind],
) -> Result<Vec<ClusterContainer>> {
    let image = flink.image.clone().unwrap_or_else(|| DEFAULT_IMAGE.to_string());
    let slots = flink.slots.unwrap_or(DEFAULT_SLOTS);
    let taskmanagers = flink.taskmanagers.unwrap_or(1).max(1);

    // Vars every cluster container receives: in-network service coordinates.
    let mut svc_env = BTreeMap::new();
    for kind in service_kinds {
        let (host, port) = in_network_coords(*kind);
        let k = kind.as_str().to_uppercase();
        svc_env.insert(format!("NOWORRIES_{k}_HOST"), host.to_string());
        svc_env.insert(format!("NOWORRIES_{k}_PORT"), port.to_string());
    }

    let jm_props = format!("jobmanager.rpc.address: {JOBMANAGER}\n");
    let tm_props = format!(
        "jobmanager.rpc.address: {JOBMANAGER}\ntaskmanager.numberOfTaskSlots: {slots}\n"
    );

    let mut jm_env = svc_env.clone();
    jm_env.insert("FLINK_PROPERTIES".to_string(), jm_props);
    let jm = JobManagerBody {
        image: image.clone(),
        command: "jobmanager".to_string(),
        // Bare container port => Docker assigns a random host port we resolve.
        ports: vec![REST_PORT.to_string()],
        networks: vec!["noworries".to_string()],
        environment: jm_env,
    };

    let mut tm_env = svc_env;
    tm_env.insert("FLINK_PROPERTIES".to_string(), tm_props);
    let tm = TaskManagerBody {
        image,
        command: "taskmanager".to_string(),
        depends_on: vec![JOBMANAGER.to_string()],
        networks: vec!["noworries".to_string()],
        environment: tm_env,
        deploy: (taskmanagers > 1).then_some(Deploy { replicas: taskmanagers }),
    };

    Ok(vec![
        ClusterContainer {
            name: JOBMANAGER.to_string(),
            body: serde_yaml::to_value(jm).map_err(|e| anyhow!(e))?,
            resolve_port: Some(REST_PORT),
        },
        ClusterContainer {
            name: TASKMANAGER.to_string(),
            body: serde_yaml::to_value(tm).map_err(|e| anyhow!(e))?,
            resolve_port: None,
        },
    ])
}

/// Outcome of submitting one job (for reporting).
pub struct SubmittedJob {
    pub name: String,
    pub job_id: String,
}

/// Build (optional), upload, run, and wait-for-RUNNING each job in order.
/// `rest_host_port` is the resolved host port mapped to the JobManager REST API.
pub fn submit_all(
    dir: &Path,
    rest_host_port: u16,
    flink: &FlinkSpec,
) -> Result<Vec<SubmittedJob>> {
    let base = format!("http://127.0.0.1:{rest_host_port}");
    let timeout = Duration::from_secs(flink.submit_timeout.unwrap_or(DEFAULT_SUBMIT_TIMEOUT_SEC));

    wait_rest_ready(&base, timeout)
        .context("Flink JobManager REST API did not become ready")?;

    let mut out = Vec::new();
    for (i, job) in flink.jobs.iter().enumerate() {
        let label = job
            .name
            .clone()
            .unwrap_or_else(|| format!("job[{i}] {}", job.jar));

        if let Some(build) = &job.build {
            if !build.trim().is_empty() {
                run_build(dir, build).with_context(|| format!("building {label}"))?;
            }
        }

        let jar_path = dir.join(&job.jar);
        if !jar_path.exists() {
            bail!(
                "job jar not found: {} (set `flink.jobs[].build` to produce it, or fix the path)",
                jar_path.display()
            );
        }

        let jar_id = upload_jar(&base, &jar_path)
            .with_context(|| format!("uploading jar for {label}"))?;
        let job_id = run_jar(&base, &jar_id, job)
            .with_context(|| format!("submitting {label}"))?;
        wait_running(&base, &job_id, timeout)
            .with_context(|| format!("waiting for {label} to reach RUNNING"))?;

        out.push(SubmittedJob { name: label, job_id });
    }
    Ok(out)
}

fn run_build(dir: &Path, build: &str) -> Result<()> {
    let status = crate::app::shell_command(build)
        .current_dir(dir)
        .status()
        .with_context(|| format!("running build command: {build}"))?;
    if !status.success() {
        bail!("build command failed (`{build}`): {status}");
    }
    Ok(())
}

fn agent() -> ureq::Agent {
    ureq::builder().timeout(Duration::from_secs(30)).build()
}

/// Read a response body and parse it as JSON. (ureq's `into_json` needs the
/// `json` feature, which we disable to avoid pulling extra deps.)
fn read_json(resp: ureq::Response) -> Result<serde_json::Value> {
    let body = resp.into_string().context("reading response body")?;
    serde_json::from_str(&body).with_context(|| format!("response was not JSON: {body}"))
}

/// Poll `GET /config` until the REST API answers.
fn wait_rest_ready(base: &str, timeout: Duration) -> Result<()> {
    let url = format!("{base}/config");
    let deadline = Instant::now() + timeout;
    loop {
        match agent().get(&url).call() {
            Ok(_) => return Ok(()),
            Err(ureq::Error::Status(_, _)) => return Ok(()), // reachable, just non-2xx
            Err(_) if Instant::now() < deadline => sleep(Duration::from_millis(1000)),
            Err(e) => bail!("REST API unreachable at {base}: {e}"),
        }
    }
}

/// `POST /jars/upload` as multipart/form-data; returns the uploaded jar id.
fn upload_jar(base: &str, jar_path: &Path) -> Result<String> {
    let bytes = std::fs::read(jar_path)
        .with_context(|| format!("reading {}", jar_path.display()))?;
    let filename = jar_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "job.jar".to_string());

    let boundary = "noworriesX0aBcD1eFgH2iJkL3mNoPqR";
    let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 256);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"jarfile\"; filename=\"{filename}\"\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/x-java-archive\r\n\r\n");
    body.extend_from_slice(&bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let url = format!("{base}/jars/upload");
    let resp = agent()
        .post(&url)
        .set("content-type", &format!("multipart/form-data; boundary={boundary}"))
        .send_bytes(&body)
        .map_err(describe_ureq)?;
    let json = read_json(resp).context("parsing upload response")?;

    // Response: {"filename":"/tmp/.../<id>_job.jar","status":"success"}.
    // The id used by /jars/:id/run is the last path segment of `filename`.
    let filename = json
        .get("filename")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("upload response had no \"filename\": {json}"))?;
    let id = filename.rsplit('/').next().unwrap_or(filename);
    if id.is_empty() {
        bail!("could not derive jar id from upload filename \"{filename}\"");
    }
    Ok(id.to_string())
}

/// `POST /jars/:id/run`; returns the new job id.
fn run_jar(base: &str, jar_id: &str, job: &FlinkJob) -> Result<String> {
    let url = format!("{base}/jars/{jar_id}/run");
    let mut req = agent().post(&url);
    if let Some(cls) = &job.entry_class {
        req = req.query("entry-class", cls);
    }
    if let Some(p) = job.parallelism {
        req = req.query("parallelism", &p.to_string());
    }
    for arg in &job.args {
        req = req.query("programArg", arg);
    }
    let resp = req.call().map_err(describe_ureq)?;
    let json = read_json(resp).context("parsing run response")?;
    let job_id = json
        .get("jobid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("run response had no \"jobid\": {json}"))?;
    Ok(job_id.to_string())
}

/// Poll `GET /jobs/:id` until the job is RUNNING (success) or a terminal failure.
fn wait_running(base: &str, job_id: &str, timeout: Duration) -> Result<()> {
    let url = format!("{base}/jobs/{job_id}");
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    loop {
        match agent().get(&url).call() {
            Ok(resp) => {
                let json = read_json(resp).context("parsing job status")?;
                let state = json.get("state").and_then(|v| v.as_str()).unwrap_or("");
                last = state.to_string();
                match state {
                    // Streaming pipelines run; a batch job may finish quickly.
                    "RUNNING" | "FINISHED" => return Ok(()),
                    "FAILED" | "FAILING" | "CANCELED" | "CANCELLING" | "SUSPENDED" => {
                        bail!("job entered terminal state {state} before running");
                    }
                    // CREATED / SCHEDULED / RECONCILING / RESTARTING / INITIALIZING: keep waiting.
                    _ => {}
                }
            }
            Err(ureq::Error::Status(404, _)) => {
                // Job not visible yet right after submit — keep polling.
            }
            Err(e) if Instant::now() >= deadline => {
                bail!("could not query job status: {}", describe_ureq(e));
            }
            Err(_) => {}
        }
        if Instant::now() > deadline {
            bail!(
                "job did not reach RUNNING within {}s (last state: {}). \
                 Check the JobManager UI / `.noworries/app.log`.",
                timeout.as_secs(),
                if last.is_empty() { "unknown" } else { &last }
            );
        }
        sleep(Duration::from_millis(1500));
    }
}

/// Turn a ureq error into a message that includes the server's response body.
fn describe_ureq(e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            anyhow!("HTTP {code}: {}", body.trim())
        }
        ureq::Error::Transport(t) => anyhow!("transport error: {t}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(jobs: Vec<FlinkJob>) -> FlinkSpec {
        FlinkSpec {
            image: None,
            taskmanagers: None,
            slots: None,
            submit_timeout: None,
            topics: vec![],
            jobs,
        }
    }

    #[test]
    fn referenced_topics_dedupes_explicit_and_check_topics() {
        use crate::spec::NoworriesSpec;
        let yaml = r#"
version: 1
services: [kafka, postgres]
flink:
  topics: [manual-topic]
  jobs:
    - jar: target/p.jar
checks:
  - name: "produce and expect"
    kafka: { produce: { topic: "events-in", message: { id: "E1" } } }
  - name: "downstream"
    kafka: { expect_message: { topic: "events-out", contains: { id: "E1" } } }
  - name: "dup source"
    kafka: { produce: { topic: "events-in", message: { id: "E2" } } }
"#;
        let s = NoworriesSpec::parse(yaml).unwrap();
        let topics = referenced_topics(s.flink.as_ref().unwrap(), &s.checks);
        // sorted + deduped: events-in once, events-out, manual-topic.
        assert_eq!(topics, vec!["events-in", "events-out", "manual-topic"]);
    }

    fn job(jar: &str) -> FlinkJob {
        FlinkJob {
            name: None,
            build: None,
            jar: jar.to_string(),
            entry_class: None,
            args: vec![],
            parallelism: None,
        }
    }

    #[test]
    fn kafka_in_network_uses_internal_port_not_external() {
        // The trap: Kafka's ComposeService.container_port is 9093 (external);
        // in-network the job must use 9092.
        assert_eq!(in_network_coords(ServiceKind::Kafka), ("kafka", 9092));
        assert_eq!(in_network_coords(ServiceKind::Elastic), ("elastic", 9200));
        assert_eq!(in_network_coords(ServiceKind::Postgres), ("postgres", 5432));
    }

    #[test]
    fn cluster_has_jobmanager_and_taskmanager() {
        let containers =
            cluster_services(&spec(vec![job("target/j.jar")]), &[ServiceKind::Kafka]).unwrap();
        let names: Vec<_> = containers.iter().map(|c| c.name.clone()).collect();
        assert_eq!(names, vec![JOBMANAGER.to_string(), TASKMANAGER.to_string()]);

        // Only the jobmanager REST port is resolved.
        let jm = &containers[0];
        assert_eq!(jm.resolve_port, Some(REST_PORT));
        assert_eq!(containers[1].resolve_port, None);
    }

    #[test]
    fn jobmanager_wires_service_coords_and_rpc_address() {
        let containers =
            cluster_services(&spec(vec![job("target/j.jar")]), &[ServiceKind::Kafka, ServiceKind::Postgres])
                .unwrap();
        let jm = serde_yaml::to_string(&containers[0].body).unwrap();
        assert!(jm.contains("command: jobmanager"));
        assert!(jm.contains("NOWORRIES_KAFKA_HOST: kafka"));
        assert!(jm.contains("NOWORRIES_KAFKA_PORT: '9092'") || jm.contains("NOWORRIES_KAFKA_PORT: \"9092\""));
        assert!(jm.contains("NOWORRIES_POSTGRES_PORT"));
        assert!(jm.contains("jobmanager.rpc.address: jobmanager"));

        let tm = serde_yaml::to_string(&containers[1].body).unwrap();
        assert!(tm.contains("command: taskmanager"));
        assert!(tm.contains("taskmanager.numberOfTaskSlots: 2"));
    }

    #[test]
    fn multiple_taskmanagers_set_replicas() {
        let mut s = spec(vec![job("target/j.jar")]);
        s.taskmanagers = Some(3);
        let containers = cluster_services(&s, &[]).unwrap();
        let tm = serde_yaml::to_string(&containers[1].body).unwrap();
        assert!(tm.contains("replicas: 3"));
    }

    #[test]
    fn single_taskmanager_omits_deploy() {
        let containers = cluster_services(&spec(vec![job("target/j.jar")]), &[]).unwrap();
        let tm = serde_yaml::to_string(&containers[1].body).unwrap();
        assert!(!tm.contains("deploy"));
    }
}
