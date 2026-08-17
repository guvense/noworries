//! Spring Boot framework adapter.

use std::collections::BTreeMap;
use std::path::Path;

use super::{conventional_env, Framework};
use crate::lifecycle::ServiceEndpoint;
use crate::services::mssql::MSSQL_SA_PASSWORD;
use crate::services::rabbitmq::{RABBITMQ_PASSWORD, RABBITMQ_USER};
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
        // Start from the conventional connection vars, then layer Spring's own
        // property names on top. Spring covers the datastores it has starters
        // for, but a Spring app talking to ClickHouse or Cassandra uses a plain
        // client that reads `CLICKHOUSE_URL` / `CASSANDRA_CONTACT_POINTS` — and
        // withholding those left it with nothing but `NOWORRIES_*`.
        let mut m = conventional_env(endpoints);
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
                ServiceKind::Mysql => {
                    m.insert(
                        "SPRING_DATASOURCE_URL".to_string(),
                        format!("jdbc:mysql://127.0.0.1:{}/noworries", ep.host_port),
                    );
                    m.insert("SPRING_DATASOURCE_USERNAME".to_string(), "noworries".to_string());
                    m.insert("SPRING_DATASOURCE_PASSWORD".to_string(), "noworries".to_string());
                }
                ServiceKind::Mongodb => {
                    m.insert(
                        "SPRING_DATA_MONGODB_URI".to_string(),
                        format!("mongodb://127.0.0.1:{}", ep.host_port),
                    );
                }
                ServiceKind::Cockroach => {
                    // Postgres-wire; single-node insecure uses root/defaultdb.
                    m.insert(
                        "SPRING_DATASOURCE_URL".to_string(),
                        format!(
                            "jdbc:postgresql://127.0.0.1:{}/defaultdb?sslmode=disable",
                            ep.host_port
                        ),
                    );
                    m.insert("SPRING_DATASOURCE_USERNAME".to_string(), "root".to_string());
                    m.insert("SPRING_DATASOURCE_PASSWORD".to_string(), "".to_string());
                }
                ServiceKind::Mssql => {
                    m.insert(
                        "SPRING_DATASOURCE_URL".to_string(),
                        format!(
                            "jdbc:sqlserver://127.0.0.1:{};encrypt=true;trustServerCertificate=true",
                            ep.host_port
                        ),
                    );
                    m.insert("SPRING_DATASOURCE_USERNAME".to_string(), "sa".to_string());
                    m.insert(
                        "SPRING_DATASOURCE_PASSWORD".to_string(),
                        MSSQL_SA_PASSWORD.to_string(),
                    );
                }
                ServiceKind::Rabbitmq => {
                    m.insert("SPRING_RABBITMQ_HOST".to_string(), "127.0.0.1".to_string());
                    m.insert("SPRING_RABBITMQ_PORT".to_string(), ep.host_port.to_string());
                    m.insert("SPRING_RABBITMQ_USERNAME".to_string(), RABBITMQ_USER.to_string());
                    m.insert(
                        "SPRING_RABBITMQ_PASSWORD".to_string(),
                        RABBITMQ_PASSWORD.to_string(),
                    );
                }
                ServiceKind::Cassandra => {
                    m.insert(
                        "SPRING_CASSANDRA_CONTACT_POINTS".to_string(),
                        format!("127.0.0.1:{}", ep.host_port),
                    );
                    m.insert("SPRING_CASSANDRA_PORT".to_string(), ep.host_port.to_string());
                    m.insert(
                        "SPRING_CASSANDRA_LOCAL_DATACENTER".to_string(),
                        "datacenter1".to_string(),
                    );
                }
                // ClickHouse and OpenSearch have no ubiquitous Spring Boot
                // auto-config keys; the framework-agnostic NOWORRIES_* vars from
                // generic_env cover them.
                ServiceKind::Clickhouse | ServiceKind::Opensearch => {}
            }
        }
        m
    }
}
