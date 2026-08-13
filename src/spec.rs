//! Types and parsing for the human/AI-editable `noworries.yml` spec file.
//!
//! The spec is intentionally framework-agnostic: it declares *what* infra a
//! feature needs and *what* "correct" means, never how a specific framework is
//! wired. Framework specifics live behind the `Framework` trait.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

pub const SPEC_FILENAME: &str = "noworries.yml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    Postgres,
    Kafka,
    Redis,
    Elastic,
    Mysql,
    Mongodb,
}

impl ServiceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceKind::Postgres => "postgres",
            ServiceKind::Kafka => "kafka",
            ServiceKind::Redis => "redis",
            ServiceKind::Elastic => "elastic",
            ServiceKind::Mysql => "mysql",
            ServiceKind::Mongodb => "mongodb",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        match token.to_ascii_lowercase().as_str() {
            "postgres" => Some(ServiceKind::Postgres),
            "kafka" => Some(ServiceKind::Kafka),
            "redis" => Some(ServiceKind::Redis),
            "elastic" | "elasticsearch" => Some(ServiceKind::Elastic),
            "mysql" => Some(ServiceKind::Mysql),
            "mongodb" | "mongo" => Some(ServiceKind::Mongodb),
            _ => None,
        }
    }
}

/// A declared service such as `postgres:16-alpine`.
#[derive(Debug, Clone)]
pub struct ServiceDecl {
    pub kind: ServiceKind,
    pub image: String,
    pub tag: Option<String>,
    pub raw: String,
}

/// The Docker Hub repository each kind maps to. Note Kafka's official image is
/// `apache/kafka`, not `kafka`, so `kafka:3.7` must expand to `apache/kafka:3.7`.
pub fn default_repo(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Postgres => "postgres",
        ServiceKind::Kafka => "apache/kafka",
        ServiceKind::Redis => "redis",
        ServiceKind::Elastic => "docker.elastic.co/elasticsearch/elasticsearch",
        ServiceKind::Mysql => "mysql",
        ServiceKind::Mongodb => "mongo",
    }
}

/// Default images used when the spec gives a bare kind with no tag.
pub fn default_image(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Postgres => "postgres:16-alpine",
        ServiceKind::Kafka => "apache/kafka:3.7.0",
        ServiceKind::Redis => "redis:7-alpine",
        ServiceKind::Elastic => "docker.elastic.co/elasticsearch/elasticsearch:8.13.4",
        ServiceKind::Mysql => "mysql:8.4",
        ServiceKind::Mongodb => "mongo:7",
    }
}

fn parse_service_decl(raw: &str) -> Result<ServiceDecl> {
    let value = raw.trim();
    if value.is_empty() {
        bail!("services entries must be non-empty strings like \"postgres:16-alpine\"");
    }
    let mut parts = value.splitn(2, ':');
    let kind_token = parts.next().unwrap_or("");
    let kind = ServiceKind::from_token(kind_token)
        .ok_or_else(|| anyhow!("unsupported service \"{value}\". supported: postgres, kafka, redis"))?;
    let tag = parts.next().map(|s| s.to_string());
    // Expand "<kind>:<tag>" to the kind's real repository, e.g.
    // "kafka:3.7" -> "apache/kafka:3.7"; bare "<kind>" -> the default image.
    let image = match &tag {
        Some(t) => format!("{}:{}", default_repo(kind), t),
        None => default_image(kind).to_string(),
    };
    Ok(ServiceDecl { kind, image, tag, raw: value.to_string() })
}

/// How to launch the app under test. All optional: `start` is auto-detected
/// from the resolved framework when omitted; `framework` forces a specific
/// framework instead of auto-detecting.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSpec {
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub health: Option<String>,
    #[serde(default)]
    pub framework: Option<String>,
    #[serde(default, rename = "port_env")]
    pub port_env: Option<String>,
    #[serde(default, rename = "ready_timeout")]
    pub ready_timeout: Option<u64>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// A Flink pipeline under test: an ephemeral session cluster
/// (jobmanager + taskmanager) onto which one or more jobs are built and
/// submitted over the REST API before the checks run. Use this **instead of**
/// `app` when the thing under test is a Flink job rather than an HTTP server.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlinkSpec {
    /// Flink image (default `flink:1.19`).
    #[serde(default)]
    pub image: Option<String>,
    /// Number of TaskManager replicas (default 1).
    #[serde(default)]
    pub taskmanagers: Option<u32>,
    /// Task slots per TaskManager (default 2).
    #[serde(default)]
    pub slots: Option<u32>,
    /// Seconds to wait for the JobManager REST API and for each job to reach
    /// RUNNING (default 120).
    #[serde(default, rename = "submit_timeout")]
    pub submit_timeout: Option<u64>,
    /// Jobs to build + submit, in order.
    pub jobs: Vec<FlinkJob>,
}

/// One Flink job: optionally built, then uploaded and run on the cluster.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlinkJob {
    /// Optional label for reporting.
    #[serde(default)]
    pub name: Option<String>,
    /// Optional shell command run (in the project dir) to build the jar before
    /// submission, e.g. `mvn -q -DskipTests package`.
    #[serde(default)]
    pub build: Option<String>,
    /// Path to the job jar (relative to the project dir), e.g.
    /// `target/pipeline-0.1.jar`.
    pub jar: String,
    /// Optional fully-qualified entry-point class (`--class`).
    #[serde(default, rename = "entry_class")]
    pub entry_class: Option<String>,
    /// Optional program arguments passed to the job.
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional job parallelism.
    #[serde(default)]
    pub parallelism: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpRequestSpec {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpExpectSpec {
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub body_contains: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DbAssertion {
    pub query: String,
    #[serde(default)]
    pub expect_row: Option<serde_yaml::Value>,
    #[serde(default)]
    pub expect_row_count: Option<i64>,
}

/// Kafka assertion. Supports producing a message to a topic (to exercise a
/// consumer the feature added) and/or expecting a message on a topic (to
/// verify a producer the feature added).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KafkaAssertion {
    #[serde(default)]
    pub produce: Option<KafkaProduce>,
    #[serde(default)]
    pub expect_message: Option<KafkaExpect>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KafkaProduce {
    pub topic: String,
    pub message: serde_yaml::Value,
    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KafkaExpect {
    pub topic: String,
    pub contains: serde_yaml::Value,
    #[serde(default, rename = "timeout_ms")]
    pub timeout_ms: Option<u64>,
}

/// A non-happy-path load/timing scenario for the **trigger** phase of a check.
/// Instead of a single produce, it generates many messages in a shape designed
/// to stress a real behaviour — burst throughput, concurrent races, duplicate
/// delivery, out-of-order arrival — so data pipelines (Flink and friends) get
/// tested for correctness under load, not just the happy path. Verification uses
/// the check's ordinary observe assertions (`db`/`mysql`/`mongodb`/`elastic`
/// `expect_*count*`, etc.). Extensible: `kind` resolves to an edge-case strategy,
/// and new kinds are added without touching this struct.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioSpec {
    /// `burst` | `concurrent` | `duplicates` | `out_of_order`.
    pub kind: String,
    /// Kafka load target (the only sink for now; HTTP/other come later).
    pub kafka: ScenarioKafka,
    /// Total number of distinct logical messages (default per-kind, e.g. 100).
    #[serde(default)]
    pub count: Option<u32>,
    /// Parallel producer threads — the source of real races (default per-kind).
    #[serde(default)]
    pub concurrency: Option<u32>,
    /// Number of distinct keys the load spreads over (default per-kind). Fewer
    /// keys + higher concurrency = more contention on the same key.
    #[serde(default)]
    pub keys: Option<u32>,
    /// Optional cap on messages/second per producer (0/absent = as fast as able).
    #[serde(default, rename = "rate_per_sec")]
    pub rate_per_sec: Option<u32>,
}

/// The Kafka target + message template for a [`ScenarioSpec`]. The `key` and
/// `message` templates may reference `${seq}` (global 0-based index), `${i}`
/// (per-key sequence), `${key}` (assigned key value), and `${uuid}` (unique id).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioKafka {
    pub topic: String,
    #[serde(default)]
    pub key: Option<String>,
    pub message: serde_yaml::Value,
}

/// Redis assertion: check a key exists and/or its (possibly JSON) value matches.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedisAssertion {
    pub key: String,
    #[serde(default)]
    pub expect_exists: Option<bool>,
    #[serde(default)]
    pub expect_value: Option<serde_yaml::Value>,
    #[serde(default)]
    pub expect_value_contains: Option<serde_yaml::Value>,
}

/// An index template applied to Elasticsearch before the app runs (so
/// app-created indices pick up the right mapping). The `body` is generated by
/// Claude from the code, or pasted from production. `legacy` uses the old
/// `_template` API (ES < 7.8); otherwise `_index_template` (ES 7.8+ and 8).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElasticTemplate {
    pub name: String,
    pub body: serde_yaml::Value,
    #[serde(default)]
    pub legacy: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElasticInsert {
    #[serde(default)]
    pub id: Option<String>,
    pub document: serde_yaml::Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElasticUpdate {
    pub id: String,
    pub document: serde_yaml::Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElasticDelete {
    pub id: String,
}

/// A noworries-run Elasticsearch operation (setup or trigger). Exactly one of
/// `insert` / `update` / `delete` is set per list entry, e.g.
/// `- insert: { id: "X1", document: { ... } }`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElasticOp {
    #[serde(default)]
    pub insert: Option<ElasticInsert>,
    #[serde(default)]
    pub update: Option<ElasticUpdate>,
    #[serde(default)]
    pub delete: Option<ElasticDelete>,
}

/// Elasticsearch assertion. Supports applying a template, running
/// insert/update/delete operations, and verifying a document or a search.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElasticAssertion {
    pub index: String,
    #[serde(default)]
    pub template: Option<ElasticTemplate>,
    /// noworries-run operations, executed in order before verification.
    #[serde(default)]
    pub operations: Vec<ElasticOp>,
    /// Verify a specific document by id.
    #[serde(default)]
    pub doc_id: Option<String>,
    #[serde(default)]
    pub expect_exists: Option<bool>,
    #[serde(default)]
    pub expect_source_contains: Option<serde_yaml::Value>,
    /// Verify via a search: the Elasticsearch query DSL body (the `query` value).
    #[serde(default)]
    pub query: Option<serde_yaml::Value>,
    #[serde(default)]
    pub expect_hits: Option<i64>,
}

/// Authentication applied to every check's HTTP request. Extensible: each auth
/// style is its own optional sub-block, so adding a new one (mTLS, OAuth2, ...)
/// is a new field + a new arm in the resolver — nothing else changes. Set the
/// one(s) you need; string values support `${ENV_VAR}` interpolation so secrets
/// stay out of the repo.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AuthSpec {
    /// Log in to an endpoint, extract a token, and send it as a header.
    #[serde(default)]
    pub login: Option<AuthLogin>,
    /// A static bearer token (e.g. `${API_TOKEN}`).
    #[serde(default)]
    pub bearer: Option<AuthBearer>,
    /// HTTP Basic auth (username/password -> `Authorization: Basic ...`).
    #[serde(default)]
    pub basic: Option<AuthBasic>,
    /// API key sent as a header and/or a query parameter.
    #[serde(default)]
    pub api_key: Option<AuthApiKey>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthLogin {
    pub request: HttpRequestSpec,
    #[serde(default)]
    pub expect: Option<HttpExpectSpec>,
    /// JSON path to the token in the login response, e.g. `$.accessToken`.
    pub token_from: String,
    /// Header to place the token in (default `Authorization`).
    #[serde(default)]
    pub header: Option<String>,
    /// Scheme prefix (default `Bearer`; empty string = raw token).
    #[serde(default)]
    pub scheme: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthBearer {
    pub token: String,
    #[serde(default)]
    pub header: Option<String>,
    #[serde(default)]
    pub scheme: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthBasic {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthApiKey {
    /// Send the key in this header (e.g. `X-API-Key`). Defaults to `X-API-Key`
    /// if neither `header` nor `query` is set.
    #[serde(default)]
    pub header: Option<String>,
    /// Send the key as this query parameter instead of / in addition to a header.
    #[serde(default)]
    pub query: Option<String>,
    pub value: String,
}

/// MySQL assertion. `seed` statements run BEFORE the request (to set up data);
/// `query` + `expect_row`/`expect_row_count` verify state afterwards. This is
/// the "seed data -> hit the API -> check what changed" flow.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MySqlAssertion {
    /// Raw SQL statements executed before the request, in order (INSERT/UPDATE/
    /// DELETE/DDL) to seed initial data.
    #[serde(default)]
    pub seed: Vec<String>,
    /// A SELECT used for verification.
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub expect_row: Option<serde_yaml::Value>,
    #[serde(default)]
    pub expect_row_count: Option<i64>,
}

/// A noworries-run MongoDB operation (seed/trigger). Exactly one of
/// `insert` / `update` / `delete` per entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MongoOp {
    #[serde(default)]
    pub insert: Option<serde_yaml::Value>,
    #[serde(default)]
    pub update: Option<MongoUpdate>,
    #[serde(default)]
    pub delete: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MongoUpdate {
    pub filter: serde_yaml::Value,
    /// Fields to `$set` on the matched documents.
    pub set: serde_yaml::Value,
}

/// MongoDB assertion. `seed` operations run before the request; `find` +
/// `expect_doc_contains` / `expect_count` verify afterwards.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MongoAssertion {
    pub database: String,
    pub collection: String,
    #[serde(default)]
    pub seed: Vec<MongoOp>,
    /// Filter (Mongo query document) used for verification.
    #[serde(default)]
    pub find: Option<serde_yaml::Value>,
    #[serde(default)]
    pub expect_doc_contains: Option<serde_yaml::Value>,
    #[serde(default)]
    pub expect_count: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckSpec {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub request: Option<HttpRequestSpec>,
    #[serde(default)]
    pub expect: Option<HttpExpectSpec>,
    #[serde(default)]
    pub db: Option<DbAssertion>,
    #[serde(default)]
    pub kafka: Option<KafkaAssertion>,
    #[serde(default)]
    pub redis: Option<RedisAssertion>,
    #[serde(default)]
    pub elastic: Option<ElasticAssertion>,
    #[serde(default)]
    pub mysql: Option<MySqlAssertion>,
    #[serde(default)]
    pub mongodb: Option<MongoAssertion>,
    /// Optional edge-case load/timing scenario driving the trigger phase.
    #[serde(default)]
    pub scenario: Option<ScenarioSpec>,
}

/// Raw shape as it appears on disk (services are strings here).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpec {
    version: u32,
    services: Vec<String>,
    #[serde(default)]
    app: Option<AppSpec>,
    #[serde(default)]
    flink: Option<FlinkSpec>,
    #[serde(default)]
    auth: Option<AuthSpec>,
    #[serde(default)]
    checks: Vec<CheckSpec>,
}

/// Fully parsed + validated spec.
#[derive(Debug, Clone)]
pub struct NoworriesSpec {
    pub version: u32,
    pub services: Vec<ServiceDecl>,
    pub app: Option<AppSpec>,
    pub flink: Option<FlinkSpec>,
    pub auth: Option<AuthSpec>,
    pub checks: Vec<CheckSpec>,
}

impl NoworriesSpec {
    pub fn parse(source: &str) -> Result<Self> {
        let raw: RawSpec = serde_yaml::from_str(source)
            .with_context(|| format!("could not parse {SPEC_FILENAME}"))?;
        if raw.version != 1 {
            bail!("{SPEC_FILENAME}: unsupported version (expected 1, got {})", raw.version);
        }
        if raw.services.is_empty() {
            bail!("{SPEC_FILENAME}: \"services\" must be a non-empty list.");
        }
        let services = raw
            .services
            .iter()
            .map(|s| parse_service_decl(s))
            .collect::<Result<Vec<_>>>()?;
        for c in &raw.checks {
            if c.name.trim().is_empty() {
                bail!("{SPEC_FILENAME}: every check needs a non-empty \"name\".");
            }
        }
        if let Some(f) = &raw.flink {
            if f.jobs.is_empty() {
                bail!("{SPEC_FILENAME}: \"flink.jobs\" must list at least one job.");
            }
            for j in &f.jobs {
                if j.jar.trim().is_empty() {
                    bail!("{SPEC_FILENAME}: every flink job needs a non-empty \"jar\" path.");
                }
            }
        }
        Ok(NoworriesSpec {
            version: 1,
            services,
            app: raw.app,
            flink: raw.flink,
            auth: raw.auth,
            checks: raw.checks,
        })
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            bail!(
                "no {SPEC_FILENAME} found at {}. Run `noworries init` to scaffold one.",
                path.display()
            );
        }
        let source = fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&source)
    }

    #[cfg(test)]
    pub(crate) fn services_for_test(&self) -> &[ServiceDecl] {
        &self.services
    }

    /// Checks matching any of `tags`; all checks when `tags` is empty.
    pub fn select_checks(&self, tags: &[String]) -> Vec<CheckSpec> {
        if tags.is_empty() {
            return self.checks.clone();
        }
        self.checks
            .iter()
            .filter(|c| c.tags.iter().any(|t| tags.contains(t)))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_services(yaml: &str) -> Vec<ServiceDecl> {
        NoworriesSpec::parse(yaml).unwrap().services_for_test().to_vec()
    }

    #[test]
    fn kafka_shorthand_expands_to_apache_kafka() {
        let s = parse_services("version: 1\nservices: [kafka:3.7]\n");
        assert_eq!(s[0].kind, ServiceKind::Kafka);
        assert_eq!(s[0].image, "apache/kafka:3.7");
    }

    #[test]
    fn bare_kind_uses_default_image() {
        let s = parse_services("version: 1\nservices: [postgres, kafka, redis]\n");
        assert_eq!(s[0].image, "postgres:16-alpine");
        assert_eq!(s[1].image, "apache/kafka:3.7.0");
        assert_eq!(s[2].image, "redis:7-alpine");
    }

    #[test]
    fn unknown_service_is_rejected() {
        assert!(NoworriesSpec::parse("version: 1\nservices: [mariadb:11]\n").is_err());
    }

    #[test]
    fn mysql_image_expansion() {
        let s = parse_services("version: 1\nservices: [mysql:8.4, mysql]\n");
        assert_eq!(s[0].kind, ServiceKind::Mysql);
        assert_eq!(s[0].image, "mysql:8.4");
        assert_eq!(s[1].image, "mysql:8.4");
    }

    #[test]
    fn mongo_tokens_and_image_expansion() {
        let s = parse_services("version: 1\nservices: [mongodb:7, mongo]\n");
        assert_eq!(s[0].kind, ServiceKind::Mongodb);
        assert_eq!(s[0].image, "mongo:7");
        assert_eq!(s[1].kind, ServiceKind::Mongodb);
        assert_eq!(s[1].image, "mongo:7");
    }

    #[test]
    fn elastic_tokens_and_image_expansion() {
        let s = parse_services("version: 1\nservices: [elastic:8.13.4, elasticsearch]\n");
        assert_eq!(s[0].kind, ServiceKind::Elastic);
        assert_eq!(s[0].image, "docker.elastic.co/elasticsearch/elasticsearch:8.13.4");
        assert_eq!(s[1].kind, ServiceKind::Elastic);
        assert_eq!(s[1].image, "docker.elastic.co/elasticsearch/elasticsearch:8.13.4");
    }

    #[test]
    fn elastic_check_parses_template_ops_and_verification() {
        let yaml = r#"
version: 1
services: [elasticsearch:8.13.4]
checks:
  - name: "orders indexed"
    elastic:
      index: "orders"
      template:
        name: "orders-tmpl"
        body: { index_patterns: ["orders*"], template: { mappings: { properties: { sku: { type: "keyword" } } } } }
      operations:
        - insert: { id: "X1", document: { sku: "X1", status: "PENDING" } }
        - update: { id: "X1", document: { status: "SHIPPED" } }
        - delete: { id: "X2" }
      doc_id: "X1"
      expect_exists: true
      expect_source_contains: { status: "SHIPPED" }
      query: { match: { sku: "X1" } }
      expect_hits: 1
"#;
        let spec = NoworriesSpec::parse(yaml).unwrap();
        let e = spec.checks[0].elastic.as_ref().unwrap();
        assert_eq!(e.index, "orders");
        assert_eq!(e.template.as_ref().unwrap().name, "orders-tmpl");
        assert_eq!(e.operations.len(), 3);
        assert_eq!(e.expect_hits, Some(1));
        assert!(e.operations[0].insert.is_some());
        assert!(e.operations[1].update.is_some());
        assert!(e.operations[2].delete.is_some());
    }

    #[test]
    fn flink_block_parses_jobs() {
        let yaml = r#"
version: 1
services: [kafka, postgres, elastic]
flink:
  image: "flink:1.19"
  taskmanagers: 2
  slots: 4
  jobs:
    - name: "enrich"
      build: "mvn -q -DskipTests package"
      jar: "target/enrich-0.1.jar"
      entry_class: "com.acme.Enrich"
      args: ["--source", "events-in"]
      parallelism: 2
    - jar: "target/index-0.1.jar"
checks:
  - name: "event flows through"
    kafka: { produce: { topic: "events-in", key: "E1", message: { id: "E1" } } }
"#;
        let spec = NoworriesSpec::parse(yaml).unwrap();
        let f = spec.flink.as_ref().unwrap();
        assert_eq!(f.image.as_deref(), Some("flink:1.19"));
        assert_eq!(f.taskmanagers, Some(2));
        assert_eq!(f.slots, Some(4));
        assert_eq!(f.jobs.len(), 2);
        assert_eq!(f.jobs[0].name.as_deref(), Some("enrich"));
        assert_eq!(f.jobs[0].entry_class.as_deref(), Some("com.acme.Enrich"));
        assert_eq!(f.jobs[0].args, vec!["--source", "events-in"]);
        assert_eq!(f.jobs[0].parallelism, Some(2));
        assert_eq!(f.jobs[1].jar, "target/index-0.1.jar");
    }

    #[test]
    fn flink_requires_at_least_one_job() {
        let yaml = "version: 1\nservices: [kafka]\nflink:\n  jobs: []\n";
        assert!(NoworriesSpec::parse(yaml).is_err());
    }

    #[test]
    fn scenario_block_parses_on_a_check() {
        let yaml = r#"
version: 1
services: [kafka, postgres]
checks:
  - name: "burst of 500 orders all persist"
    scenario:
      kind: burst
      count: 500
      concurrency: 8
      rate_per_sec: 2000
      kafka:
        topic: orders-in
        key: "order-${seq}"
        message: { id: "${uuid}", version: "${i}" }
    db: { query: "SELECT count(*) n FROM orders", expect_row: { n: 500 } }
"#;
        let spec = NoworriesSpec::parse(yaml).unwrap();
        let sc = spec.checks[0].scenario.as_ref().unwrap();
        assert_eq!(sc.kind, "burst");
        assert_eq!(sc.count, Some(500));
        assert_eq!(sc.concurrency, Some(8));
        assert_eq!(sc.rate_per_sec, Some(2000));
        assert_eq!(sc.kafka.topic, "orders-in");
        assert_eq!(sc.kafka.key.as_deref(), Some("order-${seq}"));
    }
}
