//! Types and parsing for the human/AI-editable `noworries.yml` spec file.
//!
//! The spec is intentionally framework-agnostic: it declares *what* infra a
//! feature needs and *what* "correct" means, never how a specific framework is
//! wired. Framework specifics live behind the `Framework` trait.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

pub const SPEC_FILENAME: &str = "noworries.yml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    Postgres,
    Kafka,
    Redis,
}

impl ServiceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceKind::Postgres => "postgres",
            ServiceKind::Kafka => "kafka",
            ServiceKind::Redis => "redis",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        match token.to_ascii_lowercase().as_str() {
            "postgres" => Some(ServiceKind::Postgres),
            "kafka" => Some(ServiceKind::Kafka),
            "redis" => Some(ServiceKind::Redis),
            _ => None,
        }
    }
}

/// A declared service such as `postgres:16-alpine`.
#[derive(Debug, Clone)]
pub struct ServiceDecl {
    pub kind: ServiceKind,
    pub image: String,
    pub tag: Option<String>,
    pub raw: String,
}

/// The Docker Hub repository each kind maps to. Note Kafka's official image is
/// `apache/kafka`, not `kafka`, so `kafka:3.7` must expand to `apache/kafka:3.7`.
pub fn default_repo(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Postgres => "postgres",
        ServiceKind::Kafka => "apache/kafka",
        ServiceKind::Redis => "redis",
    }
}

/// Default images used when the spec gives a bare kind with no tag.
pub fn default_image(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Postgres => "postgres:16-alpine",
        ServiceKind::Kafka => "apache/kafka:3.7.0",
        ServiceKind::Redis => "redis:7-alpine",
    }
}

fn parse_service_decl(raw: &str) -> Result<ServiceDecl> {
    let value = raw.trim();
    if value.is_empty() {
        bail!("services entries must be non-empty strings like \"postgres:16-alpine\"");
    }
    let mut parts = value.splitn(2, ':');
    let kind_token = parts.next().unwrap_or("");
    let kind = ServiceKind::from_token(kind_token)
        .ok_or_else(|| anyhow!("unsupported service \"{value}\". supported: postgres, kafka, redis"))?;
    let tag = parts.next().map(|s| s.to_string());
    // Expand "<kind>:<tag>" to the kind's real repository, e.g.
    // "kafka:3.7" -> "apache/kafka:3.7"; bare "<kind>" -> the default image.
    let image = match &tag {
        Some(t) => format!("{}:{}", default_repo(kind), t),
        None => default_image(kind).to_string(),
    };
    Ok(ServiceDecl { kind, image, tag, raw: value.to_string() })
}

/// How to launch the app under test. All optional: `start` is auto-detected
/// from the resolved framework when omitted; `framework` forces a specific
/// framework instead of auto-detecting.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSpec {
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub health: Option<String>,
    #[serde(default)]
    pub framework: Option<String>,
    #[serde(default, rename = "port_env")]
    pub port_env: Option<String>,
    #[serde(default, rename = "ready_timeout")]
    pub ready_timeout: Option<u64>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpRequestSpec {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpExpectSpec {
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub body_contains: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DbAssertion {
    pub query: String,
    #[serde(default)]
    pub expect_row: Option<serde_yaml::Value>,
    #[serde(default)]
    pub expect_row_count: Option<i64>,
}

/// Kafka assertion. Supports producing a message to a topic (to exercise a
/// consumer the feature added) and/or expecting a message on a topic (to
/// verify a producer the feature added).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KafkaAssertion {
    #[serde(default)]
    pub produce: Option<KafkaProduce>,
    #[serde(default)]
    pub expect_message: Option<KafkaExpect>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KafkaProduce {
    pub topic: String,
    pub message: serde_yaml::Value,
    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KafkaExpect {
    pub topic: String,
    pub contains: serde_yaml::Value,
    #[serde(default, rename = "timeout_ms")]
    pub timeout_ms: Option<u64>,
}

/// Redis assertion: check a key exists and/or its (possibly JSON) value matches.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedisAssertion {
    pub key: String,
    #[serde(default)]
    pub expect_exists: Option<bool>,
    #[serde(default)]
    pub expect_value: Option<serde_yaml::Value>,
    #[serde(default)]
    pub expect_value_contains: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckSpec {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub request: Option<HttpRequestSpec>,
    #[serde(default)]
    pub expect: Option<HttpExpectSpec>,
    #[serde(default)]
    pub db: Option<DbAssertion>,
    #[serde(default)]
    pub kafka: Option<KafkaAssertion>,
    #[serde(default)]
    pub redis: Option<RedisAssertion>,
}

/// Raw shape as it appears on disk (services are strings here).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpec {
    version: u32,
    services: Vec<String>,
    #[serde(default)]
    app: Option<AppSpec>,
    #[serde(default)]
    checks: Vec<CheckSpec>,
}

/// Fully parsed + validated spec.
#[derive(Debug, Clone)]
pub struct NoworriesSpec {
    pub version: u32,
    pub services: Vec<ServiceDecl>,
    pub app: Option<AppSpec>,
    pub checks: Vec<CheckSpec>,
}

impl NoworriesSpec {
    pub fn parse(source: &str) -> Result<Self> {
        let raw: RawSpec = serde_yaml::from_str(source)
            .with_context(|| format!("could not parse {SPEC_FILENAME}"))?;
        if raw.version != 1 {
            bail!("{SPEC_FILENAME}: unsupported version (expected 1, got {})", raw.version);
        }
        if raw.services.is_empty() {
            bail!("{SPEC_FILENAME}: \"services\" must be a non-empty list.");
        }
        let services = raw
            .services
            .iter()
            .map(|s| parse_service_decl(s))
            .collect::<Result<Vec<_>>>()?;
        for c in &raw.checks {
            if c.name.trim().is_empty() {
                bail!("{SPEC_FILENAME}: every check needs a non-empty \"name\".");
            }
        }
        Ok(NoworriesSpec {
            version: 1,
            services,
            app: raw.app,
            checks: raw.checks,
        })
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            bail!(
                "no {SPEC_FILENAME} found at {}. Run `noworries init` to scaffold one.",
                path.display()
            );
        }
        let source = fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&source)
    }

    #[cfg(test)]
    pub(crate) fn services_for_test(&self) -> &[ServiceDecl] {
        &self.services
    }

    /// Checks matching any of `tags`; all checks when `tags` is empty.
    pub fn select_checks(&self, tags: &[String]) -> Vec<CheckSpec> {
        if tags.is_empty() {
            return self.checks.clone();
        }
        self.checks
            .iter()
            .filter(|c| c.tags.iter().any(|t| tags.contains(t)))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_services(yaml: &str) -> Vec<ServiceDecl> {
        NoworriesSpec::parse(yaml).unwrap().services_for_test().to_vec()
    }

    #[test]
    fn kafka_shorthand_expands_to_apache_kafka() {
        let s = parse_services("version: 1\nservices: [kafka:3.7]\n");
        assert_eq!(s[0].kind, ServiceKind::Kafka);
        assert_eq!(s[0].image, "apache/kafka:3.7");
    }

    #[test]
    fn bare_kind_uses_default_image() {
        let s = parse_services("version: 1\nservices: [postgres, kafka, redis]\n");
        assert_eq!(s[0].image, "postgres:16-alpine");
        assert_eq!(s[1].image, "apache/kafka:3.7.0");
        assert_eq!(s[2].image, "redis:7-alpine");
    }

    #[test]
    fn unknown_service_is_rejected() {
        assert!(NoworriesSpec::parse("version: 1\nservices: [mysql:8]\n").is_err());
    }
}
