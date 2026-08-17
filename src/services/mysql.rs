//! MySQL service provider — also serves the wire-compatible `mariadb` alias.
//!
//! The two differ in one place that matters: **the healthcheck command**.
//! MariaDB 11 dropped the `mysql*` compatibility symlinks, so `mysqladmin` is
//! simply absent from `mariadb:11*` images — the container then never reports
//! healthy and the run times out with the service stuck in `running/unhealthy`.
//! The probe below tries `mariadb-admin` first and falls back to `mysqladmin`,
//! which covers MariaDB 11+, MariaDB 10.x, and MySQL itself.

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
                test: vec!["CMD-SHELL".to_string(), healthcheck_cmd(&decl.image)],
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

/// The readiness probe for `image`.
///
/// MariaDB images ship `mariadb-admin`; MariaDB 11 no longer ships the
/// `mysqladmin` symlink, and MySQL images only ship `mysqladmin`. Trying both
/// keeps one provider serving both engines — the alias promise is "same
/// provider, same checks, different image", and the healthcheck is the one
/// place where the image genuinely differs.
fn healthcheck_cmd(image: &str) -> String {
    if is_mariadb(image) {
        "mariadb-admin ping -h 127.0.0.1 --silent \
         || mysqladmin ping -h 127.0.0.1 --silent \
         || exit 1"
            .to_string()
    } else {
        "mysqladmin ping -h 127.0.0.1 --silent || exit 1".to_string()
    }
}

/// True for a MariaDB image reference (`mariadb`, `mariadb:11.4`,
/// `docker.io/library/mariadb:latest`, …).
fn is_mariadb(image: &str) -> bool {
    let repo = image.split(':').next().unwrap_or(image);
    repo.rsplit('/').next().unwrap_or(repo).eq_ignore_ascii_case("mariadb")
}

pub fn mysql_jdbc(host_port: u16) -> String {
    format!("jdbc:mysql://127.0.0.1:{host_port}/{MYSQL_DB}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mariadb_images_get_a_mariadb_probe() {
        // MariaDB 11 ships no `mysqladmin`, so a mysql-only probe leaves the
        // container in running/unhealthy forever.
        for img in ["mariadb", "mariadb:11", "mariadb:11.4", "docker.io/library/mariadb:latest"] {
            let cmd = healthcheck_cmd(img);
            assert!(cmd.starts_with("mariadb-admin ping"), "{img}: {cmd}");
            assert!(cmd.contains("|| mysqladmin ping"), "{img} keeps the 10.x fallback");
        }
    }

    #[test]
    fn mysql_images_keep_the_mysql_probe() {
        for img in ["mysql", "mysql:8.4", "mysql:8.0-debian"] {
            assert_eq!(
                healthcheck_cmd(img),
                "mysqladmin ping -h 127.0.0.1 --silent || exit 1"
            );
        }
    }

    #[test]
    fn is_mariadb_is_not_fooled_by_similar_names() {
        assert!(!is_mariadb("mysql:8.4"));
        assert!(!is_mariadb("my-mariadb-proxy:1"));
        assert!(is_mariadb("MariaDB:11"));
    }

    #[test]
    fn compose_service_uses_the_image_specific_probe() {
        let decl = ServiceDecl {
            kind: ServiceKind::Mysql,
            image: "mariadb:11.4".into(),
            tag: Some("11.4".into()),
            raw: "mariadb:11.4".into(),
        };
        let svc = Mysql.compose_service(&decl, None).unwrap();
        let yaml = serde_yaml::to_string(&svc.body).unwrap();
        assert!(yaml.contains("mariadb-admin ping"), "{yaml}");
    }
}
