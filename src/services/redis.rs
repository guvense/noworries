//! Redis service provider — plain single-node, random host port.

use anyhow::{anyhow, Result};
use serde::Serialize;

use super::{ComposeService, Healthcheck, ServiceProvider};
use crate::spec::{ServiceDecl, ServiceKind};

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
