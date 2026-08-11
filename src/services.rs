//! Service providers. Each infra dependency (Postgres, Kafka, Redis, ...) is a
//! `ServiceProvider` that knows how to describe itself as a compose service and
//! (in later phases) how to run its own assertions. Adding a new service is a
//! new impl + one line in `provider_for` — nothing else in the CLI changes.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use serde::Serialize;

use crate::spec::{ServiceDecl, ServiceKind};

/// Baked-in credentials for the ephemeral Postgres container.
pub const PG_USER: &str = "noworries";
pub const PG_PASSWORD: &str = "noworries";
pub const PG_DB: &str = "noworries";

/// A generated compose service plus the metadata lifecycle needs.
#[derive(Debug, Clone)]
pub struct ComposeService {
    pub name: String,
    pub kind: ServiceKind,
    pub container_port: u16,
    pub has_healthcheck: bool,
    /// If set, the host port was pre-assigned at generation time (e.g. Kafka,
    /// whose advertised listener must know its host port up front). Lifecycle
    /// uses this directly instead of asking Docker for a random mapping.
    pub host_port: Option<u16>,
    /// the compose service body (image/env/ports/healthcheck/...).
    pub body: serde_yaml::Value,
}

pub trait ServiceProvider {
    fn kind(&self) -> ServiceKind;

    /// Whether this service needs its host port fixed at generation time
    /// (true for Kafka; false for Postgres/Redis which can use random ports).
    fn needs_fixed_host_port(&self) -> bool {
        false
    }

    /// Build the compose fragment. `assigned_host_port` is `Some` iff
    /// `needs_fixed_host_port()` returned true.
    fn compose_service(
        &self,
        decl: &ServiceDecl,
        assigned_host_port: Option<u16>,
    ) -> Result<ComposeService>;
}

/// Resolve the provider for a service kind. Redis lands in a later phase;
/// returning `None` makes compose-gen fail loudly instead of half-working.
pub fn provider_for(kind: ServiceKind) -> Option<Box<dyn ServiceProvider>> {
    match kind {
        ServiceKind::Postgres => Some(Box::new(Postgres)),
        ServiceKind::Kafka => Some(Box::new(Kafka)),
        ServiceKind::Redis => Some(Box::new(Redis)),
    }
}

pub struct Postgres;

#[derive(Serialize)]
struct Healthcheck {
    test: Vec<String>,
    interval: String,
    timeout: String,
    retries: u32,
    start_period: String,
}

#[derive(Serialize)]
struct PgBody {
    image: String,
    environment: BTreeMap<String, String>,
    ports: Vec<String>,
    networks: Vec<String>,
    healthcheck: Healthcheck,
}

impl ServiceProvider for Postgres {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Postgres
    }

    fn compose_service(&self, decl: &ServiceDecl, _assigned: Option<u16>) -> Result<ComposeService> {
        let mut environment = BTreeMap::new();
        environment.insert("POSTGRES_USER".to_string(), PG_USER.to_string());
        environment.insert("POSTGRES_PASSWORD".to_string(), PG_PASSWORD.to_string());
        environment.insert("POSTGRES_DB".to_string(), PG_DB.to_string());

        let body = PgBody {
            image: decl.image.clone(),
            environment,
            // Bare container port => Docker assigns a random free host port.
            ports: vec!["5432".to_string()],
            networks: vec!["noworries".to_string()],
            healthcheck: Healthcheck {
                test: vec![
                    "CMD-SHELL".to_string(),
                    format!("pg_isready -U {PG_USER} -d {PG_DB}"),
                ],
                interval: "2s".to_string(),
                timeout: "3s".to_string(),
                retries: 30,
                start_period: "3s".to_string(),
            },
        };

        Ok(ComposeService {
            name: "postgres".to_string(),
            kind: ServiceKind::Postgres,
            container_port: 5432,
            has_healthcheck: true,
            host_port: None,
            body: serde_yaml::to_value(body).map_err(|e| anyhow!(e))?,
        })
    }
}

pub fn postgres_jdbc(host_port: u16) -> String {
    format!("jdbc:postgresql://127.0.0.1:{host_port}/{PG_DB}")
}

/// Kafka in single-node KRaft mode (no ZooKeeper). Because a Kafka client must
/// be given an address the broker *advertises*, and the app runs on the host,
/// the external listener is advertised at `localhost:<fixed host port>`. That
/// host port is chosen at generation time (hence `needs_fixed_host_port`).
pub struct Kafka;

const KAFKA_INTERNAL_PORT: u16 = 9092;
const KAFKA_EXTERNAL_PORT: u16 = 9093;
const KAFKA_CONTROLLER_PORT: u16 = 9091;

#[derive(Serialize)]
struct KafkaBody {
    image: String,
    environment: BTreeMap<String, String>,
    ports: Vec<String>,
    networks: Vec<String>,
    healthcheck: Healthcheck,
}

impl ServiceProvider for Kafka {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Kafka
    }

    fn needs_fixed_host_port(&self) -> bool {
        true
    }

    fn compose_service(&self, decl: &ServiceDecl, assigned: Option<u16>) -> Result<ComposeService> {
        let host_port = assigned
            .ok_or_else(|| anyhow!("internal error: Kafka requires a pre-assigned host port"))?;

        let mut env = BTreeMap::new();
        env.insert("KAFKA_NODE_ID".into(), "1".into());
        env.insert("KAFKA_PROCESS_ROLES".into(), "broker,controller".into());
        env.insert(
            "KAFKA_CONTROLLER_QUORUM_VOTERS".into(),
            format!("1@localhost:{KAFKA_CONTROLLER_PORT}"),
        );
        env.insert(
            "KAFKA_LISTENERS".into(),
            format!(
                "CONTROLLER://:{KAFKA_CONTROLLER_PORT},INTERNAL://:{KAFKA_INTERNAL_PORT},EXTERNAL://:{KAFKA_EXTERNAL_PORT}"
            ),
        );
        // INTERNAL is reachable inside the compose network as "kafka"; EXTERNAL
        // is reachable from the host at the fixed mapped port.
        env.insert(
            "KAFKA_ADVERTISED_LISTENERS".into(),
            format!("INTERNAL://kafka:{KAFKA_INTERNAL_PORT},EXTERNAL://localhost:{host_port}"),
        );
        env.insert(
            "KAFKA_LISTENER_SECURITY_PROTOCOL_MAP".into(),
            "CONTROLLER:PLAINTEXT,INTERNAL:PLAINTEXT,EXTERNAL:PLAINTEXT".into(),
        );
        env.insert("KAFKA_CONTROLLER_LISTENER_NAMES".into(), "CONTROLLER".into());
        env.insert("KAFKA_INTER_BROKER_LISTENER_NAME".into(), "INTERNAL".into());
        env.insert("KAFKA_OFFSETS_TOPIC_REPLICATION_FACTOR".into(), "1".into());
        env.insert("KAFKA_TRANSACTION_STATE_LOG_REPLICATION_FACTOR".into(), "1".into());
        env.insert("KAFKA_TRANSACTION_STATE_LOG_MIN_ISR".into(), "1".into());
        env.insert("KAFKA_GROUP_INITIAL_REBALANCE_DELAY_MS".into(), "0".into());
        env.insert("KAFKA_AUTO_CREATE_TOPICS_ENABLE".into(), "true".into());

        let body = KafkaBody {
            image: decl.image.clone(),
            environment: env,
            // Fixed host port -> container external listener.
            ports: vec![format!("{host_port}:{KAFKA_EXTERNAL_PORT}")],
            networks: vec!["noworries".into()],
            healthcheck: Healthcheck {
                test: vec![
                    "CMD-SHELL".into(),
                    format!(
                        "/opt/kafka/bin/kafka-topics.sh --bootstrap-server localhost:{KAFKA_INTERNAL_PORT} --list"
                    ),
                ],
                interval: "5s".into(),
                timeout: "5s".into(),
                retries: 30,
                start_period: "10s".into(),
            },
        };

        Ok(ComposeService {
            name: "kafka".to_string(),
            kind: ServiceKind::Kafka,
            container_port: KAFKA_EXTERNAL_PORT,
            has_healthcheck: true,
            host_port: Some(host_port),
            body: serde_yaml::to_value(body).map_err(|e| anyhow!(e))?,
        })
    }
}

/// Redis. Plain single-node; random host port (no advertised-address problem).
pub struct Redis;

#[derive(Serialize)]
struct RedisBody {
    image: String,
    ports: Vec<String>,
    networks: Vec<String>,
    healthcheck: Healthcheck,
}

impl ServiceProvider for Redis {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Redis
    }

    fn compose_service(&self, decl: &ServiceDecl, _assigned: Option<u16>) -> Result<ComposeService> {
        let body = RedisBody {
            image: decl.image.clone(),
            ports: vec!["6379".to_string()],
            networks: vec!["noworries".to_string()],
            healthcheck: Healthcheck {
                test: vec!["CMD".into(), "redis-cli".into(), "ping".into()],
                interval: "2s".into(),
                timeout: "3s".into(),
                retries: 30,
                start_period: "2s".into(),
            },
        };
        Ok(ComposeService {
            name: "redis".to_string(),
            kind: ServiceKind::Redis,
            container_port: 6379,
            has_healthcheck: true,
            host_port: None,
            body: serde_yaml::to_value(body).map_err(|e| anyhow!(e))?,
        })
    }
}
