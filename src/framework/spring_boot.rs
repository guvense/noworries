//! Spring Boot framework adapter.

use std::collections::BTreeMap;
use std::path::Path;

use super::{generic_env, Framework};
use crate::lifecycle::ServiceEndpoint;
use crate::services::{PG_DB, PG_PASSWORD, PG_USER};
use crate::spec::ServiceKind;

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
                ServiceKind::Elastic => {
                    m.insert(
                        "SPRING_ELASTICSEARCH_URIS".to_string(),
                        format!("http://127.0.0.1:{}", ep.host_port),
                    );
                }
            }
        }
        m
    }
}
