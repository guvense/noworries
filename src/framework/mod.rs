//! Framework adapters. A [`Framework`] knows how to detect its kind of project,
//! how to start it, where its health endpoint lives, and how to translate
//! resolved service endpoints into environment variables. This module owns only
//! the interface, the registry, and shared helpers — each framework lives in
//! its own file under `framework/`. Adding one is a new file + one line in
//! [`registry`].

use std::collections::BTreeMap;
use std::path::Path;

use crate::lifecycle::ServiceEndpoint;
use crate::services::mysql::{MYSQL_DB, MYSQL_PASSWORD, MYSQL_USER};
use crate::services::{PG_DB, PG_PASSWORD, PG_USER};
use crate::spec::ServiceKind;

pub mod go;
pub mod node;
pub mod python;
pub mod spring_boot;

pub use go::Go;
pub use node::NodeJs;
pub use python::FastApi;
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
        Box::new(Go),
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
        ] {
            assert_eq!(f.default_health_path(), "none");
            assert_eq!(f.default_port_env(), "PORT");
        }
        assert_eq!(SpringBoot.default_port_env(), "SERVER_PORT");
        assert_eq!(SpringBoot.default_health_path(), "/actuator/health");
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
}
