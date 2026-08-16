//! RabbitMQ integration test against a REAL broker via the management HTTP API.
//! Declares a queue, publishes messages, then verifies the `rabbitmq` check
//! reports them. Runs only when NOWORRIES_IT_RABBITMQ_MGMT_PORT points at the
//! management API (15672) of a broker with the noworries/noworries user (as the
//! provider and ci/docker-compose.yml set up).

use std::collections::BTreeMap;
use std::time::Duration;

use noworries::lifecycle::ServiceEndpoint;
use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::{NoworriesSpec, ServiceKind};

// Basic auth for noworries:noworries.
const AUTH: &str = "Basic bm93b3JyaWVzOm5vd29ycmllcw==";

fn mgmt(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let agent = ureq::builder().timeout(Duration::from_secs(15)).build();
    let url = format!("http://127.0.0.1:{port}{path}");
    let req = agent.request(method, &url).set("Authorization", AUTH).set("Content-Type", "application/json");
    let resp = match body {
        Some(b) => req.send_string(b),
        None => req.call(),
    };
    match resp {
        Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(code, r)) => (code, r.into_string().unwrap_or_default()),
        Err(e) => panic!("rabbitmq mgmt transport error: {e}"),
    }
}

#[test]
fn rabbitmq_check_sees_published_messages() {
    let Some(mgmt_port) =
        std::env::var("NOWORRIES_IT_RABBITMQ_MGMT_PORT").ok().and_then(|p| p.parse::<u16>().ok())
    else {
        eprintln!("skipping rabbitmq IT: set NOWORRIES_IT_RABBITMQ_MGMT_PORT");
        return;
    };

    // Declare a durable queue on the default vhost ("/").
    let (code, body) =
        mgmt(mgmt_port, "PUT", "/api/queues/%2F/rabbit_it_q", Some(r#"{"durable":true}"#));
    assert!(code < 300, "declare queue failed (HTTP {code}): {body}");

    // Publish two messages via the default exchange (routing key = queue name).
    for i in 0..2 {
        let payload = format!(
            r#"{{"properties":{{}},"routing_key":"rabbit_it_q","payload":"msg{i}","payload_encoding":"string"}}"#
        );
        let (c, b) = mgmt(mgmt_port, "POST", "/api/exchanges/%2F/amq.default/publish", Some(&payload));
        assert!(c < 300, "publish failed (HTTP {c}): {b}");
    }

    let yaml = r#"
version: 1
services: [rabbitmq]
checks:
  - name: "queue exists and holds messages"
    rabbitmq:
      queue: rabbit_it_q
      min_messages: 1
"#;
    let checks = NoworriesSpec::parse(yaml).unwrap().checks;
    // The rabbitmq check reads the management port from the endpoint's aux_ports.
    let mut aux = BTreeMap::new();
    aux.insert(15672u16, mgmt_port);
    let host_port =
        std::env::var("NOWORRIES_IT_RABBITMQ_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(5672);
    let endpoints = vec![ServiceEndpoint {
        service: "rabbitmq".into(),
        kind: ServiceKind::Rabbitmq,
        host_port,
        container_port: 5672,
        aux_ports: aux,
    }];
    let ctx = RunnerContext::new(0, endpoints);

    // The check itself retries (management stats refresh on a delay), so a single
    // run_checks call is enough.
    let results = run_checks(&checks, &ctx);
    for r in &results {
        assert!(r.passed, "rabbitmq check failed: {r:?}");
    }
}
