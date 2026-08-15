//! OpenTelemetry `traces` assertion test against an in-process fake Jaeger query
//! API — no real backend needed, always runs. Verifies the whole assertion path:
//! query-URL construction, HTTP GET, and Jaeger-shaped response counting.

use std::sync::Arc;
use std::thread;

use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::NoworriesSpec;

/// Fake Jaeger: `/api/traces?service=…` → `{ "data": [ ... ] }` with `n` traces,
/// where `n` is taken from a `?n=` we don't send — instead it echoes a fixed
/// count for the known service and an empty list otherwise.
fn fake_jaeger() -> (u16, Arc<tiny_http::Server>) {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let port = server.server_addr().to_ip().unwrap().port();
    let s = Arc::clone(&server);
    thread::spawn(move || {
        for req in s.incoming_requests() {
            let url = req.url().to_string();
            // Two traces for orders-service, none for anything else.
            let body = if url.contains("service=orders-service") {
                r#"{"data":[{"traceID":"a"},{"traceID":"b"}]}"#
            } else {
                r#"{"data":[]}"#
            };
            let _ = req.respond(tiny_http::Response::from_string(body));
        }
    });
    (port, server)
}

#[test]
fn traces_present_passes_and_absent_fails() {
    let (port, server) = fake_jaeger();
    let base = format!("http://127.0.0.1:{port}/api/traces");

    let yaml = format!(
        r#"
version: 1
services: [postgres]
checks:
  - name: "request produced a trace"
    traces:
      query_url: "{base}"
      service: "orders-service"
      operation: "POST /orders"
      tags: {{ "http.status_code": "201" }}
      min_count: 2
  - name: "no traces for a missing service"
    traces:
      query_url: "{base}"
      service: "ghost-service"
      min_count: 1
"#
    );
    let checks = NoworriesSpec::parse(&yaml).unwrap().checks;
    let ctx = RunnerContext::new(0, vec![]);
    let results = run_checks(&checks, &ctx);

    server.unblock();
    assert!(results[0].passed, "present traces should pass: {:?}", results[0]);
    assert!(!results[1].passed, "absent traces should fail: {:?}", results[1]);
}
