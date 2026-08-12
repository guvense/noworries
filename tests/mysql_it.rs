//! MySQL integration test against a REAL server. Exercises the seed -> verify
//! flow (write data, then read it back) through `run_checks`. Runs only when
//! NOWORRIES_IT_MYSQL_PORT points at MySQL on localhost
//! (user/pass/db = noworries).

use noworries::lifecycle::ServiceEndpoint;
use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::{NoworriesSpec, ServiceKind};

#[test]
fn seed_then_verify_roundtrips() {
    let Some(port) = std::env::var("NOWORRIES_IT_MYSQL_PORT").ok().and_then(|p| p.parse::<u16>().ok())
    else {
        eprintln!("skipping mysql IT: set NOWORRIES_IT_MYSQL_PORT");
        return;
    };

    let yaml = r#"
version: 1
services: [mysql]
checks:
  - name: "seed then read back"
    mysql:
      seed:
        - "CREATE TABLE IF NOT EXISTS orders (id INT PRIMARY KEY AUTO_INCREMENT, sku VARCHAR(64), status VARCHAR(32))"
        - "DELETE FROM orders WHERE sku = 'IT1'"
        - "INSERT INTO orders (sku, status) VALUES ('IT1', 'PENDING')"
      query: "SELECT status FROM orders WHERE sku = 'IT1'"
      expect_row: { status: "PENDING" }
      expect_row_count: 1
"#;
    let checks = NoworriesSpec::parse(yaml).unwrap().checks;
    let endpoints = vec![ServiceEndpoint {
        service: "mysql".into(),
        kind: ServiceKind::Mysql,
        host_port: port,
        container_port: 3306,
    }];
    let ctx = RunnerContext::new(0, endpoints);

    let results = run_checks(&checks, &ctx);
    assert!(results[0].passed, "mysql seed+verify should pass, got {:?}", results[0]);
}
