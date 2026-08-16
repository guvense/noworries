//! RabbitMQ service provider.
//!
//! Uses the `-management` image so the HTTP management API (15672) is available
//! alongside AMQP (5672) — a later live-check phase can assert queue/exchange
//! state over that REST API with the existing `ureq` client, no AMQP crate. For
//! now the provider boots the broker, wires `amqp://` env for the app, and
//! confirms readiness with `rabbitmq-diagnostics ping` (present in every image).

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use serde::Serialize;

use super::{ComposeService, Healthcheck, ServiceProvider};
use crate::spec::{ServiceDecl, ServiceKind};

/// Baked-in credentials for the ephemeral RabbitMQ container.
pub const RABBITMQ_USER: &str = "noworries";
pub const RABBITMQ_PASSWORD: &str = "noworries";

pub struct Rabbitmq;

#[derive(Serialize)]
struct RabbitmqBody {
    image: String,
    environment: BTreeMap<String, String>,
    ports: Vec<String>,
    networks: Vec<String>,
    healthcheck: Healthcheck,
}

/// RabbitMQ management API container port (HTTP), resolved alongside AMQP so
/// live checks can query queue/message state over REST.
pub const RABBITMQ_MGMT_PORT: u16 = 15672;

impl ServiceProvider for Rabbitmq {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Rabbitmq
    }

    fn aux_ports(&self) -> &'static [u16] {
        &[RABBITMQ_MGMT_PORT]
    }

    fn compose_service(&self, decl: &ServiceDecl, _assigned: Option<u16>) -> Result<ComposeService> {
        let mut env = BTreeMap::new();
        env.insert("RABBITMQ_DEFAULT_USER".into(), RABBITMQ_USER.into());
        env.insert("RABBITMQ_DEFAULT_PASS".into(), RABBITMQ_PASSWORD.into());

        let body = RabbitmqBody {
            image: decl.image.clone(),
            environment: env,
            // 5672 is the primary (AMQP) endpoint; 15672 exposes the management
            // API for later HTTP-based assertions.
            ports: vec!["5672".to_string(), "15672".to_string()],
            networks: vec!["noworries".to_string()],
            healthcheck: Healthcheck {
                test: vec![
                    "CMD-SHELL".to_string(),
                    "rabbitmq-diagnostics -q ping || exit 1".to_string(),
                ],
                interval: "5s".to_string(),
                timeout: "5s".to_string(),
                retries: 30,
                start_period: "15s".to_string(),
            },
        };

        Ok(ComposeService {
            name: "rabbitmq".to_string(),
            kind: ServiceKind::Rabbitmq,
            container_port: 5672,
            has_healthcheck: true,
            host_port: None,
            body: serde_yaml::to_value(body).map_err(|e| anyhow!(e))?,
        })
    }
}
