//! Edge-case load/timing scenarios — the "not just the happy path" layer.
//!
//! A data pipeline (Flink and friends) can pass a single trigger→observe check
//! yet still be wrong under load: it may drop messages under a burst, corrupt
//! state under concurrent writes to the same key, double-process duplicates, or
//! mishandle out-of-order arrival. This module lets a check declare a
//! [`ScenarioSpec`] whose `kind` selects a strategy that expands into a concrete
//! [`ScenarioPlan`] — many messages, possibly produced concurrently — while the
//! check's ordinary observe assertions verify the invariant (exact count, one
//! row per key, dedup, ordering).
//!
//! Extensible exactly like `framework/` and `services/`: the [`EdgeCase`] trait
//! is the interface, each strategy lives in its own file, and adding one is a new
//! file plus a line in [`registry`]. Flink is the first consumer; the same
//! scenarios apply to any service under test that reads from Kafka.

use anyhow::{anyhow, Result};
use serde_json::Value as Json;

use crate::spec::ScenarioSpec;

pub mod burst;
pub mod concurrent;
pub mod duplicates;
pub mod out_of_order;

pub use burst::Burst;
pub use concurrent::Concurrent;
pub use duplicates::Duplicates;
pub use out_of_order::OutOfOrder;

/// One message the plan will produce.
#[derive(Debug, Clone, PartialEq)]
pub struct ProduceAction {
    pub topic: String,
    pub key: Option<String>,
    pub payload: Json,
}

/// A concrete, executable plan expanded from a [`ScenarioSpec`].
#[derive(Debug, Clone)]
pub struct ScenarioPlan {
    /// Messages to produce, in the order they should be sent.
    pub actions: Vec<ProduceAction>,
    /// Parallel producer threads (>= 1). Concurrency is what turns an ordered
    /// list of writes into a real race.
    pub concurrency: usize,
    /// Minimum milliseconds between messages per producer (rate limiting);
    /// 0 = produce as fast as possible.
    pub per_message_delay_ms: u64,
    /// One-line human summary for the run report.
    pub summary: String,
}

/// The extension point: a strategy that expands a scenario into a plan.
pub trait EdgeCase {
    /// The `kind` token used in `scenario.kind`.
    fn kind(&self) -> &'static str;
    /// What invariant this scenario stresses (shown in help/docs).
    fn describe(&self) -> &'static str;
    /// Expand the declared scenario into an executable plan.
    fn plan(&self, s: &ScenarioSpec) -> Result<ScenarioPlan>;
}

/// Registry / composition root: all known edge-case strategies.
pub fn registry() -> Vec<Box<dyn EdgeCase>> {
    vec![
        Box::new(Burst),
        Box::new(Concurrent),
        Box::new(Duplicates),
        Box::new(OutOfOrder),
    ]
}

/// Resolve a strategy by `kind`, with a helpful error listing the known kinds.
pub fn resolve(kind: &str) -> Result<Box<dyn EdgeCase>> {
    let known: Vec<&str> = registry().iter().map(|e| e.kind()).collect();
    registry()
        .into_iter()
        .find(|e| e.kind().eq_ignore_ascii_case(kind))
        .ok_or_else(|| anyhow!("unknown scenario kind \"{kind}\" (known: {})", known.join(", ")))
}

/// Placeholder substitutions available in `key`/`message` templates.
pub struct Subs {
    /// Global 0-based message index across the whole plan.
    pub seq: u32,
    /// Per-key 0-based sequence (a version number for that key).
    pub i: u32,
    /// The assigned key value.
    pub key: String,
    /// A plan-unique token (`u-<seq>`), handy for a distinct id field.
    pub uuid: String,
}

impl Subs {
    fn apply(&self, s: &str) -> String {
        s.replace("${seq}", &self.seq.to_string())
            .replace("${i}", &self.i.to_string())
            .replace("${key}", &self.key)
            .replace("${uuid}", &self.uuid)
    }
}

/// Expand a YAML message template into JSON, substituting `${...}` placeholders
/// in every string leaf. Non-string scalars pass through unchanged.
pub fn render(template: &serde_yaml::Value, subs: &Subs) -> Json {
    match template {
        serde_yaml::Value::String(s) => Json::String(subs.apply(s)),
        serde_yaml::Value::Bool(b) => Json::Bool(*b),
        serde_yaml::Value::Null => Json::Null,
        serde_yaml::Value::Number(_) => serde_json::to_value(template).unwrap_or(Json::Null),
        serde_yaml::Value::Sequence(seq) => {
            Json::Array(seq.iter().map(|e| render(e, subs)).collect())
        }
        serde_yaml::Value::Mapping(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                let key = match k {
                    serde_yaml::Value::String(s) => subs.apply(s),
                    other => serde_json::to_value(other)
                        .ok()
                        .and_then(|j| j.as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| format!("{other:?}")),
                };
                obj.insert(key, render(v, subs));
            }
            Json::Object(obj)
        }
        _ => Json::Null,
    }
}

/// Build one [`ProduceAction`] from a scenario's kafka template for a given
/// (seq, per-key i, key) triple. Shared by every strategy so templating stays
/// consistent.
pub fn action_for(s: &ScenarioSpec, seq: u32, i: u32, key_value: String) -> ProduceAction {
    let subs = Subs {
        seq,
        i,
        key: key_value.clone(),
        uuid: format!("u-{seq}"),
    };
    let key = s
        .kafka
        .key
        .as_ref()
        .map(|k| subs.apply(k))
        .or(Some(key_value));
    ProduceAction {
        topic: s.kafka.topic.clone(),
        key,
        payload: render(&s.kafka.message, &subs),
    }
}

/// Convert an optional `rate_per_sec` into a per-message delay (ms).
pub fn delay_from_rate(rate_per_sec: Option<u32>) -> u64 {
    match rate_per_sec {
        Some(r) if r > 0 => 1000 / r as u64,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{ScenarioKafka, ScenarioSpec};

    fn spec(kind: &str, count: Option<u32>, keys: Option<u32>, concurrency: Option<u32>) -> ScenarioSpec {
        ScenarioSpec {
            kind: kind.to_string(),
            kafka: ScenarioKafka {
                topic: "orders-in".to_string(),
                key: None,
                message: serde_yaml::from_str("{ id: \"${key}\", version: \"${i}\", n: \"${seq}\" }").unwrap(),
            },
            count,
            concurrency,
            keys,
            rate_per_sec: None,
            expect_throughput_per_sec: None,
        }
    }

    #[test]
    fn resolve_lists_known_kinds_on_error() {
        assert!(resolve("burst").is_ok());
        assert!(resolve("Concurrent").is_ok()); // case-insensitive
        let err = resolve("nope").err().expect("unknown kind should error").to_string();
        assert!(err.contains("burst") && err.contains("out_of_order"), "{err}");
    }

    #[test]
    fn burst_one_key_per_message_by_default() {
        let plan = Burst.plan(&spec("burst", Some(5), None, None)).unwrap();
        assert_eq!(plan.actions.len(), 5);
        assert_eq!(plan.concurrency, 1);
        // distinct keys k0..k4
        let keys: Vec<_> = plan.actions.iter().map(|a| a.key.clone().unwrap()).collect();
        assert_eq!(keys, vec!["k0", "k1", "k2", "k3", "k4"]);
    }

    #[test]
    fn concurrent_races_over_few_keys_with_per_key_versions() {
        let plan = Concurrent.plan(&spec("concurrent", Some(6), Some(2), Some(4))).unwrap();
        assert_eq!(plan.actions.len(), 6);
        assert_eq!(plan.concurrency, 4);
        // key k0 gets versions 0,1,2 ; k1 gets 0,1,2
        let k0_versions: Vec<_> = plan
            .actions
            .iter()
            .filter(|a| a.key.as_deref() == Some("k0"))
            .map(|a| a.payload.get("version").unwrap().as_str().unwrap().to_string())
            .collect();
        assert_eq!(k0_versions, vec!["0", "1", "2"]);
    }

    #[test]
    fn concurrent_forces_min_two_producers() {
        let plan = Concurrent.plan(&spec("concurrent", Some(4), Some(1), Some(1))).unwrap();
        assert!(plan.concurrency >= 2);
    }

    #[test]
    fn duplicates_sends_each_message_twice() {
        let plan = Duplicates.plan(&spec("duplicates", Some(3), None, None)).unwrap();
        assert_eq!(plan.actions.len(), 6);
        // consecutive pairs are identical
        assert_eq!(plan.actions[0], plan.actions[1]);
        assert_eq!(plan.actions[2], plan.actions[3]);
        assert_ne!(plan.actions[0], plan.actions[2]);
    }

    #[test]
    fn out_of_order_reverses_arrival_but_keeps_logical_index() {
        let plan = OutOfOrder.plan(&spec("out_of_order", Some(4), Some(1), None)).unwrap();
        assert_eq!(plan.actions.len(), 4);
        // arrival order carries version 3,2,1,0 (highest first)
        let versions: Vec<_> = plan
            .actions
            .iter()
            .map(|a| a.payload.get("version").unwrap().as_str().unwrap().to_string())
            .collect();
        assert_eq!(versions, vec!["3", "2", "1", "0"]);
    }

    #[test]
    fn out_of_order_distributes_count_across_keys() {
        // 5 messages over 2 keys => 3 + 2
        let plan = OutOfOrder.plan(&spec("out_of_order", Some(5), Some(2), None)).unwrap();
        assert_eq!(plan.actions.len(), 5);
        let k0 = plan.actions.iter().filter(|a| a.key.as_deref() == Some("k0")).count();
        let k1 = plan.actions.iter().filter(|a| a.key.as_deref() == Some("k1")).count();
        assert_eq!((k0, k1), (3, 2));
    }

    #[test]
    fn render_substitutes_all_placeholders() {
        let plan = Burst.plan(&spec("burst", Some(2), None, None)).unwrap();
        let first = &plan.actions[0].payload;
        assert_eq!(first.get("id").unwrap().as_str(), Some("k0"));
        assert_eq!(first.get("version").unwrap().as_str(), Some("0"));
        assert_eq!(first.get("n").unwrap().as_str(), Some("0"));
        let second = &plan.actions[1].payload;
        assert_eq!(second.get("n").unwrap().as_str(), Some("1"));
    }

    #[test]
    fn explicit_key_template_overrides_default() {
        let mut s = spec("burst", Some(2), None, None);
        s.kafka.key = Some("order-${seq}".to_string());
        let plan = Burst.plan(&s).unwrap();
        assert_eq!(plan.actions[0].key.as_deref(), Some("order-0"));
        assert_eq!(plan.actions[1].key.as_deref(), Some("order-1"));
    }

    #[test]
    fn zero_count_is_rejected() {
        assert!(Burst.plan(&spec("burst", Some(0), None, None)).is_err());
    }
}
