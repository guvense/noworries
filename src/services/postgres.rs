//! Postgres service provider.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use serde::Serialize;

use super::{ComposeService, Healthcheck, ServiceProvider, PG_DB, PG_PASSWORD, PG_USER};
use crate::spec::{ServiceDecl, ServiceKind};

pub struct Postgres;

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

/// JDBC URL for a resolved Postgres endpoint (used in reporting).
pub fn postgres_jdbc(host_port: u16) -> String {
    format!("jdbc:postgresql://127.0.0.1:{host_port}/{PG_DB}")
}
