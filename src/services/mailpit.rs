//! SMTP sink service provider (Mailpit).
//!
//! Apps send mail; noworries has to be able to *read* it back. Mailpit fills
//! both roles in one container: an SMTP server on 1025 that accepts anything,
//! and a JSON API on 8025 that the `email:` check queries. Nothing is delivered
//! anywhere, so a password-reset or order-confirmation flow can be verified
//! without a real mailbox.
//!
//! Mailpit rather than MailHog, which is the better-known name: MailHog has had
//! no release since 2020 and publishes no arm64 image, so on Apple Silicon it
//! runs under emulation. Mailpit is maintained, multi-arch, and its API is a
//! superset of what this check needs.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use serde::Serialize;

use super::{ComposeService, Healthcheck, ServiceProvider};
use crate::spec::{ServiceDecl, ServiceKind};

pub struct Mailpit;

/// HTTP API port, resolved alongside SMTP so the `email:` check can read the
/// mailbox the app just wrote to.
pub const MAILPIT_API_PORT: u16 = 8025;

#[derive(Serialize)]
struct MailpitBody {
    image: String,
    environment: BTreeMap<String, String>,
    ports: Vec<String>,
    networks: Vec<String>,
    healthcheck: Healthcheck,
}

impl ServiceProvider for Mailpit {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Smtp
    }

    fn aux_ports(&self) -> &'static [u16] {
        &[MAILPIT_API_PORT]
    }

    fn compose_service(&self, decl: &ServiceDecl, _assigned: Option<u16>) -> Result<ComposeService> {
        let mut env = BTreeMap::new();
        // Keep the mailbox in memory: every run starts empty, which is what an
        // assertion like "exactly one confirmation was sent" depends on.
        env.insert("MP_MAX_MESSAGES".into(), "5000".into());
        env.insert("MP_SMTP_AUTH_ACCEPT_ANY".into(), "1".into());
        env.insert("MP_SMTP_AUTH_ALLOW_INSECURE".into(), "1".into());

        let body = MailpitBody {
            image: decl.image.clone(),
            environment: env,
            ports: vec!["1025".to_string(), MAILPIT_API_PORT.to_string()],
            networks: vec!["noworries".to_string()],
            healthcheck: Healthcheck {
                // Mailpit's own readiness subcommand — exec form, so it needs
                // no shell, curl or wget in the image.
                test: vec![
                    "CMD".to_string(),
                    "/mailpit".to_string(),
                    "readyz".to_string(),
                ],
                interval: "3s".to_string(),
                timeout: "5s".to_string(),
                retries: 20,
                start_period: "3s".to_string(),
            },
        };

        Ok(ComposeService {
            name: "smtp".to_string(),
            kind: ServiceKind::Smtp,
            container_port: 1025,
            has_healthcheck: true,
            host_port: None,
            body: serde_yaml::to_value(body).map_err(|e| anyhow!(e))?,
        })
    }
}
