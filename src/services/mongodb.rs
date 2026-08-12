//! MongoDB service provider — single node, no auth (test only).

use anyhow::{anyhow, Result};
use serde::Serialize;

use super::{ComposeService, Healthcheck, ServiceProvider};
use crate::spec::{ServiceDecl, ServiceKind};

pub struct Mongodb;

#[derive(Serialize)]
struct MongoBody {
    image: String,
    ports: Vec<String>,
    networks: Vec<String>,
    healthcheck: Healthcheck,
}

impl ServiceProvider for Mongodb {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Mongodb
    }

    fn compose_service(&self, decl: &ServiceDecl, _assigned: Option<u16>) -> Result<ComposeService> {
        let body = MongoBody {
            image: decl.image.clone(),
            ports: vec!["27017".to_string()],
            networks: vec!["noworries".to_string()],
            healthcheck: Healthcheck {
                test: vec![
                    "CMD-SHELL".to_string(),
                    "mongosh --quiet --eval 'db.runCommand({ ping: 1 })' || exit 1".to_string(),
                ],
                interval: "3s".to_string(),
                timeout: "5s".to_string(),
                retries: 40,
                start_period: "5s".to_string(),
            },
        };
        Ok(ComposeService {
            name: "mongodb".to_string(),
            kind: ServiceKind::Mongodb,
            container_port: 27017,
            has_healthcheck: true,
            host_port: None,
            body: serde_yaml::to_value(body).map_err(|e| anyhow!(e))?,
        })
    }
}
