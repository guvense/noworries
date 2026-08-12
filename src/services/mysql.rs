//! MySQL service provider.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use serde::Serialize;

use super::{ComposeService, Healthcheck, ServiceProvider};
use crate::spec::{ServiceDecl, ServiceKind};

/// Baked-in credentials for the ephemeral MySQL container.
pub const MYSQL_USER: &str = "noworries";
pub const MYSQL_PASSWORD: &str = "noworries";
pub const MYSQL_DB: &str = "noworries";

pub struct Mysql;

#[derive(Serialize)]
struct MysqlBody {
    image: String,
    environment: BTreeMap<String, String>,
    ports: Vec<String>,
    networks: Vec<String>,
    healthcheck: Healthcheck,
}

impl ServiceProvider for Mysql {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Mysql
    }

    fn compose_service(&self, decl: &ServiceDecl, _assigned: Option<u16>) -> Result<ComposeService> {
        let mut env = BTreeMap::new();
        env.insert("MYSQL_ROOT_PASSWORD".into(), MYSQL_PASSWORD.into());
        env.insert("MYSQL_DATABASE".into(), MYSQL_DB.into());
        env.insert("MYSQL_USER".into(), MYSQL_USER.into());
        env.insert("MYSQL_PASSWORD".into(), MYSQL_PASSWORD.into());

        let body = MysqlBody {
            image: decl.image.clone(),
            environment: env,
            ports: vec!["3306".to_string()],
            networks: vec!["noworries".to_string()],
            healthcheck: Healthcheck {
                test: vec![
                    "CMD-SHELL".to_string(),
                    "mysqladmin ping -h 127.0.0.1 --silent || exit 1".to_string(),
                ],
                interval: "3s".to_string(),
                timeout: "5s".to_string(),
                retries: 40,
                start_period: "10s".to_string(),
            },
        };

        Ok(ComposeService {
            name: "mysql".to_string(),
            kind: ServiceKind::Mysql,
            container_port: 3306,
            has_healthcheck: true,
            host_port: None,
            body: serde_yaml::to_value(body).map_err(|e| anyhow!(e))?,
        })
    }
}

pub fn mysql_jdbc(host_port: u16) -> String {
    format!("jdbc:mysql://127.0.0.1:{host_port}/{MYSQL_DB}")
}
