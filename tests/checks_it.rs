//! In-process verification of the extensible check types that talk HTTP/WS —
//! no Docker needed, always runs. A tiny fake app server answers /graphql,
//! /metrics, /events and /order; a tiny WebSocket server answers ws://.

use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::NoworriesSpec;

/// Start a fake app HTTP server routing the paths the checks hit. Returns
/// (port, server) — keep the server alive for the test, then `unblock()`.
fn fake_app() -> (u16, Arc<tiny_http::Server>) {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let port = server.server_addr().to_ip().unwrap().port();
    let s = Arc::clone(&server);
    thread::spawn(move || {
        for req in s.incoming_requests() {
            let path = req.url().split('?').next().unwrap_or("").to_string();
            let (code, body): (u16, String) = match path.as_str() {
                "/graphql" => (200, r#"{"data":{"order":{"status":"PENDING"}}}"#.into()),
                "/metrics" => (
                    200,
                    "# HELP x\nhttp_requests_total{status=\"200\"} 5\nup 1\n".into(),
                ),
                "/events" => (200, "data: {\"type\":\"OrderCreated\",\"id\":\"E1\"}\n\n".into()),
                "/order" => (200, r#"{"id":123,"sku":"ABC","status":"PENDING"}"#.into()),
                _ => (404, "{}".into()),
            };
            let resp = tiny_http::Response::from_string(body).with_status_code(code);
            let _ = req.respond(resp);
        }
    });
    (port, server)
}

#[test]
fn graphql_metrics_sse_run_against_fake_app() {
    let (port, server) = fake_app();

    let yaml = r#"
version: 1
services: [postgres]
checks:
  - name: "graphql query"
    graphql:
      path: /graphql
      query: "query { order(id: 1) { status } }"
      expect_data: { order: { status: "PENDING" } }
  - name: "prometheus metric"
    metrics:
      path: /metrics
      metric: http_requests_total
      labels: { status: "200" }
      expect: ">= 1"
  - name: "sse event"
    sse:
      path: /events
      contains: { type: "OrderCreated" }
      timeout_ms: 3000
"#;
    let checks = NoworriesSpec::parse(yaml).unwrap().checks;
    let ctx = RunnerContext::new(port, vec![]);
    let results = run_checks(&checks, &ctx);

    server.unblock();
    for r in &results {
        assert!(r.passed, "{} should pass: {:?}", r.name, r);
    }
}

#[test]
fn snapshot_updates_then_matches() {
    let (port, server) = fake_app();
    let tmp = std::env::temp_dir().join(format!("nw-snap-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    let yaml = r#"
version: 1
services: [postgres]
checks:
  - name: "order response snapshot"
    request: { method: GET, path: /order }
    snapshot:
      file: order.snap.json
      ignore: [ "$.id" ]
"#;
    let checks = NoworriesSpec::parse(yaml).unwrap().checks;

    // First pass: write the golden.
    let mut ctx = RunnerContext::new(port, vec![]);
    ctx.dir = tmp.clone();
    ctx.update_snapshots = true;
    let r1 = run_checks(&checks, &ctx);
    assert!(r1[0].passed, "update pass: {:?}", r1[0]);
    assert!(tmp.join("order.snap.json").exists());

    // Second pass: must match (id ignored, so a changing id wouldn't matter).
    let mut ctx2 = RunnerContext::new(port, vec![]);
    ctx2.dir = tmp.clone();
    let r2 = run_checks(&checks, &ctx2);
    assert!(r2[0].passed, "match pass: {:?}", r2[0]);

    server.unblock();
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn websocket_receives_expected_message() {
    // Tiny WS server: accept one connection, send a JSON text message.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            if let Ok(mut ws) = tungstenite::accept(stream) {
                let _ = ws.send(tungstenite::Message::Text(
                    r#"{"type":"OrderCreated","id":"W1"}"#.into(),
                ));
                // give the client time to read before closing
                std::thread::sleep(std::time::Duration::from_millis(200));
                let _ = ws.close(None);
            }
        }
    });

    let yaml = format!(
        r#"
version: 1
services: [postgres]
checks:
  - name: "ws message"
    websocket:
      url: "ws://127.0.0.1:{port}/"
      expect_message: {{ type: "OrderCreated" }}
      timeout_ms: 3000
"#
    );
    let checks = NoworriesSpec::parse(&yaml).unwrap().checks;
    let ctx = RunnerContext::new(0, vec![]);
    let results = run_checks(&checks, &ctx);
    assert!(results[0].passed, "ws check should pass: {:?}", results[0]);
}

/// Regression: an acting assertion (GraphQL, gRPC, a WebSocket `send`) must run
/// BEFORE the observers in the same check.
///
/// It used to run last, so a spec that triggered over GraphQL and observed the
/// effect elsewhere could never pass in one piece — the observer looked for the
/// effect, exhausted its retry budget, and only then did the mutation fire. The
/// workaround was splitting one check into two.
///
/// The observer here is `logs:`, which needs no container: the fake app appends
/// a line to the app log when `/graphql` is hit, and the check asserts that line
/// is present. Ordering is the only thing that decides the outcome.
#[test]
fn graphql_trigger_runs_before_observers_in_the_same_check() {
    let log = std::env::temp_dir().join(format!("nw-order-{}.log", std::process::id()));
    let _ = std::fs::remove_file(&log);
    std::fs::write(&log, "app started\n").unwrap();

    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let port = server.server_addr().to_ip().unwrap().port();
    let s = Arc::clone(&server);
    let log_for_app = log.clone();
    thread::spawn(move || {
        for req in s.incoming_requests() {
            let path = req.url().split('?').next().unwrap_or("").to_string();
            if path == "/graphql" {
                // The app's side effect: a line in its log.
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&log_for_app)
                    .unwrap();
                writeln!(f, "MUTATION_APPLIED sku=ABC").unwrap();
            }
            let body = r#"{"data":{"createOrder":{"sku":"ABC"}}}"#;
            let _ = req.respond(tiny_http::Response::from_string(body).with_status_code(200));
        }
    });

    let yaml = r#"
version: 1
services: [postgres]
checks:
  - name: "mutation, then observe its effect"
    graphql:
      path: /graphql
      query: "mutation { createOrder(sku: \"ABC\") { sku } }"
      expect_data: { createOrder: { sku: "ABC" } }
    logs:
      contains: ["MUTATION_APPLIED sku=ABC"]
"#;
    let checks = NoworriesSpec::parse(yaml).unwrap().checks;
    let mut ctx = RunnerContext::new(port, vec![]);
    ctx.app_log_path = Some(log.clone());
    let results = run_checks(&checks, &ctx);

    server.unblock();
    let _ = std::fs::remove_file(&log);

    assert!(
        results[0].passed,
        "the mutation must fire before the log observer reads: {:?}",
        results[0]
    );
}
