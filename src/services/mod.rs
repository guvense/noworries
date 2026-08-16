//! Service providers. Each infra dependency (Postgres, Kafka, Redis, ...) is a
//! [`ServiceProvider`] living in its own module under `services/`. This module
//! owns only the interface, the shared types, and the registry — adding a new
//! service is a new file + one line in [`provider_for`], nothing else.

use anyhow::Result;
use serde::Serialize;

use crate::spec::{ServiceDecl, ServiceKind};

pub mod cassandra;
pub mod clickhouse;
pub mod cockroach;
pub mod elastic;
pub mod kafka;
pub mod mongodb;
pub mod mssql;
pub mod mysql;
pub mod opensearch;
pub mod postgres;
pub mod rabbitmq;
pub mod redis;

pub use cassandra::Cassandra;
pub use clickhouse::Clickhouse;
pub use cockroach::Cockroach;
pub use elastic::Elastic;
pub use kafka::Kafka;
pub use mongodb::Mongodb;
pub use mssql::Mssql;
pub use mysql::Mysql;
pub use opensearch::Opensearch;
pub use postgres::{postgres_jdbc, Postgres};
pub use rabbitmq::Rabbitmq;
pub use redis::Redis;

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

/// The interface every service implements. Keep this small and stable; put
/// behaviour in the per-service modules.
pub trait ServiceProvider {
    fn kind(&self) -> ServiceKind;

    /// Whether this service needs its host port fixed at generation time
    /// (true for Kafka; false for Postgres/Redis which can use random ports).
    fn needs_fixed_host_port(&self) -> bool {
        false
    }

    /// Secondary container ports (besides `container_port`) that should also
    /// have their random host mapping resolved and recorded on the endpoint —
    /// e.g. RabbitMQ's management API (15672) alongside AMQP (5672). Empty for
    /// single-port services. The ports listed here must appear in the compose
    /// `ports` this provider emits, or Docker will have no mapping to resolve.
    fn aux_ports(&self) -> &'static [u16] {
        &[]
    }

    /// Build the compose fragment. `assigned_host_port` is `Some` iff
    /// `needs_fixed_host_port()` returned true.
    fn compose_service(
        &self,
        decl: &ServiceDecl,
        assigned_host_port: Option<u16>,
    ) -> Result<ComposeService>;
}

/// Shared Docker Compose `healthcheck` fragment used by the service modules.
#[derive(Serialize)]
pub(crate) struct Healthcheck {
    pub(crate) test: Vec<String>,
    pub(crate) interval: String,
    pub(crate) timeout: String,
    pub(crate) retries: u32,
    pub(crate) start_period: String,
}

/// Registry / composition root: resolve the provider for a service kind. This
/// is the single place that knows the concrete implementations.
pub fn provider_for(kind: ServiceKind) -> Option<Box<dyn ServiceProvider>> {
    match kind {
        ServiceKind::Postgres => Some(Box::new(Postgres)),
        ServiceKind::Kafka => Some(Box::new(Kafka)),
        ServiceKind::Redis => Some(Box::new(Redis)),
        ServiceKind::Elastic => Some(Box::new(Elastic)),
        ServiceKind::Mysql => Some(Box::new(Mysql)),
        ServiceKind::Mongodb => Some(Box::new(Mongodb)),
        ServiceKind::Cockroach => Some(Box::new(Cockroach)),
        ServiceKind::Opensearch => Some(Box::new(Opensearch)),
        ServiceKind::Mssql => Some(Box::new(Mssql)),
        ServiceKind::Rabbitmq => Some(Box::new(Rabbitmq)),
        ServiceKind::Clickhouse => Some(Box::new(Clickhouse)),
        ServiceKind::Cassandra => Some(Box::new(Cassandra)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{default_image, ServiceDecl};

    fn decl(kind: ServiceKind) -> ServiceDecl {
        ServiceDecl {
            kind,
            image: default_image(kind).to_string(),
            tag: None,
            raw: kind.as_str().to_string(),
        }
    }

    /// Every kind resolves to a provider whose compose fragment builds cleanly,
    /// carries a healthcheck, and reports the expected primary container port.
    #[test]
    fn every_kind_builds_a_compose_service() {
        let cases = [
            (ServiceKind::Postgres, 5432u16),
            (ServiceKind::Kafka, 9093),
            (ServiceKind::Redis, 6379),
            (ServiceKind::Elastic, 9200),
            (ServiceKind::Mysql, 3306),
            (ServiceKind::Mongodb, 27017),
            (ServiceKind::Cockroach, 26257),
            (ServiceKind::Opensearch, 9200),
            (ServiceKind::Mssql, 1433),
            (ServiceKind::Rabbitmq, 5672),
            (ServiceKind::Clickhouse, 8123),
            (ServiceKind::Cassandra, 9042),
        ];
        for (kind, port) in cases {
            let provider = provider_for(kind).expect("provider exists");
            let assigned = if provider.needs_fixed_host_port() { Some(50000) } else { None };
            let svc = provider
                .compose_service(&decl(kind), assigned)
                .unwrap_or_else(|e| panic!("{kind:?} compose failed: {e}"));
            assert_eq!(svc.kind, kind, "{kind:?} reports its own kind");
            assert_eq!(svc.container_port, port, "{kind:?} primary port");
            assert!(svc.has_healthcheck, "{kind:?} declares a healthcheck");
            // The body must be a mapping with an image, i.e. it serialized.
            let body = svc.body.as_mapping().expect("body is a mapping");
            assert!(body.contains_key(serde_yaml::Value::from("image")), "{kind:?} has image");
        }
    }
}
