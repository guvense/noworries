//! ClickHouse integration test against a REAL server over its HTTP interface.
//! Seeds a table + rows, then verifies the `clickhouse` check reads them. Runs
//! only when NOWORRIES_IT_CLICKHOUSE_PORT points at a ClickHouse whose
//! noworries/noworries user + `noworries` database exist (as the provider and
//! ci/docker-compose.yml set up).

use noworries::lifecycle::ServiceEndpoint;
use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::{NoworriesSpec, ServiceKind};

/// POST a statement to the `noworries` database, panicking on a server error.
fn exec(port: u16, sql: &str) {
    let agent = ureq::builder().timeout(std::time::Duration::from_secs(15)).build();
    let url = format!("http://127.0.0.1:{port}/?database=noworries");
    match agent
        .post(&url)
        .set("X-ClickHouse-User", "noworries")
        .set("X-ClickHouse-Key", "noworries")
        .send_string(sql)
    {
        Ok(_) => {}
        Err(ureq::Error::Status(code, r)) => {
            panic!("clickhouse seed HTTP {code}: {}", r.into_string().unwrap_or_default())
        }
        Err(e) => panic!("clickhouse seed transport error: {e}"),
    }
}

#[test]
fn clickhouse_check_reads_rows() {
    let Some(port) =
        std::env::var("NOWORRIES_IT_CLICKHOUSE_PORT").ok().and_then(|p| p.parse::<u16>().ok())
    else {
        eprintln!("skipping clickhouse IT: set NOWORRIES_IT_CLICKHOUSE_PORT");
        return;
    };

    exec(port, "DROP TABLE IF EXISTS ch_it_events");
    exec(port, "CREATE TABLE ch_it_events (id UInt64, sku String) ENGINE = MergeTree ORDER BY id");
    exec(port, "INSERT INTO ch_it_events VALUES (1, 'ABC'), (2, 'DEF')");

    let yaml = r#"
version: 1
services: [clickhouse]
checks:
  - name: "clickhouse row count"
    clickhouse:
      query: "SELECT count() FROM ch_it_events"
      expect_value: 2
  - name: "clickhouse first row"
    clickhouse:
      query: "SELECT id, sku FROM ch_it_events ORDER BY id"
      expect_rows: 2
      expect_row: { id: 1, sku: ABC }
"#;
    let checks = NoworriesSpec::parse(yaml).unwrap().checks;
    let endpoints = vec![ServiceEndpoint {
        service: "clickhouse".into(),
        kind: ServiceKind::Clickhouse,
        host_port: port,
        container_port: 8123,
        aux_ports: Default::default(),
    }];
    let ctx = RunnerContext::new(0, endpoints);

    let results = run_checks(&checks, &ctx);
    for r in &results {
        assert!(r.passed, "clickhouse check failed: {r:?}");
    }
}
