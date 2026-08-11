//! Kafka service provider — single-node KRaft (no ZooKeeper).
//!
//! Because a Kafka client must be given an address the broker *advertises*, and
//! the app under test runs on the host, the external listener is advertised at
//! `localhost:<fixed host port>`. That host port is chosen at generation time,
//! which is why this provider returns `true` from `needs_fixed_host_port()`.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use serde::Serialize;

use super::{ComposeService, Healthcheck, ServiceProvider};
use crate::spec::{ServiceDecl, ServiceKind};

pub struct Kafka;

const INTERNAL_PORT: u16 = 9092;
const EXTERNAL_PORT: u16 = 9093;
const CONTROLLER_PORT: u16 = 9091;

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
            format!("1@localhost:{CONTROLLER_PORT}"),
        );
        env.insert(
            "KAFKA_LISTENERS".into(),
            format!("CONTROLLER://:{CONTROLLER_PORT},INTERNAL://:{INTERNAL_PORT},EXTERNAL://:{EXTERNAL_PORT}"),
        );
        // INTERNAL is reachable inside the compose network as "kafka"; EXTERNAL
        // is reachable from the host at the fixed mapped port.
        env.insert(
            "KAFKA_ADVERTISED_LISTENERS".into(),
            format!("INTERNAL://kafka:{INTERNAL_PORT},EXTERNAL://localhost:{host_port}"),
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
            ports: vec![format!("{host_port}:{EXTERNAL_PORT}")],
            networks: vec!["noworries".into()],
            healthcheck: Healthcheck {
                test: vec![
                    "CMD-SHELL".into(),
                    format!("/opt/kafka/bin/kafka-topics.sh --bootstrap-server localhost:{INTERNAL_PORT} --list"),
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
            container_port: EXTERNAL_PORT,
            has_healthcheck: true,
            host_port: Some(host_port),
            body: serde_yaml::to_value(body).map_err(|e| anyhow!(e))?,
        })
    }
}
