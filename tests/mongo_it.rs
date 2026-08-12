//! MongoDB integration test against a REAL server. Exercises the seed
//! (insert/update/delete) -> verify (find/count) flow through `run_checks`.
//! Runs only when NOWORRIES_IT_MONGO_PORT points at MongoDB on localhost.

use noworries::lifecycle::ServiceEndpoint;
use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::{NoworriesSpec, ServiceKind};

#[test]
fn seed_then_verify_roundtrips() {
    let Some(port) = std::env::var("NOWORRIES_IT_MONGO_PORT").ok().and_then(|p| p.parse::<u16>().ok())
    else {
        eprintln!("skipping mongo IT: set NOWORRIES_IT_MONGO_PORT");
        return;
    };

    let yaml = r#"
version: 1
services: [mongodb]
checks:
  - name: "seed insert/update/delete then read back"
    mongodb:
      database: "app"
      collection: "orders"
      seed:
        - delete: { sku: "IT1" }
        - insert: { document: { sku: "IT1", status: "PENDING" } }
        - update: { filter: { sku: "IT1" }, set: { status: "SHIPPED" } }
        - insert: { document: { sku: "IT2", status: "PENDING" } }
        - delete: { sku: "IT2" }
      find: { sku: "IT1" }
      expect_doc_contains: { status: "SHIPPED" }
      expect_count: 1
"#;
    let checks = NoworriesSpec::parse(yaml).unwrap().checks;
    let endpoints = vec![ServiceEndpoint {
        service: "mongodb".into(),
        kind: ServiceKind::Mongodb,
        host_port: port,
        container_port: 27017,
    }];
    let ctx = RunnerContext::new(0, endpoints);

    let results = run_checks(&checks, &ctx);
    assert!(results[0].passed, "mongo seed+verify should pass, got {:?}", results[0]);
}
