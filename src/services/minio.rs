//! S3-compatible object storage service provider (MinIO).
//!
//! File-upload flows are everywhere and were previously unverifiable: the app
//! would PUT an object and noworries had no way to look. MinIO answers the S3
//! API on 9000, so the `s3:` check reads objects back with ordinary signed
//! requests — the same ones that work against real S3.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use serde::Serialize;

use super::{ComposeService, Healthcheck, ServiceProvider};
use crate::spec::{ServiceDecl, ServiceKind};

/// Baked-in credentials for the ephemeral MinIO container. MinIO requires at
/// least 8 characters, which `noworries` satisfies.
pub const MINIO_USER: &str = "noworries";
pub const MINIO_PASSWORD: &str = "noworries";
/// The region MinIO reports; requests must be signed for the same one.
pub const MINIO_REGION: &str = "us-east-1";

pub struct Minio;

#[derive(Serialize)]
struct MinioBody {
    image: String,
    command: String,
    environment: BTreeMap<String, String>,
    ports: Vec<String>,
    networks: Vec<String>,
    healthcheck: Healthcheck,
}

impl ServiceProvider for Minio {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Minio
    }

    fn compose_service(&self, decl: &ServiceDecl, _assigned: Option<u16>) -> Result<ComposeService> {
        let mut env = BTreeMap::new();
        env.insert("MINIO_ROOT_USER".into(), MINIO_USER.into());
        env.insert("MINIO_ROOT_PASSWORD".into(), MINIO_PASSWORD.into());
        env.insert("MINIO_REGION".into(), MINIO_REGION.into());

        let body = MinioBody {
            image: decl.image.clone(),
            // The image's entrypoint is `minio`, but it does nothing without a
            // subcommand — `server /data` is what actually starts the API.
            command: "server /data".to_string(),
            environment: env,
            ports: vec!["9000".to_string()],
            networks: vec!["noworries".to_string()],
            healthcheck: Healthcheck {
                // MinIO's own client, which ships in the image, in exec form —
                // the image has no shell, so a `CMD-SHELL` probe would never
                // pass (and the container would sit unhealthy forever).
                test: vec![
                    "CMD".to_string(),
                    "mc".to_string(),
                    "ready".to_string(),
                    "local".to_string(),
                ],
                interval: "3s".to_string(),
                timeout: "5s".to_string(),
                retries: 20,
                start_period: "3s".to_string(),
            },
        };

        Ok(ComposeService {
            name: "minio".to_string(),
            kind: ServiceKind::Minio,
            container_port: 9000,
            has_healthcheck: true,
            host_port: None,
            body: serde_yaml::to_value(body).map_err(|e| anyhow!(e))?,
        })
    }
}
