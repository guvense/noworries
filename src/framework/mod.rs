//! Framework adapters. A [`Framework`] knows how to detect its kind of project,
//! how to start it, where its health endpoint lives, and how to translate
//! resolved service endpoints into environment variables. This module owns only
//! the interface, the registry, and shared helpers — each framework lives in
//! its own file under `framework/`. Adding one is a new file + one line in
//! [`registry`].

use std::collections::BTreeMap;
use std::path::Path;

use crate::lifecycle::ServiceEndpoint;
use crate::services::clickhouse::{CLICKHOUSE_DB, CLICKHOUSE_PASSWORD, CLICKHOUSE_USER};
use crate::services::mssql::MSSQL_SA_PASSWORD;
use crate::services::mysql::{MYSQL_DB, MYSQL_PASSWORD, MYSQL_USER};
use crate::services::rabbitmq::{RABBITMQ_PASSWORD, RABBITMQ_USER};
use crate::services::{PG_DB, PG_PASSWORD, PG_USER};
use crate::spec::ServiceKind;

pub mod django;
pub mod dotnet;
pub mod go;
pub mod laravel;
pub mod node;
pub mod python;
pub mod rails;
pub mod spring_boot;

pub use django::Django;
pub use dotnet::DotNet;
pub use go::Go;
pub use laravel::Laravel;
pub use node::NodeJs;
pub use python::FastApi;
pub use rails::Rails;
pub use spring_boot::SpringBoot;

/// The interface every framework implements.
pub trait Framework {
    fn name(&self) -> &'static str;
    /// Does this framework appear to be used in `dir`?
    fn detect(&self, dir: &Path) -> bool;
    /// Best-guess command to start the app, if inferable from `dir`.
    fn default_start_command(&self, dir: &Path) -> Option<String>;
    /// Health endpoint polled until 2xx before checks run. Return `"none"` to
    /// skip HTTP probing and just wait for the port to accept TCP (frameworks
    /// with no conventional health endpoint).
    fn default_health_path(&self) -> &'static str;
    /// Env var the app reads for its HTTP port. Framework default; overridden by
    /// `app.port_env`.
    fn default_port_env(&self) -> &'static str {
        "SERVER_PORT"
    }
    /// Extra variables the framework needs before it will actually bind `port`,
    /// beyond [`Framework::default_port_env`]. ASP.NET Core is the case that
    /// forced this: `ASPNETCORE_HTTP_PORTS` alone loses to the `applicationUrl`
    /// in `Properties/launchSettings.json`, so the app kept listening on 5000
    /// while noworries probed the port it had assigned.
    fn port_env_extra(&self, _port: u16) -> BTreeMap<String, String> {
        BTreeMap::new()
    }
    /// Environment variables wiring the app to the resolved service endpoints.
    fn env_wiring(&self, endpoints: &[ServiceEndpoint]) -> BTreeMap<String, String>;
}

/// Registry / composition root: all known frameworks, in detection-priority
/// order. This is the single place that knows the concrete implementations.
/// Order matters: more specific manifests (pom.xml/go.mod) before broad ones so
/// e.g. a Spring project that happens to ship a package.json still detects as
/// Spring Boot.
pub fn registry() -> Vec<Box<dyn Framework>> {
    vec![
        Box::new(SpringBoot),
        Box::new(DotNet),
        Box::new(Go),
        Box::new(Rails),
        Box::new(Laravel),
        Box::new(Django),
        Box::new(FastApi),
        Box::new(NodeJs),
    ]
}

/// Resolve the framework: an explicit name wins, otherwise auto-detect.
pub fn detect(dir: &Path, forced: Option<&str>) -> Option<Box<dyn Framework>> {
    let regs = registry();
    match forced {
        Some(name) => regs.into_iter().find(|f| f.name().eq_ignore_ascii_case(name)),
        None => regs.into_iter().find(|f| f.detect(dir)),
    }
}

/// Framework-agnostic vars every framework receives in addition to its own.
/// Shared helper for framework implementations.
pub fn generic_env(endpoints: &[ServiceEndpoint]) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for ep in endpoints {
        let k = ep.kind.as_str().to_uppercase();
        m.insert(format!("NOWORRIES_{k}_HOST"), "127.0.0.1".to_string());
        m.insert(format!("NOWORRIES_{k}_PORT"), ep.host_port.to_string());
    }
    m
}

/// Convention-based wiring shared by the non-JVM frameworks (Node, FastAPI, Go).
/// These ecosystems have no single config standard, so noworries exports the
/// widely-used connection-string / broker-list env vars (`DATABASE_URL`,
/// `REDIS_URL`, `KAFKA_BROKERS`, `MONGODB_URI`, `ELASTICSEARCH_URL`, ...) plus
/// per-part Postgres/MySQL vars, on top of [`generic_env`]. Apps read whichever
/// their client library expects; the framework-agnostic `NOWORRIES_*` vars are
/// always present as a fallback.
pub fn conventional_env(endpoints: &[ServiceEndpoint]) -> BTreeMap<String, String> {
    let mut m = generic_env(endpoints);
    let host = "127.0.0.1";
    for ep in endpoints {
        let p = ep.host_port;
        match ep.kind {
            ServiceKind::Postgres => {
                m.insert(
                    "DATABASE_URL".to_string(),
                    format!("postgres://{PG_USER}:{PG_PASSWORD}@{host}:{p}/{PG_DB}"),
                );
                m.insert("PGHOST".to_string(), host.to_string());
                m.insert("PGPORT".to_string(), p.to_string());
                m.insert("PGUSER".to_string(), PG_USER.to_string());
                m.insert("PGPASSWORD".to_string(), PG_PASSWORD.to_string());
                m.insert("PGDATABASE".to_string(), PG_DB.to_string());
            }
            ServiceKind::Mysql => {
                m.insert(
                    "DATABASE_URL".to_string(),
                    format!("mysql://{MYSQL_USER}:{MYSQL_PASSWORD}@{host}:{p}/{MYSQL_DB}"),
                );
                m.insert("MYSQL_HOST".to_string(), host.to_string());
                m.insert("MYSQL_PORT".to_string(), p.to_string());
                m.insert("MYSQL_USER".to_string(), MYSQL_USER.to_string());
                m.insert("MYSQL_PASSWORD".to_string(), MYSQL_PASSWORD.to_string());
                m.insert("MYSQL_DATABASE".to_string(), MYSQL_DB.to_string());
            }
            ServiceKind::Mongodb => {
                let uri = format!("mongodb://{host}:{p}");
                m.insert("MONGODB_URI".to_string(), uri.clone());
                m.insert("MONGO_URL".to_string(), uri);
            }
            ServiceKind::Redis => {
                m.insert("REDIS_URL".to_string(), format!("redis://{host}:{p}"));
                m.insert("REDIS_HOST".to_string(), host.to_string());
                m.insert("REDIS_PORT".to_string(), p.to_string());
            }
            ServiceKind::Kafka => {
                let brokers = format!("{host}:{p}");
                m.insert("KAFKA_BROKERS".to_string(), brokers.clone());
                m.insert("KAFKA_BOOTSTRAP_SERVERS".to_string(), brokers);
            }
            ServiceKind::Elastic => {
                let url = format!("http://{host}:{p}");
                m.insert("ELASTICSEARCH_URL".to_string(), url.clone());
                m.insert("ELASTIC_URL".to_string(), url);
            }
            ServiceKind::Cockroach => {
                // Postgres-wire, but auth is `root` with no password against
                // `defaultdb` in single-node insecure mode.
                m.insert(
                    "DATABASE_URL".to_string(),
                    format!("postgres://root@{host}:{p}/defaultdb?sslmode=disable"),
                );
                m.insert("PGHOST".to_string(), host.to_string());
                m.insert("PGPORT".to_string(), p.to_string());
                m.insert("PGUSER".to_string(), "root".to_string());
                m.insert("PGDATABASE".to_string(), "defaultdb".to_string());
            }
            ServiceKind::Opensearch => {
                let url = format!("http://{host}:{p}");
                m.insert("OPENSEARCH_URL".to_string(), url.clone());
                // OpenSearch clients often reuse the Elasticsearch env var.
                m.insert("ELASTICSEARCH_URL".to_string(), url);
            }
            ServiceKind::Mssql => {
                m.insert(
                    "DATABASE_URL".to_string(),
                    format!("sqlserver://sa:{MSSQL_SA_PASSWORD}@{host}:{p}"),
                );
                m.insert("MSSQL_HOST".to_string(), host.to_string());
                m.insert("MSSQL_PORT".to_string(), p.to_string());
                m.insert("MSSQL_USER".to_string(), "sa".to_string());
                m.insert("MSSQL_SA_PASSWORD".to_string(), MSSQL_SA_PASSWORD.to_string());
                // `MSSQL_PASSWORD` is what most drivers/docs reach for; the
                // container image's own name is MSSQL_SA_PASSWORD, so ship both.
                m.insert("MSSQL_PASSWORD".to_string(), MSSQL_SA_PASSWORD.to_string());
                // No database is created for you — SQL Server starts with the
                // system databases, so `master` is where an app lands unless it
                // creates its own.
                m.insert("MSSQL_DATABASE".to_string(), "master".to_string());
                m.insert(
                    "MSSQL_JDBC_URL".to_string(),
                    format!("jdbc:sqlserver://{host}:{p};encrypt=false;trustServerCertificate=true"),
                );
            }
            ServiceKind::Rabbitmq => {
                let url = format!("amqp://{RABBITMQ_USER}:{RABBITMQ_PASSWORD}@{host}:{p}");
                m.insert("RABBITMQ_URL".to_string(), url.clone());
                m.insert("AMQP_URL".to_string(), url);
            }
            ServiceKind::Clickhouse => {
                m.insert("CLICKHOUSE_URL".to_string(), format!("http://{host}:{p}"));
                // ClickHouse's HTTP interface ignores the user's default
                // database: without `?database=`, statements land in `default`
                // while the check looks in `noworries`. This DSN carries both
                // the credentials and the database, so it just works.
                m.insert(
                    "CLICKHOUSE_DSN".to_string(),
                    format!(
                        "http://{CLICKHOUSE_USER}:{CLICKHOUSE_PASSWORD}@{host}:{p}/?database={CLICKHOUSE_DB}"
                    ),
                );
                m.insert("CLICKHOUSE_HOST".to_string(), host.to_string());
                m.insert("CLICKHOUSE_PORT".to_string(), p.to_string());
                m.insert("CLICKHOUSE_USER".to_string(), CLICKHOUSE_USER.to_string());
                m.insert("CLICKHOUSE_PASSWORD".to_string(), CLICKHOUSE_PASSWORD.to_string());
                m.insert("CLICKHOUSE_DATABASE".to_string(), CLICKHOUSE_DB.to_string());
            }
            ServiceKind::Smtp => {
                // The names every mail library reaches for; the sink accepts
                // any credentials, so none are needed.
                m.insert("SMTP_HOST".to_string(), host.to_string());
                m.insert("SMTP_PORT".to_string(), p.to_string());
                m.insert("SMTP_URL".to_string(), format!("smtp://{host}:{p}"));
                m.insert("MAIL_HOST".to_string(), host.to_string());
                m.insert("MAIL_PORT".to_string(), p.to_string());
                m.insert("MAIL_MAILER".to_string(), "smtp".to_string());
                m.insert("EMAIL_HOST".to_string(), host.to_string());
                m.insert("EMAIL_PORT".to_string(), p.to_string());
            }
            ServiceKind::Minio => {
                let url = format!("http://{host}:{p}");
                m.insert("S3_ENDPOINT".to_string(), url.clone());
                m.insert("AWS_ENDPOINT_URL".to_string(), url.clone());
                m.insert("AWS_ENDPOINT_URL_S3".to_string(), url.clone());
                m.insert("MINIO_ENDPOINT".to_string(), url);
                m.insert("AWS_ACCESS_KEY_ID".to_string(), crate::services::minio::MINIO_USER.to_string());
                m.insert(
                    "AWS_SECRET_ACCESS_KEY".to_string(),
                    crate::services::minio::MINIO_PASSWORD.to_string(),
                );
                m.insert("AWS_REGION".to_string(), crate::services::minio::MINIO_REGION.to_string());
                m.insert(
                    "AWS_DEFAULT_REGION".to_string(),
                    crate::services::minio::MINIO_REGION.to_string(),
                );
                // Path-style is mandatory against MinIO: a virtual-host URL
                // (`http://bucket.127.0.0.1:9000`) doesn't resolve.
                m.insert("AWS_S3_FORCE_PATH_STYLE".to_string(), "true".to_string());
            }
            ServiceKind::Cassandra => {
                m.insert("CASSANDRA_CONTACT_POINTS".to_string(), host.to_string());
                m.insert("CASSANDRA_HOST".to_string(), host.to_string());
                m.insert("CASSANDRA_PORT".to_string(), p.to_string());
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A throwaway directory containing the given marker files, cleaned up on drop.
    struct Scratch {
        dir: std::path::PathBuf,
    }

    impl Scratch {
        fn with(files: &[&str]) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!("nw-fw-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            for f in files {
                std::fs::write(dir.join(f), b"{}").unwrap();
            }
            Scratch { dir }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn detected_name(files: &[&str]) -> Option<String> {
        let s = Scratch::with(files);
        detect(&s.dir, None).map(|f| f.name().to_string())
    }

    #[test]
    fn detects_spring_boot() {
        assert_eq!(detected_name(&["pom.xml"]).as_deref(), Some("spring-boot"));
        assert_eq!(detected_name(&["build.gradle"]).as_deref(), Some("spring-boot"));
    }

    #[test]
    fn detects_go() {
        assert_eq!(detected_name(&["go.mod"]).as_deref(), Some("go"));
    }

    #[test]
    fn detects_node() {
        assert_eq!(detected_name(&["package.json"]).as_deref(), Some("node"));
    }

    #[test]
    fn detects_fastapi() {
        assert_eq!(
            detected_name(&["pyproject.toml", "main.py"]).as_deref(),
            Some("fastapi")
        );
        assert_eq!(detected_name(&["main.py"]).as_deref(), Some("fastapi"));
    }

    #[test]
    fn detects_django() {
        assert_eq!(detected_name(&["manage.py"]).as_deref(), Some("django"));
    }

    #[test]
    fn detects_dotnet() {
        assert_eq!(detected_name(&["Api.csproj"]).as_deref(), Some("dotnet"));
        assert_eq!(detected_name(&["Solution.sln"]).as_deref(), Some("dotnet"));
        assert_eq!(detected_name(&["App.fsproj"]).as_deref(), Some("dotnet"));
    }

    #[test]
    fn detects_laravel() {
        assert_eq!(detected_name(&["artisan"]).as_deref(), Some("laravel"));
    }

    #[test]
    fn detects_rails() {
        // bin/rails is nested, so build the marker by hand.
        let s = Scratch::with(&[]);
        std::fs::create_dir_all(s.dir.join("bin")).unwrap();
        std::fs::write(s.dir.join("bin").join("rails"), b"#!/usr/bin/env ruby").unwrap();
        assert_eq!(detect(&s.dir, None).map(|f| f.name().to_string()).as_deref(), Some("rails"));
    }

    #[test]
    fn django_wins_over_fastapi_when_both_markers_present() {
        // A Django project that also happens to ship a main.py must resolve to
        // django (manage.py checked first via registry ordering).
        assert_eq!(
            detected_name(&["manage.py", "main.py"]).as_deref(),
            Some("django")
        );
    }

    #[test]
    fn spring_wins_over_node_when_both_present() {
        // A Spring project shipping a package.json (e.g. a JS frontend) must
        // still resolve to spring-boot thanks to registry ordering.
        assert_eq!(
            detected_name(&["pom.xml", "package.json"]).as_deref(),
            Some("spring-boot")
        );
    }

    #[test]
    fn forced_name_overrides_detection() {
        let s = Scratch::with(&["go.mod"]);
        let fw = detect(&s.dir, Some("node")).unwrap();
        assert_eq!(fw.name(), "node");
    }

    #[test]
    fn nothing_detected_in_empty_dir() {
        assert_eq!(detected_name(&[]), None);
    }

    #[test]
    fn non_spring_frameworks_default_to_tcp_and_port_env() {
        for f in [
            Box::new(NodeJs) as Box<dyn Framework>,
            Box::new(FastApi),
            Box::new(Go),
            Box::new(Django),
            Box::new(Rails),
            Box::new(Laravel),
        ] {
            assert_eq!(f.default_health_path(), "none");
            assert_eq!(f.default_port_env(), "PORT");
        }
        assert_eq!(SpringBoot.default_port_env(), "SERVER_PORT");
        assert_eq!(SpringBoot.default_health_path(), "/actuator/health");
        // .NET binds via Kestrel's own env var, not PORT.
        assert_eq!(DotNet.default_port_env(), "ASPNETCORE_HTTP_PORTS");
        assert_eq!(DotNet.default_health_path(), "none");
    }

    #[test]
    fn laravel_env_wiring_covers_all_service_kinds_and_emits_db_vars() {
        let eps: Vec<ServiceEndpoint> = [
            ServiceKind::Postgres,
            ServiceKind::Mysql,
            ServiceKind::Mongodb,
            ServiceKind::Redis,
            ServiceKind::Kafka,
            ServiceKind::Elastic,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, kind)| ServiceEndpoint {
            service: kind.as_str().to_string(),
            kind,
            host_port: 20000 + i as u16,
            container_port: 5432,
            aux_ports: BTreeMap::new(),
        })
        .collect();

        let env = Laravel.env_wiring(&eps);
        // Postgres is index 0, but Mysql (index 1) overwrites DB_CONNECTION last.
        assert_eq!(env.get("DB_CONNECTION").map(|s| s.as_str()), Some("mysql"));
        assert_eq!(env.get("DB_PORT").map(|s| s.as_str()), Some("20001"));
        assert_eq!(env.get("REDIS_PORT").map(|s| s.as_str()), Some("20003"));
        // Base conventional vars still present for kinds Laravel doesn't map.
        assert_eq!(env.get("KAFKA_BROKERS").map(|s| s.as_str()), Some("127.0.0.1:20004"));
    }

    #[test]
    fn conventional_env_covers_all_service_kinds() {
        let eps: Vec<ServiceEndpoint> = [
            ServiceKind::Postgres,
            ServiceKind::Mysql,
            ServiceKind::Mongodb,
            ServiceKind::Redis,
            ServiceKind::Kafka,
            ServiceKind::Elastic,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, kind)| ServiceEndpoint {
            service: kind.as_str().to_string(),
            kind,
            host_port: 10000 + i as u16,
            container_port: 5432,
            aux_ports: BTreeMap::new(),
        })
        .collect();

        let env = conventional_env(&eps);
        assert!(env["DATABASE_URL"].starts_with("postgres://") || env["DATABASE_URL"].starts_with("mysql://"));
        assert_eq!(env.get("REDIS_URL").map(|s| s.as_str()), Some("redis://127.0.0.1:10003"));
        assert_eq!(env.get("KAFKA_BROKERS").map(|s| s.as_str()), Some("127.0.0.1:10004"));
        assert_eq!(env.get("MONGODB_URI").map(|s| s.as_str()), Some("mongodb://127.0.0.1:10002"));
        assert_eq!(env.get("ELASTICSEARCH_URL").map(|s| s.as_str()), Some("http://127.0.0.1:10005"));
        // generic fallback vars still present
        assert_eq!(env.get("NOWORRIES_REDIS_PORT").map(|s| s.as_str()), Some("10003"));
    }

    fn ep(kind: ServiceKind, host_port: u16) -> ServiceEndpoint {
        ServiceEndpoint {
            service: "svc".into(),
            kind,
            host_port,
            container_port: 1433,
            aux_ports: Default::default(),
        }
    }

    #[test]
    fn mssql_ships_the_password_under_both_names_and_a_database() {
        let env = conventional_env(&[ep(ServiceKind::Mssql, 14330)]);
        assert_eq!(env["MSSQL_USER"], "sa");
        assert_eq!(env["MSSQL_PASSWORD"], env["MSSQL_SA_PASSWORD"]);
        // SQL Server creates no database of its own — `master` is where an app
        // lands, and saying so beats leaving it to be discovered.
        assert_eq!(env["MSSQL_DATABASE"], "master");
        assert!(env["MSSQL_JDBC_URL"].contains("jdbc:sqlserver://127.0.0.1:14330"));
    }

    #[test]
    fn clickhouse_dsn_carries_credentials_and_the_database() {
        // The HTTP interface ignores the user's default database, so a bare
        // CLICKHOUSE_URL silently writes to `default`.
        let env = conventional_env(&[ep(ServiceKind::Clickhouse, 8124)]);
        assert_eq!(env["CLICKHOUSE_URL"], "http://127.0.0.1:8124");
        assert_eq!(
            env["CLICKHOUSE_DSN"],
            "http://noworries:noworries@127.0.0.1:8124/?database=noworries"
        );
    }

    #[test]
    fn spring_apps_still_get_the_conventional_vars() {
        // Spring has no starter for ClickHouse; the app uses a plain client
        // that reads CLICKHOUSE_URL, which used to be missing entirely.
        let env = SpringBoot.env_wiring(&[
            ep(ServiceKind::Clickhouse, 8124),
            ep(ServiceKind::Postgres, 5433),
        ]);
        assert!(env.contains_key("CLICKHOUSE_URL"));
        assert!(env.contains_key("CLICKHOUSE_DSN"));
        // ...without losing Spring's own property names.
        assert!(env["SPRING_DATASOURCE_URL"].starts_with("jdbc:postgresql://"));
        assert!(env.contains_key("NOWORRIES_POSTGRES_PORT"));
    }

    #[test]
    fn dotnet_binds_the_assigned_port_unambiguously() {
        // ASPNETCORE_HTTP_PORTS alone loses to launchSettings.json.
        let extra = DotNet.port_env_extra(5123);
        assert_eq!(extra["ASPNETCORE_URLS"], "http://0.0.0.0:5123");
        let dir = std::env::temp_dir();
        assert_eq!(
            DotNet.default_start_command(&dir).as_deref(),
            Some("dotnet run --no-launch-profile")
        );
        // Other frameworks add nothing.
        assert!(Go.port_env_extra(1).is_empty());
    }
}
