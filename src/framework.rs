//! Framework adapters. A `Framework` knows how to detect its kind of project,
//! how to start it, where its health endpoint lives, and how to translate
//! resolved service endpoints into the environment variables that framework
//! expects. Spring Boot is implemented today; adding Go (or anything else) is a
//! new impl + one line in `registry()`.

use std::collections::BTreeMap;
use std::path::Path;

use crate::lifecycle::ServiceEndpoint;
use crate::services::{PG_DB, PG_PASSWORD, PG_USER};
use crate::spec::ServiceKind;

pub trait Framework {
    fn name(&self) -> &'static str;
    /// Does this framework appear to be used in `dir`?
    fn detect(&self, dir: &Path) -> bool;
    /// Best-guess command to start the app, if inferable from `dir`.
    fn default_start_command(&self, dir: &Path) -> Option<String>;
    /// Health endpoint polled until 2xx before checks run.
    fn default_health_path(&self) -> &'static str;
    /// Environment variables wiring the app to the resolved service endpoints.
    fn env_wiring(&self, endpoints: &[ServiceEndpoint]) -> BTreeMap<String, String>;
}

/// All known frameworks, in detection-priority order.
pub fn registry() -> Vec<Box<dyn Framework>> {
    vec![Box::new(SpringBoot)]
    // Future: Box::new(Go), Box::new(NodeExpress), ...
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
pub fn generic_env(endpoints: &[ServiceEndpoint]) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for ep in endpoints {
        let k = ep.kind.as_str().to_uppercase();
        m.insert(format!("NOWORRIES_{k}_HOST"), "127.0.0.1".to_string());
        m.insert(format!("NOWORRIES_{k}_PORT"), ep.host_port.to_string());
    }
    m
}

pub struct SpringBoot;

impl Framework for SpringBoot {
    fn name(&self) -> &'static str {
        "spring-boot"
    }

    fn detect(&self, dir: &Path) -> bool {
        ["pom.xml", "build.gradle", "build.gradle.kts", "mvnw", "gradlew"]
            .iter()
            .any(|f| dir.join(f).exists())
    }

    fn default_start_command(&self, dir: &Path) -> Option<String> {
        if dir.join("mvnw").exists() {
            Some("./mvnw spring-boot:run".to_string())
        } else if dir.join("pom.xml").exists() {
            Some("mvn spring-boot:run".to_string())
        } else if dir.join("gradlew").exists() {
            Some("./gradlew bootRun".to_string())
        } else if dir.join("build.gradle").exists() || dir.join("build.gradle.kts").exists() {
            Some("gradle bootRun".to_string())
        } else {
            None
        }
    }

    fn default_health_path(&self) -> &'static str {
        "/actuator/health"
    }

    fn env_wiring(&self, endpoints: &[ServiceEndpoint]) -> BTreeMap<String, String> {
        let mut m = generic_env(endpoints);
        for ep in endpoints {
            match ep.kind {
                ServiceKind::Postgres => {
                    m.insert(
                        "SPRING_DATASOURCE_URL".to_string(),
                        format!("jdbc:postgresql://127.0.0.1:{}/{PG_DB}", ep.host_port),
                    );
                    m.insert("SPRING_DATASOURCE_USERNAME".to_string(), PG_USER.to_string());
                    m.insert("SPRING_DATASOURCE_PASSWORD".to_string(), PG_PASSWORD.to_string());
                }
                ServiceKind::Kafka => {
                    m.insert(
                        "SPRING_KAFKA_BOOTSTRAP_SERVERS".to_string(),
                        format!("127.0.0.1:{}", ep.host_port),
                    );
                }
                ServiceKind::Redis => {
                    m.insert("SPRING_DATA_REDIS_HOST".to_string(), "127.0.0.1".to_string());
                    m.insert("SPRING_DATA_REDIS_PORT".to_string(), ep.host_port.to_string());
                }
            }
        }
        m
    }
}
