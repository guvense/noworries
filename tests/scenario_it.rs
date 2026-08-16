//! Edge-case scenario integration test against a REAL Kafka broker. Exercises
//! the concurrent multi-producer path (`produce_plan`) that unit tests can't:
//! several producer threads flooding a topic at once. Runs only when
//! NOWORRIES_IT_KAFKA_PORT points at a broker advertised on localhost.

use noworries::lifecycle::ServiceEndpoint;
use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::{NoworriesSpec, ServiceKind};

#[test]
fn burst_scenario_floods_kafka_and_messages_are_consumable() {
    let Some(port) = std::env::var("NOWORRIES_IT_KAFKA_PORT").ok().and_then(|p| p.parse::<u16>().ok())
    else {
        eprintln!("skipping scenario IT: set NOWORRIES_IT_KAFKA_PORT to a broker advertised on localhost");
        return;
    };

    // A burst of 40 messages across 4 concurrent producers to a unique topic,
    // then confirm the flooded messages are consumable (verifies the real
    // concurrent produce path end to end).
    let yaml = r#"
version: 1
services: [kafka]
checks:
  - name: "burst floods the topic"
    scenario:
      kind: burst
      count: 40
      concurrency: 4
      kafka:
        topic: "noworries-scenario-it"
        key: "k-${seq}"
        message: { id: "${uuid}", n: "${seq}" }
    kafka:
      expect_message:
        topic: "noworries-scenario-it"
        contains: { n: "0" }
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
        "burst scenario + consume should pass, got {:?}",
        results[0]
    );
    // The scenario assertion should report all 40 produced.
    let scenario = results[0]
        .assertions
        .iter()
        .find(|a| a.kind == "scenario")
        .expect("a scenario assertion should be present");
    assert!(
        scenario.passed && scenario.message.contains("40"),
        "scenario should produce 40 messages, got: {}",
        scenario.message
    );
}
