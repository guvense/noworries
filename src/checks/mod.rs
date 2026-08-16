//! Extensible check assertions.
//!
//! The original assertions (http, db, kafka, …) are hard-wired in [`crate::runner`].
//! This module is the **open/closed extension point** for newer protocol/observability
//! assertions: each type is a small [`Assertion`] impl in its own file, added to
//! [`registry`] with a single line — no change to the runner's core loop. Mirrors
//! the `framework/`, `services/`, and `edgecases/` trait modules.
//!
//! Every assertion inspects its own optional field on the [`CheckSpec`] and
//! returns an empty vec when the check doesn't declare it.

use std::time::{Duration, Instant};

use serde_json::Value as Json;

use crate::runner::{interpolate, AssertionResult, RunnerContext};
use crate::spec::CheckSpec;

pub mod cassandra;
pub mod clickhouse;
pub mod graphql;
pub mod rabbitmq;
pub mod grpc;
pub mod metrics;
pub mod schema;
pub mod snapshot;
pub mod sse;
pub mod traces;
pub mod websocket;

/// The interface every extensible assertion implements.
pub trait Assertion {
    /// Evaluate this assertion type for `check`. Return an empty vec if the check
    /// doesn't declare it. `retry` is the eventual-consistency budget.
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, retry: Duration) -> Vec<AssertionResult>;
}

/// Registry / composition root: all extensible assertion types. Snapshot runs
/// last because it reads the HTTP response captured by the check's `request`.
pub fn registry() -> Vec<Box<dyn Assertion>> {
    vec![
        Box::new(graphql::GraphQl),
        Box::new(metrics::Metrics),
        Box::new(schema::Schema),
        Box::new(clickhouse::Clickhouse),
        Box::new(rabbitmq::Rabbitmq),
        Box::new(cassandra::Cassandra),
        Box::new(sse::Sse),
        Box::new(websocket::WebSocket),
        Box::new(grpc::Grpc),
        Box::new(traces::Traces),
        Box::new(snapshot::Snapshot),
    ]
}

/// Run every registry assertion for a check, appending results to `out`.
pub fn run_all(check: &CheckSpec, ctx: &RunnerContext, retry: Duration, out: &mut Vec<AssertionResult>) {
    for a in registry() {
        out.extend(a.run(check, ctx, retry));
    }
}

// ---- shared helpers ----

pub(crate) fn pass(kind: &str, message: String) -> AssertionResult {
    AssertionResult { kind: kind.into(), passed: true, message, expected: None, actual: None }
}

pub(crate) fn fail(kind: &str, message: String, expected: Option<Json>, actual: Option<Json>) -> AssertionResult {
    AssertionResult { kind: kind.into(), passed: false, message, expected, actual }
}

/// YAML value → JSON value (for comparing declared expectations uniformly).
pub(crate) fn yaml_json(v: &serde_yaml::Value) -> Json {
    serde_json::to_value(v).unwrap_or(Json::Null)
}

/// Equal as JSON, or equal as scalar string forms. Datastores that speak over
/// text/JSON transports blur the string/number line (ClickHouse renders 64-bit
/// ints as JSON strings; `cqlsh`/`SELECT JSON` and delimited CLI output are all
/// text), so `1` must match `"1"`.
pub(crate) fn loose_scalar_eq(actual: &Json, expected: &Json) -> bool {
    actual == expected || scalar_str(actual) == scalar_str(expected)
}

pub(crate) fn scalar_str(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        Json::Number(n) => n.to_string(),
        Json::Bool(b) => b.to_string(),
        Json::Null => "null".to_string(),
        other => other.to_string(),
    }
}

/// Resolve a `path`-or-absolute-URL against the app, with `${VAR}` interpolation.
/// A relative path becomes `http://127.0.0.1:<app_port><path>`; an absolute URL
/// (or `${VAR}` that expands to one) is used as-is. `schemes` are the accepted
/// absolute prefixes (e.g. `["http://","https://"]` or `["ws://","wss://"]`).
pub(crate) fn resolve_target(raw: &str, ctx: &RunnerContext, schemes: &[&str]) -> String {
    let s = interpolate(raw, &ctx.env);
    if schemes.iter().any(|p| s.starts_with(p)) {
        s
    } else {
        format!("http://127.0.0.1:{}{}", ctx.app_port, s)
    }
}

pub(crate) fn http_agent(timeout: Duration) -> ureq::Agent {
    ureq::builder().timeout(timeout).build()
}

/// Retry `eval` until it returns all-passing or `retry` elapses; returns the last
/// result set. Use for assertions observing an eventually-consistent effect.
pub(crate) fn retry_until<F>(retry: Duration, mut eval: F) -> Vec<AssertionResult>
where
    F: FnMut() -> Vec<AssertionResult>,
{
    let deadline = Instant::now() + retry;
    let mut last = eval();
    while !last.iter().all(|a| a.passed) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(300));
        last = eval();
    }
    last
}
