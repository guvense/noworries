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

/// A JSON Schema for the on-disk `noworries.yml`, generated from the actual
/// deserialized types — so it always matches the installed binary's accepted
/// schema. Free-form YAML fields (bodies, expectations, messages) are typed as
/// `any`. Used by `noworries spec --format json` for editor completion and
/// programmatic queries (`.definitions.MockStub.properties`).
pub fn json_schema() -> String {
    let schema = schemars::schema_for!(RawSpec);
    serde_json::to_string_pretty(&schema).unwrap_or_else(|_| "{}".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    Postgres,
    Kafka,
    Redis,
    Elastic,
    Mysql,
    Mongodb,
    Cockroach,
    Opensearch,
    Mssql,
    Rabbitmq,
    Clickhouse,
    Cassandra,
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
            ServiceKind::Cockroach => "cockroach",
            ServiceKind::Opensearch => "opensearch",
            ServiceKind::Mssql => "mssql",
            ServiceKind::Rabbitmq => "rabbitmq",
            ServiceKind::Clickhouse => "clickhouse",
            ServiceKind::Cassandra => "cassandra",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        match token.to_ascii_lowercase().as_str() {
            "postgres" => Some(ServiceKind::Postgres),
            "kafka" => Some(ServiceKind::Kafka),
            "redis" => Some(ServiceKind::Redis),
            "elastic" | "elasticsearch" => Some(ServiceKind::Elastic),
            // MariaDB speaks the MySQL wire protocol, so it reuses the MySQL
            // provider/client; only the pulled image differs (see parse_service_decl).
            "mysql" | "mariadb" => Some(ServiceKind::Mysql),
            "mongodb" | "mongo" => Some(ServiceKind::Mongodb),
            // CockroachDB and TimescaleDB both speak the Postgres wire protocol.
            // Timescale is a true drop-in (same image family/env), so it reuses
            // the Postgres provider via a different image; Cockroach needs its
            // own container config and gets its own kind.
            "cockroach" | "cockroachdb" => Some(ServiceKind::Cockroach),
            "timescale" | "timescaledb" => Some(ServiceKind::Postgres),
            "opensearch" => Some(ServiceKind::Opensearch),
            "mssql" | "sqlserver" => Some(ServiceKind::Mssql),
            "rabbitmq" | "rabbit" | "amqp" => Some(ServiceKind::Rabbitmq),
            "clickhouse" => Some(ServiceKind::Clickhouse),
            // ScyllaDB is CQL-wire-compatible with Cassandra, so it reuses the
            // Cassandra provider with the `scylladb/scylla` image.
            "cassandra" | "scylla" | "scylladb" => Some(ServiceKind::Cassandra),
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
        ServiceKind::Cockroach => "cockroachdb/cockroach",
        ServiceKind::Opensearch => "opensearchproject/opensearch",
        ServiceKind::Mssql => "mcr.microsoft.com/mssql/server",
        ServiceKind::Rabbitmq => "rabbitmq",
        ServiceKind::Clickhouse => "clickhouse/clickhouse-server",
        ServiceKind::Cassandra => "cassandra",
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
        ServiceKind::Cockroach => "cockroachdb/cockroach:v24.2.0",
        ServiceKind::Opensearch => "opensearchproject/opensearch:2.17.1",
        ServiceKind::Mssql => "mcr.microsoft.com/mssql/server:2022-latest",
        ServiceKind::Rabbitmq => "rabbitmq:3.13-management",
        ServiceKind::Clickhouse => "clickhouse/clickhouse-server:24.8",
        ServiceKind::Cassandra => "cassandra:5.0",
    }
}

fn parse_service_decl(raw: &str) -> Result<ServiceDecl> {
    let value = raw.trim();
    if value.is_empty() {
        bail!("services entries must be non-empty strings like \"postgres:16-alpine\"");
    }
    let mut parts = value.splitn(2, ':');
    let kind_token = parts.next().unwrap_or("");
    let kind = ServiceKind::from_token(kind_token).ok_or_else(|| {
        anyhow!(
            "unsupported service \"{value}\". supported: postgres, timescaledb, cockroachdb, mysql, mariadb, mssql, mongodb, redis, kafka, rabbitmq, elasticsearch, opensearch, clickhouse, cassandra, scylladb"
        )
    })?;
    let tag = parts.next().map(|s| s.to_string());
    // Most tokens map onto their kind's default repository, but a few are
    // wire-compatible aliases that reuse a kind's provider while pulling a
    // different image (e.g. "mariadb" -> ServiceKind::Mysql, but the `mariadb`
    // image, not `mysql`).
    let (repo, default_img): (&str, String) = match kind_token.to_ascii_lowercase().as_str() {
        "mariadb" => ("mariadb", "mariadb:11".to_string()),
        // Timescale maps onto the Postgres kind but pulls the TimescaleDB image
        // (Postgres + the timescaledb extension pre-installed).
        "timescale" | "timescaledb" => {
            ("timescale/timescaledb", "timescale/timescaledb:2.17.2-pg16".to_string())
        }
        // Scylla maps onto the Cassandra kind but pulls the ScyllaDB image.
        "scylla" | "scylladb" => ("scylladb/scylla", "scylladb/scylla:6.1".to_string()),
        _ => (default_repo(kind), default_image(kind).to_string()),
    };
    // Expand "<kind>:<tag>" to the resolved repository, e.g.
    // "kafka:3.7" -> "apache/kafka:3.7"; bare "<kind>" -> the default image.
    let image = match &tag {
        Some(t) => format!("{repo}:{t}"),
        None => default_img,
    };
    Ok(ServiceDecl { kind, image, tag, raw: value.to_string() })
}

/// How to launch the app under test. All optional: `start` is auto-detected
/// from the resolved framework when omitted; `framework` forces a specific
/// framework instead of auto-detecting.
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
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
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
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
    /// Kafka topics to pre-create before the jobs start. noworries also
    /// auto-collects topics referenced in checks' `kafka.produce` /
    /// `kafka.expect_message`, so this is only for topics no check names.
    #[serde(default)]
    pub topics: Vec<String>,
    /// Jobs to build + submit, in order.
    pub jobs: Vec<FlinkJob>,
}

/// One Flink job: optionally built, then uploaded and run on the cluster.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HttpRequestSpec {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub body: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HttpExpectSpec {
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub body_contains: Option<serde_yaml::Value>,
    /// Assert the response arrived within this many milliseconds (latency SLO).
    #[serde(default, rename = "max_ms")]
    pub max_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbAssertion {
    pub query: String,
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub expect_row: Option<serde_yaml::Value>,
    #[serde(default)]
    pub expect_row_count: Option<i64>,
}

/// Kafka assertion. Supports producing a message to a topic (to exercise a
/// consumer the feature added) and/or expecting a message on a topic (to
/// verify a producer the feature added).
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KafkaAssertion {
    #[serde(default)]
    pub produce: Option<KafkaProduce>,
    #[serde(default)]
    pub expect_message: Option<KafkaExpect>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KafkaProduce {
    pub topic: String,
    #[schemars(with = "serde_json::Value")]
    pub message: serde_yaml::Value,
    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KafkaExpect {
    pub topic: String,
    #[schemars(with = "serde_json::Value")]
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
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
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
    /// Assert the achieved produce throughput was at least this many msgs/second.
    #[serde(default, rename = "expect_throughput_per_sec")]
    pub expect_throughput_per_sec: Option<u32>,
}

/// The Kafka target + message template for a [`ScenarioSpec`]. The `key` and
/// `message` templates may reference `${seq}` (global 0-based index), `${i}`
/// (per-key sequence), `${key}` (assigned key value), and `${uuid}` (unique id).
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioKafka {
    pub topic: String,
    #[serde(default)]
    pub key: Option<String>,
    #[schemars(with = "serde_json::Value")]
    pub message: serde_yaml::Value,
}

/// Redis assertion: check a key exists and/or its (possibly JSON) value matches.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RedisAssertion {
    pub key: String,
    #[serde(default)]
    pub expect_exists: Option<bool>,
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub expect_value: Option<serde_yaml::Value>,
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub expect_value_contains: Option<serde_yaml::Value>,
}

/// An index template applied to Elasticsearch before the app runs (so
/// app-created indices pick up the right mapping). The `body` is generated by
/// Claude from the code, or pasted from production. `legacy` uses the old
/// `_template` API (ES < 7.8); otherwise `_index_template` (ES 7.8+ and 8).
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ElasticTemplate {
    pub name: String,
    #[schemars(with = "serde_json::Value")]
    pub body: serde_yaml::Value,
    #[serde(default)]
    pub legacy: bool,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ElasticInsert {
    #[serde(default)]
    pub id: Option<String>,
    #[schemars(with = "serde_json::Value")]
    pub document: serde_yaml::Value,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ElasticUpdate {
    pub id: String,
    #[schemars(with = "serde_json::Value")]
    pub document: serde_yaml::Value,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ElasticDelete {
    pub id: String,
}

/// A noworries-run Elasticsearch operation (setup or trigger). Exactly one of
/// `insert` / `update` / `delete` is set per list entry, e.g.
/// `- insert: { id: "X1", document: { ... } }`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
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
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
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
    #[schemars(with = "Option<serde_json::Value>")]
    pub expect_source_contains: Option<serde_yaml::Value>,
    /// Verify via a search: the Elasticsearch query DSL body (the `query` value).
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub query: Option<serde_yaml::Value>,
    #[serde(default)]
    pub expect_hits: Option<i64>,
}

/// Authentication applied to every check's HTTP request. Extensible: each auth
/// style is its own optional sub-block, so adding a new one (mTLS, OAuth2, ...)
/// is a new field + a new arm in the resolver — nothing else changes. Set the
/// one(s) you need; string values support `${ENV_VAR}` interpolation so secrets
/// stay out of the repo.
#[derive(Debug, Clone, Deserialize, Default, schemars::JsonSchema)]
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
    /// OpenID Connect / OAuth2: fetch a token from the provider and send it as
    /// a bearer. Covers Keycloak / Auth0 / Cognito / Entra without hand-rolling
    /// the token request as an `auth.login`.
    #[serde(default)]
    pub oidc: Option<AuthOidc>,
}

/// OAuth2 / OpenID Connect token acquisition.
///
/// Give `issuer` and the token endpoint is read from
/// `<issuer>/.well-known/openid-configuration`; give `token_url` to skip
/// discovery (or when the provider doesn't publish a document).
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthOidc {
    /// Issuer base URL — discovery reads `<issuer>/.well-known/openid-configuration`.
    #[serde(default)]
    pub issuer: Option<String>,
    /// Token endpoint, used as-is instead of discovery.
    #[serde(default)]
    pub token_url: Option<String>,
    pub client_id: String,
    /// Confidential clients only; omit for a public client.
    #[serde(default)]
    pub client_secret: Option<String>,
    /// `client_credentials` (default) or `password`.
    #[serde(default)]
    pub grant: Option<String>,
    /// Resource-owner credentials, for `grant: password`.
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    /// Space-separated scopes.
    #[serde(default)]
    pub scope: Option<String>,
    /// `audience` parameter (Auth0 requires it to get a JWT for your API).
    #[serde(default)]
    pub audience: Option<String>,
    /// How the client secret is presented: `post` (default, in the form body)
    /// or `basic` (an `Authorization: Basic` header — Cognito requires this for
    /// clients that have a secret).
    #[serde(default)]
    pub client_auth: Option<String>,
    /// Extra form parameters passed through verbatim.
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    /// JSON path to the token in the response (default `$.access_token`).
    #[serde(default)]
    pub token_from: Option<String>,
    /// Header to place the token in (default `Authorization`).
    #[serde(default)]
    pub header: Option<String>,
    /// Scheme prefix (default `Bearer`; empty string = raw token).
    #[serde(default)]
    pub scheme: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthBearer {
    pub token: String,
    #[serde(default)]
    pub header: Option<String>,
    #[serde(default)]
    pub scheme: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthBasic {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
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

/// An **external / upstream** service the app under test calls out to but that
/// noworries does NOT stand up (a partner sandbox API, an auth server, ...).
/// noworries injects its URL and credentials into the app's environment so the
/// app can reach it, both under the app's own env-var names (`env`, `url_env`,
/// per-auth `*_env`) and under conventional `NOWORRIES_EXTERNAL_<NAME>_*` names.
/// All string values support `${VAR}` interpolation, so secrets live in the
/// gitignored `.noworries.env` and are prompted for when missing.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalSpec {
    /// Logical name; drives the conventional env-var prefix
    /// (`NOWORRIES_EXTERNAL_<NAME>_*`, uppercased, non-alphanumerics → `_`).
    pub name: String,
    /// The (sandbox) base URL the app should call.
    #[serde(default)]
    pub url: Option<String>,
    /// Also expose the URL under this app-specific env var (e.g. `PAYMENTS_BASE_URL`).
    #[serde(default, rename = "url_env")]
    pub url_env: Option<String>,
    /// Extra literal env vars to set for this dependency (values interpolate `${VAR}`).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Credentials for the dependency, materialized into env vars.
    #[serde(default)]
    pub auth: Option<ExternalAuth>,
    /// Stand up an in-process **mock** of this dependency instead of pointing at a
    /// real sandbox: noworries serves the `stubs` on a local port, injects that
    /// URL as the external's URL, and records every request the app makes so
    /// checks can assert on them via `external_calls`.
    #[serde(default)]
    pub mock: Option<MockSpec>,
}

/// In-process mock of an external dependency.
#[derive(Debug, Clone, Deserialize, Default, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MockSpec {
    /// Response rules, matched top-to-bottom; the first match wins. An unmatched
    /// request still gets recorded and answered with 200 (empty body).
    #[serde(default)]
    pub stubs: Vec<MockStub>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MockStub {
    #[serde(rename = "when")]
    pub when_: MockWhen,
    #[serde(default)]
    pub respond: MockRespond,
}

/// Match condition for a [`MockStub`].
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MockWhen {
    /// HTTP method to match (case-insensitive); omit to match any.
    #[serde(default)]
    pub method: Option<String>,
    /// Request path to match exactly (query string ignored).
    pub path: String,
    /// Deep-subset match against the request's JSON body — so the same
    /// method+path can return different responses per payload (e.g. `amount: 100`
    /// vs `amount: 200`). Stubs are tried top-to-bottom; put the specific ones
    /// (with `body_contains`) before a catch-all.
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub body_contains: Option<serde_yaml::Value>,
}

/// Canned response for a [`MockStub`].
#[derive(Debug, Clone, Deserialize, Default, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MockRespond {
    /// HTTP status to return (default 200).
    #[serde(default)]
    pub status: Option<u16>,
    /// JSON body to return (a plain string is sent as-is).
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub body: Option<serde_yaml::Value>,
    /// Extra response headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Artificial latency before responding (ms) — to test client timeout /
    /// circuit-breaker behaviour.
    #[serde(default, rename = "delay_ms")]
    pub delay_ms: Option<u64>,
}

/// Auth for an [`ExternalSpec`]. Set the one style the dependency uses. Each
/// materializes both the raw parts and a ready-to-send header value.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalAuth {
    #[serde(default)]
    pub basic: Option<ExternalBasic>,
    #[serde(default)]
    pub bearer: Option<ExternalBearer>,
    #[serde(default)]
    pub api_key: Option<ExternalApiKey>,
}

/// Basic auth. Sets `_USER`/`_PASSWORD` (raw) and `_AUTHORIZATION` =
/// `Basic base64(user:pass)`. Optional `*_env` also aliases to app-specific names.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalBasic {
    pub username: String,
    pub password: String,
    #[serde(default, rename = "username_env")]
    pub username_env: Option<String>,
    #[serde(default, rename = "password_env")]
    pub password_env: Option<String>,
    /// App-specific env var to also receive the ready `Basic ...` header value.
    #[serde(default, rename = "header_env")]
    pub header_env: Option<String>,
}

/// Bearer/token auth. Sets `_TOKEN` (raw) and `_AUTHORIZATION` = `<scheme> <token>`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalBearer {
    pub token: String,
    /// Scheme prefix for the header value (default `Bearer`; empty = raw token).
    #[serde(default)]
    pub scheme: Option<String>,
    #[serde(default, rename = "token_env")]
    pub token_env: Option<String>,
    #[serde(default, rename = "header_env")]
    pub header_env: Option<String>,
}

/// API-key auth. Sets `_API_KEY` (raw value) and `_API_KEY_HEADER` (the header
/// name the app should use, default `X-API-Key`).
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalApiKey {
    pub value: String,
    /// The HTTP header name the app should send the key in (default `X-API-Key`).
    #[serde(default)]
    pub header: Option<String>,
    #[serde(default, rename = "value_env")]
    pub value_env: Option<String>,
}

/// MySQL assertion. `seed` statements run BEFORE the request (to set up data);
/// `query` + `expect_row`/`expect_row_count` verify state afterwards. This is
/// the "seed data -> hit the API -> check what changed" flow.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
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
    #[schemars(with = "Option<serde_json::Value>")]
    pub expect_row: Option<serde_yaml::Value>,
    #[serde(default)]
    pub expect_row_count: Option<i64>,
}

/// A noworries-run MongoDB operation (seed/trigger). Exactly one of
/// `insert` / `update` / `delete` per entry.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MongoOp {
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub insert: Option<serde_yaml::Value>,
    #[serde(default)]
    pub update: Option<MongoUpdate>,
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub delete: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MongoUpdate {
    #[schemars(with = "serde_json::Value")]
    pub filter: serde_yaml::Value,
    /// Fields to `$set` on the matched documents.
    #[schemars(with = "serde_json::Value")]
    pub set: serde_yaml::Value,
}

/// MongoDB assertion. `seed` operations run before the request; `find` +
/// `expect_doc_contains` / `expect_count` verify afterwards.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MongoAssertion {
    pub database: String,
    pub collection: String,
    #[serde(default)]
    pub seed: Vec<MongoOp>,
    /// Filter (Mongo query document) used for verification.
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub find: Option<serde_yaml::Value>,
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub expect_doc_contains: Option<serde_yaml::Value>,
    #[serde(default)]
    pub expect_count: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckSpec {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Run this check as one of the `users:` identities instead of the
    /// top-level `auth:` — the way to exercise RBAC (same request, different
    /// role, different expected status).
    #[serde(default, rename = "as")]
    pub as_user: Option<String>,
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
    /// Assertions against the app's captured log (`.noworries/app.log`).
    #[serde(default)]
    pub logs: Option<LogAssertion>,
    /// Assert the app called a mocked external during this check.
    #[serde(default, rename = "external_calls")]
    pub external_calls: Vec<ExternalCallAssertion>,
    // --- extensible assertion types (see src/checks/) ---
    #[serde(default)]
    pub graphql: Option<GraphqlAssertion>,
    #[serde(default)]
    pub metrics: Option<MetricsAssertion>,
    #[serde(default)]
    pub snapshot: Option<SnapshotAssertion>,
    #[serde(default)]
    pub schema: Option<SchemaAssertion>,
    #[serde(default)]
    pub sse: Option<SseAssertion>,
    #[serde(default)]
    pub websocket: Option<WebsocketAssertion>,
    #[serde(default)]
    pub grpc: Option<GrpcAssertion>,
    #[serde(default)]
    pub traces: Option<TracesAssertion>,
    #[serde(default)]
    pub clickhouse: Option<ClickhouseAssertion>,
    #[serde(default)]
    pub rabbitmq: Option<RabbitmqAssertion>,
    #[serde(default)]
    pub cassandra: Option<CassandraAssertion>,
    #[serde(default)]
    pub security: Option<SecurityAssertion>,
}

fn default_true() -> bool {
    true
}

/// GraphQL query/mutation over HTTP POST; asserts on `data` and `errors`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphqlAssertion {
    /// Endpoint path (relative to the app) or absolute URL. Default `/graphql`.
    #[serde(default = "default_graphql_path")]
    pub path: String,
    pub query: String,
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub variables: Option<serde_yaml::Value>,
    /// Deep-subset match against the response `data`.
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub expect_data: Option<serde_yaml::Value>,
    /// Fail if the response has a non-empty `errors` array (default true).
    #[serde(default = "default_true")]
    pub expect_no_errors: bool,
}

fn default_graphql_path() -> String {
    "/graphql".to_string()
}

/// Prometheus metric assertion: scrape a metrics endpoint, match one series,
/// and compare its value.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MetricsAssertion {
    /// Metrics endpoint path or absolute URL. Default `/metrics`.
    #[serde(default = "default_metrics_path")]
    pub path: String,
    /// Metric name (e.g. `http_server_requests_seconds_count`).
    pub metric: String,
    /// Labels the series must have (subset match).
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Comparison: `">= 1"`, `"== 3"`, `"<= 5"`, `"> 0"`, `"< 10"`, `"= 2"`.
    /// Omit to just assert the series is present.
    #[serde(default)]
    pub expect: Option<String>,
}

fn default_metrics_path() -> String {
    "/metrics".to_string()
}

/// Golden-file assertion on the check's HTTP `request` response body.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotAssertion {
    /// Golden file path (relative to the project dir).
    pub file: String,
    /// JSON paths to blank out before comparing (volatile fields like ids/times).
    #[serde(default)]
    pub ignore: Vec<String>,
}

/// Postgres schema assertion: check a table's columns.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaAssertion {
    pub table: String,
    /// DB schema (default `public`).
    #[serde(default)]
    pub schema: Option<String>,
    /// Expected `column -> data_type` (subset; type compared loosely).
    #[serde(default)]
    pub columns: BTreeMap<String, String>,
    /// Columns that must exist (type unchecked).
    #[serde(default, rename = "has_columns")]
    pub has_columns: Vec<String>,
}

/// ClickHouse assertion: run a SQL query over the HTTP interface (port 8123)
/// and check the result. Reuses the existing `ureq` client — no native driver.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClickhouseAssertion {
    /// SQL query to run (e.g. `SELECT count() FROM events WHERE user_id = 42`).
    pub query: String,
    /// Assert the query returns exactly this many rows.
    #[serde(default, rename = "expect_rows")]
    pub expect_rows: Option<usize>,
    /// Assert the first row equals this `column -> value` map (subset match,
    /// loose type compare — ClickHouse returns 64-bit ints as JSON strings).
    #[serde(default, rename = "expect_row")]
    #[schemars(with = "Option<serde_json::Value>")]
    pub expect_row: Option<serde_yaml::Value>,
    /// Assert the single scalar result (first column of the first row) equals
    /// this value. Handy for `SELECT count() ...`.
    #[serde(default, rename = "expect_value")]
    #[schemars(with = "Option<serde_json::Value>")]
    pub expect_value: Option<serde_yaml::Value>,
}

/// Cassandra assertion: run a CQL query via `cqlsh` inside the container and
/// check the result. Uses `docker compose exec` — no native CQL driver. Also
/// covers ScyllaDB (same kind, same `cqlsh`).
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CassandraAssertion {
    /// CQL query (e.g. `SELECT count(*) FROM app.orders WHERE id = 42`). A
    /// `SELECT` is rewritten to `SELECT JSON` internally for parseable output.
    pub query: String,
    /// Assert the query returns exactly this many rows.
    #[serde(default, rename = "expect_rows")]
    pub expect_rows: Option<usize>,
    /// Assert the first row equals this `column -> value` map (subset, loose
    /// type compare).
    #[serde(default, rename = "expect_row")]
    #[schemars(with = "Option<serde_json::Value>")]
    pub expect_row: Option<serde_yaml::Value>,
    /// Assert the single scalar result (single-column query) equals this value.
    #[serde(default, rename = "expect_value")]
    #[schemars(with = "Option<serde_json::Value>")]
    pub expect_value: Option<serde_yaml::Value>,
}

/// RabbitMQ assertion: inspect a queue via the management HTTP API (port 15672).
/// Reuses the existing `ureq` client — no AMQP driver.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RabbitmqAssertion {
    /// Queue name to inspect.
    pub queue: String,
    /// Virtual host (default `/`).
    #[serde(default)]
    pub vhost: Option<String>,
    /// Assert the queue exists (default `true`). Set `false` to assert absence.
    #[serde(default, rename = "expect_exists")]
    pub expect_exists: Option<bool>,
    /// Assert the queue holds at least this many messages (ready + unacked).
    #[serde(default)]
    pub min_messages: Option<u64>,
    /// Assert the queue holds exactly this many messages.
    #[serde(default)]
    pub expect_messages: Option<u64>,
}

/// Defensive security assertion: probe an endpoint of the app under test with
/// abuse cases and assert it behaves safely. This verifies *your own* app's
/// hardening (auth enforced, hostile input handled without a server crash, no
/// internal error leakage, security headers present) — it only ever talks to the
/// app noworries started, and asserts safe behaviour rather than exploiting.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SecurityAssertion {
    /// Endpoint path to probe (relative to the app, or absolute). Defaults to
    /// the check's `request.path`.
    #[serde(default)]
    pub path: Option<String>,
    /// HTTP method. Defaults to the check's `request.method`, else `GET`.
    #[serde(default)]
    pub method: Option<String>,
    /// A JSON body used as the baseline for input probes. Defaults to the
    /// check's `request.body`.
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub body: Option<serde_yaml::Value>,
    /// Assert an unauthenticated request (auth stripped) is rejected — 401/403.
    #[serde(default)]
    pub require_auth: Option<bool>,
    /// Assert hostile / malformed input is handled without a server error: no
    /// probe returns 5xx, and a malformed body returns a 4xx.
    #[serde(default)]
    pub reject_bad_input: Option<bool>,
    /// Assert responses never leak stack traces / server-internal error detail.
    #[serde(default)]
    pub no_error_leak: Option<bool>,
    /// Response headers that must be present (e.g. `X-Content-Type-Options`).
    #[serde(default)]
    pub require_headers: Vec<String>,
}

/// Server-Sent-Events assertion: read an event stream until a matching event.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SseAssertion {
    /// Stream path (relative to the app) or absolute URL.
    pub path: String,
    /// Deep-subset match against an event's `data` (parsed as JSON).
    #[schemars(with = "serde_json::Value")]
    pub contains: serde_yaml::Value,
    #[serde(default, rename = "timeout_ms")]
    pub timeout_ms: Option<u64>,
}

/// WebSocket assertion: connect, optionally send a message, await a match.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebsocketAssertion {
    /// `ws://`/`wss://` URL, or a relative path (→ `ws://127.0.0.1:<app>`).
    pub url: String,
    /// Message to send on connect (JSON; a plain string is sent as text).
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub send: Option<serde_yaml::Value>,
    /// Deep-subset match against a received message (parsed as JSON).
    #[serde(default, rename = "expect_message")]
    #[schemars(with = "serde_json::Value")]
    pub expect_message: serde_yaml::Value,
    #[serde(default, rename = "timeout_ms")]
    pub timeout_ms: Option<u64>,
}

/// gRPC assertion via `grpcurl` (shelled out). Requires `grpcurl` on PATH and
/// server reflection, or explicit `protos`/`import_paths`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrpcAssertion {
    /// `host:port` (supports `${VAR}`).
    pub target: String,
    /// Fully-qualified method: `package.Service/Method`.
    pub method: String,
    /// Request message as JSON.
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub data: Option<serde_yaml::Value>,
    /// Deep-subset match against the response JSON.
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub expect_contains: Option<serde_yaml::Value>,
    /// Use plaintext (no TLS). Default true.
    #[serde(default = "default_true")]
    pub plaintext: bool,
    /// `-import-path` args for grpcurl (when not using reflection).
    #[serde(default)]
    pub import_paths: Vec<String>,
    /// `-proto` files for grpcurl (when not using reflection).
    #[serde(default)]
    pub protos: Vec<String>,
}

/// OpenTelemetry trace assertion: query a Jaeger/Tempo-compatible HTTP API and
/// assert matching traces exist.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracesAssertion {
    /// Trace-query URL (e.g. Jaeger `http://127.0.0.1:16686/api/traces`).
    pub query_url: String,
    /// Service name to query.
    pub service: String,
    /// Optional operation/span name filter.
    #[serde(default)]
    pub operation: Option<String>,
    /// Span tags that must match (subset).
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
    /// Minimum number of matching traces required (default 1).
    #[serde(default, rename = "min_count")]
    pub min_count: Option<usize>,
}

/// Assertions against the app's captured stdout/stderr log.
#[derive(Debug, Clone, Deserialize, Default, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LogAssertion {
    /// Substrings that must ALL appear in the log.
    #[serde(default)]
    pub contains: Vec<String>,
    /// Substrings that must NOT appear (e.g. `ERROR`, `Exception`).
    #[serde(default)]
    pub absent: Vec<String>,
}

/// Assert the app made a matching request to a mocked external.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalCallAssertion {
    /// Which external's mock to inspect (matches `externals[].name`).
    pub external: String,
    /// HTTP method to match (case-insensitive); omit to match any.
    #[serde(default)]
    pub method: Option<String>,
    /// Request path to match exactly.
    pub path: String,
    /// Deep-subset match against the recorded request's JSON body.
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub body_contains: Option<serde_yaml::Value>,
    /// Exact number of matching calls required; omit for "at least one".
    #[serde(default)]
    pub times: Option<usize>,
    /// How long to wait for the app to make the call (ms), for asynchronous
    /// flows. Omit to use the check's default eventual-consistency window (~6s,
    /// scaled up by a scenario's size).
    #[serde(default, rename = "timeout_ms")]
    pub timeout_ms: Option<u64>,
}

/// Raw shape as it appears on disk (services are strings here).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RawSpec {
    version: u32,
    services: Vec<String>,
    #[serde(default)]
    app: Option<AppSpec>,
    #[serde(default)]
    flink: Option<FlinkSpec>,
    #[serde(default)]
    externals: Vec<ExternalSpec>,
    #[serde(default)]
    setup: Vec<String>,
    #[serde(default)]
    auth: Option<AuthSpec>,
    #[serde(default)]
    users: BTreeMap<String, AuthSpec>,
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
    pub externals: Vec<ExternalSpec>,
    /// Shell commands run (with the app's env wiring) after infra is healthy and
    /// before the app starts — for DB migrations / fixtures (Flyway, Liquibase,
    /// Prisma, Alembic, raw SQL, …).
    pub setup: Vec<String>,
    pub auth: Option<AuthSpec>,
    /// Named identities a check can select with `as:` — one entry per role, each
    /// the same shape as `auth:`. Resolved once, before the checks run.
    pub users: BTreeMap<String, AuthSpec>,
    pub checks: Vec<CheckSpec>,
}

impl NoworriesSpec {
    pub fn parse(source: &str) -> Result<Self> {
        // Surface serde's exact message (e.g. `orders[0]: unknown field
        // "method", expected one of ... at line 7 column 9`) so a human or an AI
        // can see which field/line is wrong instead of an anonymous failure.
        let raw: RawSpec = serde_yaml::from_str(source)
            .map_err(|e| anyhow::anyhow!("could not parse {SPEC_FILENAME}: {e}"))?;
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
            // Catch a typo here rather than silently running the check as the
            // default identity — an RBAC check that quietly used the wrong user
            // would pass for the wrong reason.
            if let Some(user) = &c.as_user {
                if !raw.users.contains_key(user) {
                    let known: Vec<&str> = raw.users.keys().map(|k| k.as_str()).collect();
                    bail!(
                        "{SPEC_FILENAME}: check \"{}\" runs as \"{user}\", which is not declared under \"users\"{}",
                        c.name,
                        if known.is_empty() {
                            " (no users are declared)".to_string()
                        } else {
                            format!(" (declared: {})", known.join(", "))
                        }
                    );
                }
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
        for e in &raw.externals {
            if e.name.trim().is_empty() {
                bail!("{SPEC_FILENAME}: every external needs a non-empty \"name\".");
            }
        }
        Ok(NoworriesSpec {
            version: 1,
            services,
            app: raw.app,
            flink: raw.flink,
            externals: raw.externals,
            setup: raw.setup,
            auth: raw.auth,
            users: raw.users,
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
        assert!(NoworriesSpec::parse("version: 1\nservices: [nats:2]\n").is_err());
    }

    #[test]
    fn cockroach_tokens_and_image_expansion() {
        let s = parse_services("version: 1\nservices: [cockroach, cockroachdb:v24.2.0]\n");
        assert_eq!(s[0].kind, ServiceKind::Cockroach);
        assert_eq!(s[0].image, "cockroachdb/cockroach:v24.2.0");
        assert_eq!(s[1].kind, ServiceKind::Cockroach);
        assert_eq!(s[1].image, "cockroachdb/cockroach:v24.2.0");
    }

    #[test]
    fn timescale_reuses_postgres_kind_with_timescale_image() {
        // TimescaleDB is Postgres + an extension, so it maps onto the Postgres
        // provider/client but must pull the `timescale/timescaledb` image.
        let s = parse_services("version: 1\nservices: [timescaledb, timescale:2.17.2-pg16]\n");
        assert_eq!(s[0].kind, ServiceKind::Postgres);
        assert_eq!(s[0].image, "timescale/timescaledb:2.17.2-pg16");
        assert_eq!(s[1].kind, ServiceKind::Postgres);
        assert_eq!(s[1].image, "timescale/timescaledb:2.17.2-pg16");
    }

    #[test]
    fn opensearch_token_and_image_expansion() {
        let s = parse_services("version: 1\nservices: [opensearch, opensearch:2.17.1]\n");
        assert_eq!(s[0].kind, ServiceKind::Opensearch);
        assert_eq!(s[0].image, "opensearchproject/opensearch:2.17.1");
        assert_eq!(s[1].image, "opensearchproject/opensearch:2.17.1");
    }

    #[test]
    fn mssql_tokens_and_image_expansion() {
        let s = parse_services("version: 1\nservices: [mssql, sqlserver:2022-latest]\n");
        assert_eq!(s[0].kind, ServiceKind::Mssql);
        assert_eq!(s[0].image, "mcr.microsoft.com/mssql/server:2022-latest");
        assert_eq!(s[1].kind, ServiceKind::Mssql);
        assert_eq!(s[1].image, "mcr.microsoft.com/mssql/server:2022-latest");
    }

    #[test]
    fn rabbitmq_tokens_and_image_expansion() {
        let s = parse_services("version: 1\nservices: [rabbitmq, rabbit, amqp]\n");
        assert_eq!(s[0].kind, ServiceKind::Rabbitmq);
        assert_eq!(s[0].image, "rabbitmq:3.13-management");
        assert_eq!(s[1].kind, ServiceKind::Rabbitmq);
        assert_eq!(s[2].kind, ServiceKind::Rabbitmq);
    }

    #[test]
    fn clickhouse_token_and_image_expansion() {
        let s = parse_services("version: 1\nservices: [clickhouse, clickhouse:24.8]\n");
        assert_eq!(s[0].kind, ServiceKind::Clickhouse);
        assert_eq!(s[0].image, "clickhouse/clickhouse-server:24.8");
        assert_eq!(s[1].image, "clickhouse/clickhouse-server:24.8");
    }

    #[test]
    fn cassandra_and_scylla_tokens_and_image_expansion() {
        let s = parse_services("version: 1\nservices: [cassandra, scylla, scylladb:6.1]\n");
        assert_eq!(s[0].kind, ServiceKind::Cassandra);
        assert_eq!(s[0].image, "cassandra:5.0");
        // Scylla is CQL-wire-compatible, so it reuses the Cassandra kind with
        // the scylladb image.
        assert_eq!(s[1].kind, ServiceKind::Cassandra);
        assert_eq!(s[1].image, "scylladb/scylla:6.1");
        assert_eq!(s[2].kind, ServiceKind::Cassandra);
        assert_eq!(s[2].image, "scylladb/scylla:6.1");
    }

    #[test]
    fn mariadb_reuses_mysql_kind_with_mariadb_image() {
        // MariaDB is wire-compatible with MySQL, so it maps onto the MySQL
        // provider/client but must pull the `mariadb` image.
        let s = parse_services("version: 1\nservices: [mariadb, mariadb:11.4]\n");
        assert_eq!(s[0].kind, ServiceKind::Mysql);
        assert_eq!(s[0].image, "mariadb:11");
        assert_eq!(s[1].kind, ServiceKind::Mysql);
        assert_eq!(s[1].image, "mariadb:11.4");
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
    fn parse_error_names_the_bad_field() {
        // An unknown field must produce a message that identifies it (so an AI
        // can self-correct from a minimal probe instead of guessing).
        let yaml = "version: 1\nservices: [postgres]\nchecks:\n  - name: x\n    request: { method: GET, path: /, bogus: 1 }\n";
        let err = NoworriesSpec::parse(yaml).err().unwrap().to_string();
        assert!(err.contains("unknown field") && err.contains("bogus"), "got: {err}");
    }

    #[test]
    fn flink_requires_at_least_one_job() {
        let yaml = "version: 1\nservices: [kafka]\nflink:\n  jobs: []\n";
        assert!(NoworriesSpec::parse(yaml).is_err());
    }

    #[test]
    fn logs_setup_latency_throughput_mock_parse() {
        let yaml = r#"
version: 1
services: [postgres, kafka]
setup:
  - "./mvnw -q flyway:migrate"
  - "psql -f fixtures.sql"
externals:
  - name: payments
    mock:
      stubs:
        - when: { method: POST, path: /charge }
          respond: { status: 201, body: { id: "ch_1" }, headers: { X-Trace: "t" } }
checks:
  - name: "fast + logged + called"
    request: { method: POST, path: /orders, body: { sku: "A" } }
    expect: { status: 201, max_ms: 300 }
    logs: { contains: ["OrderCreated"], absent: ["ERROR", "Exception"] }
    external_calls:
      - external: payments
        method: POST
        path: /charge
        body_contains: { amount: 100 }
        times: 1
  - name: "burst throughput"
    scenario:
      kind: burst
      count: 100
      expect_throughput_per_sec: 500
      kafka: { topic: t, message: { id: "${seq}" } }
"#;
        let spec = NoworriesSpec::parse(yaml).unwrap();
        assert_eq!(spec.setup.len(), 2);
        let ext = &spec.externals[0];
        let mock = ext.mock.as_ref().unwrap();
        assert_eq!(mock.stubs[0].when_.path, "/charge");
        assert_eq!(mock.stubs[0].respond.status, Some(201));
        let c0 = &spec.checks[0];
        assert_eq!(c0.expect.as_ref().unwrap().max_ms, Some(300));
        let logs = c0.logs.as_ref().unwrap();
        assert_eq!(logs.contains, vec!["OrderCreated"]);
        assert_eq!(logs.absent, vec!["ERROR", "Exception"]);
        assert_eq!(c0.external_calls[0].path, "/charge");
        assert_eq!(c0.external_calls[0].times, Some(1));
        let sc = spec.checks[1].scenario.as_ref().unwrap();
        assert_eq!(sc.expect_throughput_per_sec, Some(500));
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
