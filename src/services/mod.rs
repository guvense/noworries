//! Service providers. Each infra dependency (Postgres, Kafka, Redis, ...) is a
//! [`ServiceProvider`] living in its own module under `services/`. This module
//! owns only the interface, the shared types, and the registry — adding a new
//! service is a new file + one line in [`provider_for`], nothing else.

use anyhow::Result;
use serde::Serialize;

use crate::spec::{ServiceDecl, ServiceKind};

pub mod elastic;
pub mod kafka;
pub mod mongodb;
pub mod mysql;
pub mod postgres;
pub mod redis;

pub use elastic::Elastic;
pub use kafka::Kafka;
pub use mongodb::Mongodb;
pub use mysql::Mysql;
pub use postgres::{postgres_jdbc, Postgres};
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
    }
}
