//! Cassandra service provider (also serves ScyllaDB via image alias).
//!
//! Cassandra listens for CQL on 9042. ScyllaDB is wire-compatible with the same
//! protocol, so `scylla` maps onto this kind with the `scylladb/scylla` image
//! (see `parse_service_decl`) — same as `mariadb` reuses the MySQL kind.
//! Cassandra is slow to bootstrap, so the healthcheck (`cqlsh`) gets a long
//! start period and generous retries.

use anyhow::{anyhow, Result};
use serde::Serialize;

use super::{ComposeService, Healthcheck, ServiceProvider};
use crate::spec::{ServiceDecl, ServiceKind};

pub struct Cassandra;

#[derive(Serialize)]
struct CassandraBody {
    image: String,
    ports: Vec<String>,
    networks: Vec<String>,
    healthcheck: Healthcheck,
}

impl ServiceProvider for Cassandra {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Cassandra
    }

    fn compose_service(&self, decl: &ServiceDecl, _assigned: Option<u16>) -> Result<ComposeService> {
        let body = CassandraBody {
            image: decl.image.clone(),
            ports: vec!["9042".to_string()],
            networks: vec!["noworries".to_string()],
            healthcheck: Healthcheck {
                test: vec![
                    "CMD-SHELL".to_string(),
                    "cqlsh -e 'describe keyspaces' || exit 1".to_string(),
                ],
                interval: "5s".to_string(),
                timeout: "10s".to_string(),
                retries: 40,
                start_period: "60s".to_string(),
            },
        };

        Ok(ComposeService {
            name: "cassandra".to_string(),
            kind: ServiceKind::Cassandra,
            container_port: 9042,
            has_healthcheck: true,
            host_port: None,
            body: serde_yaml::to_value(body).map_err(|e| anyhow!(e))?,
        })
    }
}
