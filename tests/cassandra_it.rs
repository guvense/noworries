//! Cassandra integration test against a REAL server. The `cassandra` check runs
//! CQL via `cqlsh` *inside* the container (no host driver), so this test seeds
//! the same way and drives the check with the compose project/file needed to
//! `docker compose exec`.
//!
//! Runs only when NOWORRIES_IT_CASSANDRA_PORT is set. The compose project/file
//! default to the heavy CI stack, overridable via NOWORRIES_IT_COMPOSE_{PROJECT,FILE}.

use noworries::lifecycle::{compose_exec, ServiceEndpoint};
use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::{NoworriesSpec, ServiceKind};

fn compose_project() -> String {
    std::env::var("NOWORRIES_IT_COMPOSE_PROJECT").unwrap_or_else(|_| "noworries-ci-heavy".into())
}
fn compose_file() -> String {
    std::env::var("NOWORRIES_IT_COMPOSE_FILE").unwrap_or_else(|_| "ci/docker-compose.heavy.yml".into())
}

fn cql(stmt: &str) {
    let out = compose_exec(&compose_file(), &compose_project(), "cassandra", &["cqlsh", "-e", stmt]);
    assert_eq!(out.code, 0, "cqlsh seed failed (code {}):\nstderr: {}\nstdout: {}", out.code, out.stderr, out.stdout);
}

#[test]
fn cassandra_check_reads_rows() {
    if std::env::var("NOWORRIES_IT_CASSANDRA_PORT").is_err() {
        eprintln!("skipping cassandra IT: set NOWORRIES_IT_CASSANDRA_PORT");
        return;
    }

    cql(
        "CREATE KEYSPACE IF NOT EXISTS app WITH replication = {'class':'SimpleStrategy','replication_factor':1}; \
         CREATE TABLE IF NOT EXISTS app.orders (id bigint PRIMARY KEY, sku text); \
         INSERT INTO app.orders (id, sku) VALUES (42, 'ABC')",
    );

    let yaml = r#"
version: 1
services: [cassandra]
checks:
  - name: "cassandra row count"
    cassandra:
      query: "SELECT count(*) FROM app.orders WHERE id = 42"
      expect_value: 1
  - name: "cassandra row contents"
    cassandra:
      query: "SELECT id, sku FROM app.orders WHERE id = 42"
      expect_row: { id: 42, sku: ABC }
"#;
    let checks = NoworriesSpec::parse(yaml).unwrap().checks;
    let endpoints = vec![ServiceEndpoint {
        service: "cassandra".into(),
        kind: ServiceKind::Cassandra,
        host_port: 9042,
        container_port: 9042,
        aux_ports: Default::default(),
    }];
    let mut ctx = RunnerContext::new(0, endpoints);
    ctx.compose_project = Some(compose_project());
    ctx.compose_file = Some(compose_file());

    let results = run_checks(&checks, &ctx);
    for r in &results {
        assert!(r.passed, "cassandra check failed: {r:?}");
    }
}
