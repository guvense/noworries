//! ClickHouse service provider.
//!
//! ClickHouse exposes an HTTP interface on 8123 (and its native protocol on
//! 9000). The primary endpoint here is HTTP, because a later live-check phase
//! can run `SELECT`/schema queries over it with the existing `ureq` client — no
//! native ClickHouse crate needed. The image provisions the user/db from the
//! `CLICKHOUSE_*` env vars; readiness is confirmed with the bundled client.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use serde::Serialize;

use super::{ComposeService, Healthcheck, ServiceProvider};
use crate::spec::{ServiceDecl, ServiceKind};

/// Baked-in credentials + database for the ephemeral ClickHouse container.
pub const CLICKHOUSE_USER: &str = "noworries";
pub const CLICKHOUSE_PASSWORD: &str = "noworries";
pub const CLICKHOUSE_DB: &str = "noworries";

pub struct Clickhouse;

#[derive(Serialize)]
struct ClickhouseBody {
    image: String,
    environment: BTreeMap<String, String>,
    ports: Vec<String>,
    networks: Vec<String>,
    healthcheck: Healthcheck,
}

impl ServiceProvider for Clickhouse {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Clickhouse
    }

    fn compose_service(&self, decl: &ServiceDecl, _assigned: Option<u16>) -> Result<ComposeService> {
        let mut env = BTreeMap::new();
        env.insert("CLICKHOUSE_USER".into(), CLICKHOUSE_USER.into());
        env.insert("CLICKHOUSE_PASSWORD".into(), CLICKHOUSE_PASSWORD.into());
        env.insert("CLICKHOUSE_DB".into(), CLICKHOUSE_DB.into());
        // Let the provisioned user run DDL/DML so tests can create tables.
        env.insert("CLICKHOUSE_DEFAULT_ACCESS_MANAGEMENT".into(), "1".into());

        let body = ClickhouseBody {
            image: decl.image.clone(),
            environment: env,
            // 8123 HTTP is the primary endpoint; 9000 is the native protocol.
            ports: vec!["8123".to_string(), "9000".to_string()],
            networks: vec!["noworries".to_string()],
            healthcheck: Healthcheck {
                test: vec![
                    "CMD-SHELL".to_string(),
                    format!(
                        "clickhouse-client --user {CLICKHOUSE_USER} --password {CLICKHOUSE_PASSWORD} -q 'SELECT 1' || exit 1"
                    ),
                ],
                interval: "3s".to_string(),
                timeout: "5s".to_string(),
                retries: 30,
                start_period: "8s".to_string(),
            },
        };

        Ok(ComposeService {
            name: "clickhouse".to_string(),
            kind: ServiceKind::Clickhouse,
            container_port: 8123,
            has_healthcheck: true,
            host_port: None,
            body: serde_yaml::to_value(body).map_err(|e| anyhow!(e))?,
        })
    }
}
