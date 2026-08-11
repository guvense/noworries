//! Executes noworries.yml checks against the running app + containers and
//! collects structured pass/fail results with actual-vs-expected detail.
//!
//! Phase 2: HTTP assertions (status + body_contains) and Postgres DB assertions
//! (expect_row / expect_row_count). Kafka (phase 3) and Redis (phase 4) are
//! recorded as failing "not implemented" so a check that declares them can't
//! silently pass.

use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value as Json;

use crate::lifecycle::ServiceEndpoint;
use crate::services::{PG_DB, PG_PASSWORD, PG_USER};
use crate::spec::{CheckSpec, ServiceKind};

#[derive(Debug, Clone, Serialize)]
pub struct AssertionResult {
    pub kind: String,
    pub passed: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<Json>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<Json>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    pub assertions: Vec<AssertionResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct RunnerContext {
    pub app_port: u16,
    pub endpoints: Vec<ServiceEndpoint>,
}

/// serde_yaml value -> serde_json value, so YAML expectations compare uniformly.
fn yaml_to_json(v: &serde_yaml::Value) -> Json {
    serde_json::to_value(v).unwrap_or(Json::Null)
}

fn loose_equal(a: &Json, b: &Json) -> bool {
    if a == b {
        return true;
    }
    // Compare scalars by their string form (YAML 201 vs JSON 201, "2" vs 2, ...).
    fn scalar_str(v: &Json) -> Option<String> {
        match v {
            Json::String(s) => Some(s.clone()),
            Json::Number(n) => Some(n.to_string()),
            Json::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }
    match (scalar_str(a), scalar_str(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// Deep partial match: every key/value in `expected` must appear in `actual`.
pub fn matches_subset(expected: &Json, actual: &Json) -> bool {
    match expected {
        Json::Object(exp) => {
            let Some(act) = actual.as_object() else { return false };
            exp.iter()
                .all(|(k, v)| act.get(k).map(|av| matches_subset(v, av)).unwrap_or(false))
        }
        Json::Array(exp) => {
            let Some(act) = actual.as_array() else { return false };
            if act.len() < exp.len() {
                return false;
            }
            exp.iter().enumerate().all(|(i, v)| matches_subset(v, &act[i]))
        }
        _ => loose_equal(expected, actual),
    }
}

fn truncate_json(v: Json) -> Json {
    let s = v.to_string();
    if s.len() > 500 {
        Json::String(format!("{}…(truncated)", &s[..500]))
    } else {
        v
    }
}

fn run_http(check: &CheckSpec, ctx: &RunnerContext, out: &mut Vec<AssertionResult>) -> anyhow::Result<()> {
    let Some(req) = &check.request else { return Ok(()) };
    let url = format!("http://127.0.0.1:{}{}", ctx.app_port, req.path);
    let agent = ureq::builder().timeout(Duration::from_secs(15)).build();
    let mut request = agent.request(&req.method.to_uppercase(), &url);
    for (k, v) in &req.headers {
        request = request.set(k, v);
    }

    let send_result = if let Some(body) = &req.body {
        let body_str = match body {
            serde_yaml::Value::String(s) => s.clone(),
            other => serde_json::to_string(&yaml_to_json(other)).unwrap_or_default(),
        };
        request.set("content-type", "application/json").send_string(&body_str)
    } else {
        request.call()
    };

    let (status, body_text) = match send_result {
        Ok(resp) => {
            let s = resp.status();
            (s, resp.into_string().unwrap_or_default())
        }
        Err(ureq::Error::Status(code, resp)) => (code, resp.into_string().unwrap_or_default()),
        Err(ureq::Error::Transport(t)) => {
            anyhow::bail!("HTTP {} {} failed: {}", req.method, req.path, t);
        }
    };

    if let Some(expect) = &check.expect {
        if let Some(expected_status) = expect.status {
            let passed = status == expected_status;
            out.push(AssertionResult {
                kind: "http".into(),
                passed,
                message: if passed {
                    format!("{} {} -> {}", req.method, req.path, status)
                } else {
                    format!("{} {}: expected status {}, got {}", req.method, req.path, expected_status, status)
                },
                expected: Some(Json::from(expected_status)),
                actual: Some(Json::from(status)),
            });
        }
        if let Some(body_contains) = &expect.body_contains {
            let parsed: Json = serde_json::from_str(&body_text).unwrap_or(Json::String(body_text.clone()));
            let expected = yaml_to_json(body_contains);
            let passed = matches_subset(&expected, &parsed);
            out.push(AssertionResult {
                kind: "http-body".into(),
                passed,
                message: if passed {
                    "body contains expected fields".into()
                } else {
                    "response body did not contain expected fields".into()
                },
                expected: Some(expected),
                actual: Some(truncate_json(parsed)),
            });
        }
    }
    Ok(())
}

fn cell_to_json(row: &postgres::Row, idx: usize) -> Json {
    if let Ok(v) = row.try_get::<_, Option<String>>(idx) {
        return v.map(Json::String).unwrap_or(Json::Null);
    }
    if let Ok(v) = row.try_get::<_, Option<i64>>(idx) {
        return v.map(Json::from).unwrap_or(Json::Null);
    }
    if let Ok(v) = row.try_get::<_, Option<i32>>(idx) {
        return v.map(Json::from).unwrap_or(Json::Null);
    }
    if let Ok(v) = row.try_get::<_, Option<bool>>(idx) {
        return v.map(Json::Bool).unwrap_or(Json::Null);
    }
    if let Ok(v) = row.try_get::<_, Option<f64>>(idx) {
        return serde_json::to_value(v).unwrap_or(Json::Null);
    }
    Json::String("<unreadable value>".into())
}

fn row_to_json(row: &postgres::Row) -> Json {
    let mut map = serde_json::Map::new();
    for (i, col) in row.columns().iter().enumerate() {
        map.insert(col.name().to_string(), cell_to_json(row, i));
    }
    Json::Object(map)
}

/// One evaluation of a check's DB assertions against a fresh connection.
fn evaluate_db(db: &crate::spec::DbAssertion, host_port: u16) -> Vec<AssertionResult> {
    let mut out = Vec::new();
    let conn = format!(
        "host=127.0.0.1 port={host_port} user={PG_USER} password={PG_PASSWORD} dbname={PG_DB}"
    );
    let mut client = match postgres::Client::connect(&conn, postgres::NoTls) {
        Ok(c) => c,
        Err(e) => {
            out.push(fail_db(format!("could not connect to Postgres: {e}")));
            return out;
        }
    };
    let rows = match client.query(db.query.as_str(), &[]) {
        Ok(r) => r,
        Err(e) => {
            out.push(fail_db(format!("db query failed: {e}")));
            return out;
        }
    };

    if let Some(expected_count) = db.expect_row_count {
        let passed = rows.len() as i64 == expected_count;
        out.push(AssertionResult {
            kind: "db".into(),
            passed,
            message: if passed {
                format!("query returned {} row(s)", rows.len())
            } else {
                format!("expected {} row(s), got {}", expected_count, rows.len())
            },
            expected: Some(Json::from(expected_count)),
            actual: Some(Json::from(rows.len() as i64)),
        });
    }

    if let Some(expect_row) = &db.expect_row {
        let expected = yaml_to_json(expect_row);
        match rows.first() {
            Some(row) => {
                let actual = row_to_json(row);
                let passed = matches_subset(&expected, &actual);
                out.push(AssertionResult {
                    kind: "db".into(),
                    passed,
                    message: if passed {
                        "first row matches expected fields".into()
                    } else {
                        "first row did not match expected fields".into()
                    },
                    expected: Some(expected),
                    actual: Some(actual),
                });
            }
            None => out.push(AssertionResult {
                kind: "db".into(),
                passed: false,
                message: "expected a matching row, but query returned no rows".into(),
                expected: Some(expected),
                actual: Some(Json::Null),
            }),
        }
    }
    out
}

fn fail_db(message: String) -> AssertionResult {
    AssertionResult { kind: "db".into(), passed: false, message, expected: None, actual: None }
}

/// Run DB assertions, retrying for a few seconds so an asynchronous side effect
/// (e.g. a Kafka consumer writing to the DB) has time to land.
fn run_db(check: &CheckSpec, ctx: &RunnerContext, retry: bool, out: &mut Vec<AssertionResult>) {
    let Some(db) = &check.db else { return };
    let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Postgres) else {
        out.push(fail_db("db assertion but no Postgres service is running".into()));
        return;
    };

    let deadline = Instant::now() + Duration::from_secs(if retry { 6 } else { 0 });
    let mut last = evaluate_db(db, ep.host_port);
    while retry && !last.iter().all(|a| a.passed) && Instant::now() < deadline {
        sleep(Duration::from_millis(400));
        last = evaluate_db(db, ep.host_port);
    }
    out.extend(last);
}

/// One evaluation of a check's Redis assertions against a fresh connection.
fn evaluate_redis(r: &crate::spec::RedisAssertion, port: u16) -> Vec<AssertionResult> {
    let mut out = Vec::new();
    let client = match redis::Client::open(format!("redis://127.0.0.1:{port}/")) {
        Ok(c) => c,
        Err(e) => {
            out.push(fail_redis(format!("could not open Redis client: {e}")));
            return out;
        }
    };
    let mut con = match client.get_connection() {
        Ok(c) => c,
        Err(e) => {
            out.push(fail_redis(format!("could not connect to Redis: {e}")));
            return out;
        }
    };

    let exists: bool = match redis::cmd("EXISTS").arg(&r.key).query(&mut con) {
        Ok(v) => v,
        Err(e) => {
            out.push(fail_redis(format!("EXISTS {} failed: {e}", r.key)));
            return out;
        }
    };

    // Default expectation when none is given: the key should exist.
    let want_exists = r.expect_exists.unwrap_or(true);
    if r.expect_exists.is_some() || (r.expect_value.is_none() && r.expect_value_contains.is_none()) {
        let passed = exists == want_exists;
        out.push(AssertionResult {
            kind: "redis".into(),
            passed,
            message: if passed {
                format!("key \"{}\" exists = {}", r.key, exists)
            } else {
                format!("key \"{}\": expected exists={}, got {}", r.key, want_exists, exists)
            },
            expected: Some(Json::Bool(want_exists)),
            actual: Some(Json::Bool(exists)),
        });
    }

    if r.expect_value.is_some() || r.expect_value_contains.is_some() {
        let value: Option<String> = match redis::cmd("GET").arg(&r.key).query(&mut con) {
            Ok(v) => v,
            Err(e) => {
                out.push(fail_redis(format!("GET {} failed: {e}", r.key)));
                return out;
            }
        };
        let actual_json = match &value {
            Some(s) => serde_json::from_str(s).unwrap_or_else(|_| Json::String(s.clone())),
            None => Json::Null,
        };
        if let Some(expect_value) = &r.expect_value {
            let expected = yaml_to_json(expect_value);
            let passed = loose_equal(&expected, &actual_json);
            out.push(AssertionResult {
                kind: "redis".into(),
                passed,
                message: if passed {
                    format!("key \"{}\" value matches", r.key)
                } else {
                    format!("key \"{}\" value did not match", r.key)
                },
                expected: Some(expected),
                actual: Some(actual_json.clone()),
            });
        }
        if let Some(contains) = &r.expect_value_contains {
            let expected = yaml_to_json(contains);
            let passed = matches_subset(&expected, &actual_json);
            out.push(AssertionResult {
                kind: "redis".into(),
                passed,
                message: if passed {
                    format!("key \"{}\" value contains expected fields", r.key)
                } else {
                    format!("key \"{}\" value did not contain expected fields", r.key)
                },
                expected: Some(expected),
                actual: Some(truncate_json(actual_json)),
            });
        }
    }
    out
}

fn fail_redis(message: String) -> AssertionResult {
    AssertionResult { kind: "redis".into(), passed: false, message, expected: None, actual: None }
}

/// Run Redis assertions, retrying briefly so an async cache write can land.
fn run_redis(check: &CheckSpec, ctx: &RunnerContext, retry: bool, out: &mut Vec<AssertionResult>) {
    let Some(r) = &check.redis else { return };
    let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Redis) else {
        out.push(fail_redis("redis assertion but no Redis service is running".into()));
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(if retry { 4 } else { 0 });
    let mut last = evaluate_redis(r, ep.host_port);
    while retry && !last.iter().all(|a| a.passed) && Instant::now() < deadline {
        sleep(Duration::from_millis(300));
        last = evaluate_redis(r, ep.host_port);
    }
    out.extend(last);
}

fn yaml_to_bytes(v: &serde_yaml::Value) -> Vec<u8> {
    match v {
        serde_yaml::Value::String(s) => s.clone().into_bytes(),
        other => serde_json::to_vec(&yaml_to_json(other)).unwrap_or_default(),
    }
}

fn unique_group() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("noworries-{}-{}", std::process::id(), nanos)
}

/// Produce the message declared by a check to its topic — used to trigger a
/// consumer the feature under test added. The DB/redis assertion in the same
/// check verifies the effect.
fn kafka_produce(check: &CheckSpec, ctx: &RunnerContext, out: &mut Vec<AssertionResult>) {
    let Some(kafka) = &check.kafka else { return };
    let Some(produce) = &kafka.produce else { return };
    let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Kafka) else {
        out.push(AssertionResult {
            kind: "kafka".into(),
            passed: false,
            message: "kafka produce declared but no Kafka service is running".into(),
            expected: None,
            actual: None,
        });
        return;
    };

    let host = format!("127.0.0.1:{}", ep.host_port);
    let payload = yaml_to_bytes(&produce.message);
    let result = (|| -> anyhow::Result<()> {
        use kafka::producer::{Producer, Record, RequiredAcks};
        let mut producer = Producer::from_hosts(vec![host.clone()])
            .with_ack_timeout(Duration::from_secs(5))
            .with_required_acks(RequiredAcks::One)
            .create()?;
        if let Some(key) = &produce.key {
            producer.send(&Record::from_key_value(&produce.topic, key.as_bytes(), payload.as_slice()))?;
        } else {
            producer.send(&Record::from_value(&produce.topic, payload.as_slice()))?;
        }
        Ok(())
    })();

    match result {
        Ok(()) => out.push(AssertionResult {
            kind: "kafka".into(),
            passed: true,
            message: format!("produced message to topic \"{}\"", produce.topic),
            expected: None,
            actual: None,
        }),
        Err(e) => out.push(AssertionResult {
            kind: "kafka".into(),
            passed: false,
            message: format!("failed to produce to topic \"{}\": {e}", produce.topic),
            expected: None,
            actual: None,
        }),
    }
}

/// Consume from a topic and check a matching message arrives — used to verify a
/// producer the feature under test added.
fn kafka_expect(check: &CheckSpec, ctx: &RunnerContext, out: &mut Vec<AssertionResult>) {
    let Some(kafka) = &check.kafka else { return };
    let Some(expect) = &kafka.expect_message else { return };
    let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Kafka) else {
        out.push(AssertionResult {
            kind: "kafka".into(),
            passed: false,
            message: "kafka expect_message declared but no Kafka service is running".into(),
            expected: None,
            actual: None,
        });
        return;
    };

    let host = format!("127.0.0.1:{}", ep.host_port);
    let expected = yaml_to_json(&expect.contains);
    let timeout = Duration::from_millis(expect.timeout_ms.unwrap_or(5000));
    let expected_for_match = expected.clone();

    let found = (|| -> anyhow::Result<bool> {
        use kafka::consumer::{Consumer, FetchOffset};
        let mut consumer = Consumer::from_hosts(vec![host.clone()])
            .with_topic(expect.topic.clone())
            .with_fallback_offset(FetchOffset::Earliest)
            .with_group(unique_group())
            .create()?;
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let sets = consumer.poll()?;
            for set in sets.iter() {
                for m in set.messages() {
                    let parsed: Json = serde_json::from_slice(m.value)
                        .unwrap_or_else(|_| Json::String(String::from_utf8_lossy(m.value).into_owned()));
                    if matches_subset(&expected_for_match, &parsed) {
                        return Ok(true);
                    }
                }
            }
            sleep(Duration::from_millis(200));
        }
        Ok(false)
    })();

    match found {
        Ok(true) => out.push(AssertionResult {
            kind: "kafka".into(),
            passed: true,
            message: format!("matching message received on topic \"{}\"", expect.topic),
            expected: Some(expected),
            actual: None,
        }),
        Ok(false) => out.push(AssertionResult {
            kind: "kafka".into(),
            passed: false,
            message: format!(
                "no matching message on topic \"{}\" within {}ms",
                expect.topic,
                timeout.as_millis()
            ),
            expected: Some(expected),
            actual: None,
        }),
        Err(e) => out.push(AssertionResult {
            kind: "kafka".into(),
            passed: false,
            message: format!("kafka consume failed on topic \"{}\": {e}", expect.topic),
            expected: Some(expected),
            actual: None,
        }),
    }
}

/// Run every selected check sequentially and collect results.
pub fn run_checks(checks: &[CheckSpec], ctx: &RunnerContext) -> Vec<CheckResult> {
    let mut results = Vec::new();
    for check in checks {
        let mut assertions = Vec::new();
        let mut error = None;

        if let Err(e) = run_http(check, ctx, &mut assertions) {
            error = Some(e.to_string());
        } else {
            // Actions that trigger behaviour run before assertions that observe
            // it. A Kafka produce (which fires an async consumer) means the DB
            // assertion should be patient (retry) for eventual consistency.
            kafka_produce(check, ctx, &mut assertions);
            // Any async trigger (a Kafka produce, or an HTTP call that kicks off
            // background work) means the DB/Redis observers should be patient.
            let async_effect = check.request.is_some()
                || check
                    .kafka
                    .as_ref()
                    .map(|k| k.produce.is_some())
                    .unwrap_or(false);
            run_db(check, ctx, async_effect, &mut assertions);
            run_redis(check, ctx, async_effect, &mut assertions);
            kafka_expect(check, ctx, &mut assertions);
            if assertions.is_empty() {
                error = Some("check declares no assertions (add request/expect, db, ...)".into());
            }
        }

        let passed = error.is_none() && !assertions.is_empty() && assertions.iter().all(|a| a.passed);
        results.push(CheckResult {
            name: check.name.clone(),
            passed,
            assertions,
            error,
        });
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subset_matches_nested_objects() {
        let exp = json!({"status": "PENDING"});
        let act = json!({"id": 1, "status": "PENDING", "sku": "ABC"});
        assert!(matches_subset(&exp, &act));
    }

    #[test]
    fn subset_rejects_mismatch() {
        let exp = json!({"status": "PENDING"});
        let act = json!({"status": "NEW"});
        assert!(!matches_subset(&exp, &act));
    }

    #[test]
    fn subset_matches_array_prefix_and_loose_scalars() {
        let exp = json!([{"sku": "ABC"}]);
        let act = json!([{"sku": "ABC", "qty": 2}, {"sku": "XYZ"}]);
        assert!(matches_subset(&exp, &act));
        // loose scalar: yaml number vs json string
        assert!(loose_equal(&json!(201), &json!("201")));
    }

    #[test]
    fn subset_rejects_when_actual_missing_key() {
        let exp = json!({"status": "PENDING"});
        let act = json!({"other": 1});
        assert!(!matches_subset(&exp, &act));
    }
}
