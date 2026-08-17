//! OpenTelemetry trace assertion. The app exports spans to a trace backend
//! (Jaeger/Tempo); noworries queries that backend's HTTP API and asserts that
//! matching traces exist. Modeled on Jaeger's query API
//! (`/api/traces?service=…&operation=…&tag=k:v`), which Tempo also emulates.

use std::time::Duration;

use serde_json::Value as Json;

use super::{fail, pass, retry_until, Assertion};
use crate::runner::{interpolate, AssertionResult, RunnerContext};
use crate::spec::{CheckSpec, TracesAssertion};

pub struct Traces;

impl Assertion for Traces {
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, retry: Duration) -> Vec<AssertionResult> {
        let Some(t) = &check.traces else { return vec![] };
        let base = interpolate(&t.query_url, &ctx.env);
        let url = build_query_url(&base, t);
        let min = t.min_count.unwrap_or(1);

        retry_until(retry, || {
            let agent = super::http_agent(Duration::from_secs(15));
            let mut get = agent.get(&url);
            for (k, v) in ctx.auth_for(check).0 {
                get = get.set(k, v);
            }
            let text = match get.call() {
                Ok(r) => r.into_string().unwrap_or_default(),
                Err(ureq::Error::Status(_, r)) => r.into_string().unwrap_or_default(),
                Err(ureq::Error::Transport(e)) => {
                    return vec![fail("traces", format!("query {url} failed: {e}"), None, None)]
                }
            };
            let parsed: Json = serde_json::from_str(&text).unwrap_or(Json::Null);
            let count = count_traces(&parsed);
            if count >= min {
                vec![pass("traces", format!("{count} trace(s) for {} (>= {min})", t.service))]
            } else {
                vec![fail(
                    "traces",
                    format!("{count} trace(s) for {}, expected >= {min}", t.service),
                    Some(Json::from(min)),
                    Some(Json::from(count)),
                )]
            }
        })
    }
}

/// Build a Jaeger-style query URL (pure — unit tested).
fn build_query_url(base: &str, t: &TracesAssertion) -> String {
    let mut q: Vec<String> = vec![format!("service={}", enc(&t.service))];
    if let Some(op) = &t.operation {
        q.push(format!("operation={}", enc(op)));
    }
    for (k, v) in &t.tags {
        // Jaeger expects tag=key:value (URL-encoded).
        q.push(format!("tag={}", enc(&format!("{k}:{v}"))));
    }
    let sep = if base.contains('?') { '&' } else { '?' };
    format!("{base}{sep}{}", q.join("&"))
}

/// Count traces in a Jaeger-style `{ "data": [ ... ] }` response, or a bare array.
fn count_traces(v: &Json) -> usize {
    if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
        return arr.len();
    }
    if let Some(arr) = v.as_array() {
        return arr.len();
    }
    0
}

/// Minimal percent-encoding for query values (space, and a few reserved chars).
fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn t() -> TracesAssertion {
        TracesAssertion {
            query_url: "http://j:16686/api/traces".into(),
            service: "orders service".into(),
            operation: Some("POST /orders".into()),
            tags: std::collections::BTreeMap::from([("http.status_code".into(), "201".into())]),
            min_count: Some(1),
        }
    }

    #[test]
    fn query_url_encodes_service_operation_tags() {
        let u = build_query_url("http://j:16686/api/traces", &t());
        assert!(u.contains("service=orders%20service"));
        assert!(u.contains("operation=POST%20%2Forders"));
        assert!(u.contains("tag=http.status_code%3A201"));
        assert!(u.contains('?'));
    }

    #[test]
    fn counts_data_array_and_bare_array() {
        assert_eq!(count_traces(&json!({"data": [1, 2, 3]})), 3);
        assert_eq!(count_traces(&json!([1, 2])), 2);
        assert_eq!(count_traces(&json!({"data": null})), 0);
    }
}
