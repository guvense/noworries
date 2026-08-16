//! Laravel framework adapter (PHP).
//!
//! Detects a Laravel app by its `artisan` console entry point. Laravel has no
//! default health endpoint, so readiness is TCP-based (`health: "none"`) unless
//! the spec sets one. `artisan serve` reads the port from `--port`, which we
//! feed from `PORT`.
//!
//! Laravel's config reads discrete `DB_*` / `REDIS_*` env vars (not a single
//! connection URL), so on top of the shared [`conventional_env`] wiring (which
//! also provides `DATABASE_URL` and the `NOWORRIES_*` fallbacks) this adapter
//! emits Laravel's native `DB_CONNECTION` / `DB_HOST` / `DB_PORT` /
//! `DB_DATABASE` / `DB_USERNAME` / `DB_PASSWORD` and `REDIS_HOST` / `REDIS_PORT`.

use std::collections::BTreeMap;
use std::path::Path;

use super::{conventional_env, Framework};
use crate::lifecycle::ServiceEndpoint;
use crate::services::mssql::MSSQL_SA_PASSWORD;
use crate::services::mysql::{MYSQL_DB, MYSQL_PASSWORD, MYSQL_USER};
use crate::services::{PG_DB, PG_PASSWORD, PG_USER};
use crate::spec::ServiceKind;

pub struct Laravel;

impl Framework for Laravel {
    fn name(&self) -> &'static str {
        "laravel"
    }

    fn detect(&self, dir: &Path) -> bool {
        // `artisan` is unique to Laravel (and Lumen) projects.
        dir.join("artisan").exists()
    }

    fn default_start_command(&self, _dir: &Path) -> Option<String> {
        Some("php artisan serve --host=0.0.0.0 --port=${PORT:-8000}".to_string())
    }

    fn default_health_path(&self) -> &'static str {
        "none"
    }

    fn default_port_env(&self) -> &'static str {
        "PORT"
    }

    fn env_wiring(&self, endpoints: &[ServiceEndpoint]) -> BTreeMap<String, String> {
        // Base: DATABASE_URL / REDIS_URL / NOWORRIES_* etc. (covers every kind).
        let mut m = conventional_env(endpoints);
        let host = "127.0.0.1";
        for ep in endpoints {
            let p = ep.host_port.to_string();
            match ep.kind {
                ServiceKind::Postgres => {
                    m.insert("DB_CONNECTION".to_string(), "pgsql".to_string());
                    m.insert("DB_HOST".to_string(), host.to_string());
                    m.insert("DB_PORT".to_string(), p);
                    m.insert("DB_DATABASE".to_string(), PG_DB.to_string());
                    m.insert("DB_USERNAME".to_string(), PG_USER.to_string());
                    m.insert("DB_PASSWORD".to_string(), PG_PASSWORD.to_string());
                }
                ServiceKind::Mysql => {
                    m.insert("DB_CONNECTION".to_string(), "mysql".to_string());
                    m.insert("DB_HOST".to_string(), host.to_string());
                    m.insert("DB_PORT".to_string(), p);
                    m.insert("DB_DATABASE".to_string(), MYSQL_DB.to_string());
                    m.insert("DB_USERNAME".to_string(), MYSQL_USER.to_string());
                    m.insert("DB_PASSWORD".to_string(), MYSQL_PASSWORD.to_string());
                }
                ServiceKind::Redis => {
                    m.insert("REDIS_HOST".to_string(), host.to_string());
                    m.insert("REDIS_PORT".to_string(), p);
                }
                ServiceKind::Mongodb => {
                    // mongodb/laravel-mongodb reads a URI from DB_URI.
                    m.insert("DB_URI".to_string(), format!("mongodb://{host}:{p}"));
                }
                ServiceKind::Cockroach => {
                    // Postgres-wire; Laravel's pgsql driver, root/defaultdb.
                    m.insert("DB_CONNECTION".to_string(), "pgsql".to_string());
                    m.insert("DB_HOST".to_string(), host.to_string());
                    m.insert("DB_PORT".to_string(), p);
                    m.insert("DB_DATABASE".to_string(), "defaultdb".to_string());
                    m.insert("DB_USERNAME".to_string(), "root".to_string());
                    m.insert("DB_PASSWORD".to_string(), "".to_string());
                }
                ServiceKind::Mssql => {
                    m.insert("DB_CONNECTION".to_string(), "sqlsrv".to_string());
                    m.insert("DB_HOST".to_string(), host.to_string());
                    m.insert("DB_PORT".to_string(), p);
                    m.insert("DB_USERNAME".to_string(), "sa".to_string());
                    m.insert("DB_PASSWORD".to_string(), MSSQL_SA_PASSWORD.to_string());
                }
                // Kafka / Elastic / OpenSearch / RabbitMQ / ClickHouse /
                // Cassandra have no first-class Laravel `DB_*` keys — the
                // conventional_env vars (KAFKA_BROKERS, ELASTICSEARCH_URL,
                // RABBITMQ_URL, CLICKHOUSE_URL, ...) and the NOWORRIES_*
                // fallbacks already cover them.
                ServiceKind::Kafka
                | ServiceKind::Elastic
                | ServiceKind::Opensearch
                | ServiceKind::Rabbitmq
                | ServiceKind::Clickhouse
                | ServiceKind::Cassandra => {}
            }
        }
        m
    }
}
