//! CockroachDB service provider.
//!
//! CockroachDB speaks the Postgres wire protocol, so apps talk to it with their
//! usual Postgres client — but the *container* is nothing like the `postgres`
//! image: it needs `start-single-node --insecure` to come up without a cluster,
//! listens on 26257 (not 5432), and authenticates as `root` with no password
//! against the `defaultdb` database. That's why it gets its own provider rather
//! than reusing [`super::Postgres`] as a plain image alias.

use anyhow::{anyhow, Result};
use serde::Serialize;

use super::{ComposeService, Healthcheck, ServiceProvider};
use crate::spec::{ServiceDecl, ServiceKind};

pub struct Cockroach;

#[derive(Serialize)]
struct CockroachBody {
    image: String,
    command: String,
    ports: Vec<String>,
    networks: Vec<String>,
    healthcheck: Healthcheck,
}

impl ServiceProvider for Cockroach {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Cockroach
    }

    fn compose_service(&self, decl: &ServiceDecl, _assigned: Option<u16>) -> Result<ComposeService> {
        let body = CockroachBody {
            image: decl.image.clone(),
            // Single-node dev mode with auth disabled so the app + runner reach
            // it as `root` over plain TCP, no certs.
            command: "start-single-node --insecure".to_string(),
            ports: vec!["26257".to_string()],
            networks: vec!["noworries".to_string()],
            healthcheck: Healthcheck {
                test: vec![
                    "CMD-SHELL".to_string(),
                    "cockroach sql --insecure --host=localhost:26257 -e 'SELECT 1' || exit 1"
                        .to_string(),
                ],
                interval: "3s".to_string(),
                timeout: "5s".to_string(),
                retries: 30,
                start_period: "8s".to_string(),
            },
        };

        Ok(ComposeService {
            name: "cockroach".to_string(),
            kind: ServiceKind::Cockroach,
            container_port: 26257,
            has_healthcheck: true,
            host_port: None,
            body: serde_yaml::to_value(body).map_err(|e| anyhow!(e))?,
        })
    }
}
