//! OpenSearch service provider — single-node, security plugin disabled.
//!
//! OpenSearch is Elasticsearch-compatible on the HTTP/query surface, so apps
//! reach it with an Elasticsearch client at the same `_cluster/health` endpoint.
//! The container config differs from [`super::Elastic`] though: the setting keys
//! are OpenSearch's own (`DISABLE_SECURITY_PLUGIN`, `OPENSEARCH_JAVA_OPTS`),
//! which is why this is a separate provider rather than an image alias.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use serde::Serialize;

use super::{ComposeService, Healthcheck, ServiceProvider};
use crate::spec::{ServiceDecl, ServiceKind};

pub struct Opensearch;

#[derive(Serialize)]
struct OpensearchBody {
    image: String,
    environment: BTreeMap<String, String>,
    ports: Vec<String>,
    networks: Vec<String>,
    healthcheck: Healthcheck,
}

impl ServiceProvider for Opensearch {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Opensearch
    }

    fn compose_service(&self, decl: &ServiceDecl, _assigned: Option<u16>) -> Result<ComposeService> {
        let mut env = BTreeMap::new();
        env.insert("discovery.type".into(), "single-node".into());
        // Turn off the security plugin so the app + runner talk plain HTTP with
        // no credentials, and skip the demo-config bootstrap that would re-enable it.
        env.insert("DISABLE_SECURITY_PLUGIN".into(), "true".into());
        env.insert("DISABLE_INSTALL_DEMO_CONFIG".into(), "true".into());
        env.insert("OPENSEARCH_JAVA_OPTS".into(), "-Xms512m -Xmx512m".into());

        let body = OpensearchBody {
            image: decl.image.clone(),
            environment: env,
            ports: vec!["9200".to_string()],
            networks: vec!["noworries".to_string()],
            healthcheck: Healthcheck {
                test: vec![
                    "CMD-SHELL".to_string(),
                    "curl -s -f http://localhost:9200/_cluster/health || exit 1".to_string(),
                ],
                interval: "5s".to_string(),
                timeout: "5s".to_string(),
                retries: 30,
                start_period: "20s".to_string(),
            },
        };

        Ok(ComposeService {
            name: "opensearch".to_string(),
            kind: ServiceKind::Opensearch,
            container_port: 9200,
            has_healthcheck: true,
            host_port: None,
            body: serde_yaml::to_value(body).map_err(|e| anyhow!(e))?,
        })
    }
}
