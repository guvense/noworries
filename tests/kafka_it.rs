//! Kafka integration test against a REAL broker. This exercises both the
//! producer (`kafka.produce`) and the consumer (`kafka.expect_message`) paths
//! through `run_checks` — the consumer path is where the "offset storage" bug
//! lived, which unit tests / compile checks could never catch. It runs only
//! when NOWORRIES_IT_KAFKA_PORT points at a broker advertised on localhost.

use noworries::lifecycle::ServiceEndpoint;
use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::{NoworriesSpec, ServiceKind};

#[test]
fn produce_then_expect_message_roundtrips() {
    let Some(port) = std::env::var("NOWORRIES_IT_KAFKA_PORT").ok().and_then(|p| p.parse::<u16>().ok())
    else {
        eprintln!("skipping kafka IT: set NOWORRIES_IT_KAFKA_PORT to a broker advertised on localhost");
        return;
    };

    let yaml = r#"
version: 1
services: [kafka]
checks:
  - name: "produce then consume roundtrips"
    kafka:
      produce:
        topic: "noworries-ci"
        message: { type: "Ping", n: 1 }
      expect_message:
        topic: "noworries-ci"
        contains: { type: "Ping" }
        timeout_ms: 20000
"#;
    let checks = NoworriesSpec::parse(yaml).unwrap().checks;
    let endpoints = vec![ServiceEndpoint {
        service: "kafka".into(),
        kind: ServiceKind::Kafka,
        host_port: port,
        container_port: 9092,
        aux_ports: Default::default(),
    }];
    let ctx = RunnerContext::new(0, endpoints);

    let results = run_checks(&checks, &ctx);
    assert!(
        results[0].passed,
        "kafka produce+consume should pass, got {:?}",
        results[0]
    );
}
