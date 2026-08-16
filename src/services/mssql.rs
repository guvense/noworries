//! Microsoft SQL Server service provider.
//!
//! The official `mcr.microsoft.com/mssql/server` image needs `ACCEPT_EULA=Y`
//! and a strong SA password (8+ chars incl. upper/lower/digit/symbol) or it
//! refuses to start. Readiness is confirmed with the bundled `sqlcmd` from
//! mssql-tools18, which defaults to encrypted connections — `-C` trusts the
//! self-signed server cert so the probe works without provisioning one.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use serde::Serialize;

use super::{ComposeService, Healthcheck, ServiceProvider};
use crate::spec::{ServiceDecl, ServiceKind};

/// Baked-in SA password for the ephemeral SQL Server container. Meets the
/// engine's complexity policy; only ever reachable on localhost during a run.
pub const MSSQL_SA_PASSWORD: &str = "Noworries!Pass1";

pub struct Mssql;

#[derive(Serialize)]
struct MssqlBody {
    image: String,
    environment: BTreeMap<String, String>,
    ports: Vec<String>,
    networks: Vec<String>,
    healthcheck: Healthcheck,
}

impl ServiceProvider for Mssql {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Mssql
    }

    fn compose_service(&self, decl: &ServiceDecl, _assigned: Option<u16>) -> Result<ComposeService> {
        let mut env = BTreeMap::new();
        env.insert("ACCEPT_EULA".into(), "Y".into());
        env.insert("MSSQL_SA_PASSWORD".into(), MSSQL_SA_PASSWORD.into());
        env.insert("MSSQL_PID".into(), "Developer".into());

        let body = MssqlBody {
            image: decl.image.clone(),
            environment: env,
            ports: vec!["1433".to_string()],
            networks: vec!["noworries".to_string()],
            healthcheck: Healthcheck {
                test: vec![
                    "CMD-SHELL".to_string(),
                    format!(
                        "/opt/mssql-tools18/bin/sqlcmd -S localhost -U sa -P '{MSSQL_SA_PASSWORD}' -C -Q 'SELECT 1' || exit 1"
                    ),
                ],
                interval: "5s".to_string(),
                timeout: "5s".to_string(),
                retries: 30,
                start_period: "15s".to_string(),
            },
        };

        Ok(ComposeService {
            name: "mssql".to_string(),
            kind: ServiceKind::Mssql,
            container_port: 1433,
            has_healthcheck: true,
            host_port: None,
            body: serde_yaml::to_value(body).map_err(|e| anyhow!(e))?,
        })
    }
}
