//! SQL Server integration test against a REAL server. The `schema` check's
//! SQL Server backend queries `information_schema` with `sqlcmd` *inside* the
//! container (no host driver), so this test seeds the same way and drives the
//! check with the compose project/file needed to `docker compose exec`.
//!
//! Runs only when NOWORRIES_IT_MSSQL_PORT is set (a signal that CI brought the
//! `mssql` service up). The compose project/file default to the heavy CI stack
//! but can be overridden via NOWORRIES_IT_COMPOSE_{PROJECT,FILE}.

use noworries::lifecycle::{compose_exec, ServiceEndpoint};
use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::{NoworriesSpec, ServiceKind};

const SA_PW: &str = "Noworries!Pass1";

fn compose_project() -> String {
    std::env::var("NOWORRIES_IT_COMPOSE_PROJECT").unwrap_or_else(|_| "noworries-ci-heavy".into())
}
fn compose_file() -> String {
    std::env::var("NOWORRIES_IT_COMPOSE_FILE").unwrap_or_else(|_| "ci/docker-compose.heavy.yml".into())
}

fn sqlcmd(query: &str) {
    let out = compose_exec(
        &compose_file(),
        &compose_project(),
        "mssql",
        &["/opt/mssql-tools18/bin/sqlcmd", "-S", "localhost", "-U", "sa", "-P", SA_PW, "-C", "-b", "-Q", query],
    );
    assert_eq!(out.code, 0, "sqlcmd seed failed (code {}):\nstderr: {}\nstdout: {}", out.code, out.stderr, out.stdout);
}

#[test]
fn mssql_schema_check_reads_columns() {
    if std::env::var("NOWORRIES_IT_MSSQL_PORT").is_err() {
        eprintln!("skipping mssql IT: set NOWORRIES_IT_MSSQL_PORT");
        return;
    }

    // Seed a table in the default (master) database — the check reads it with no
    // `schema` (database) qualifier.
    sqlcmd(
        "IF OBJECT_ID('mssql_it_orders','U') IS NOT NULL DROP TABLE mssql_it_orders; \
         CREATE TABLE mssql_it_orders (id bigint PRIMARY KEY, sku nvarchar(64), qty int, created_at datetime2)",
    );

    let yaml = r#"
version: 1
services: [mssql]
checks:
  - name: "mssql orders columns"
    schema:
      table: mssql_it_orders
      has_columns: [id, sku, qty]
      columns: { sku: nvarchar, qty: int, created_at: timestamp }
"#;
    let checks = NoworriesSpec::parse(yaml).unwrap().checks;
    // host_port is unused by the SQL Server backend (it execs into the
    // container), but the endpoint must exist so the check finds its service.
    let endpoints = vec![ServiceEndpoint {
        service: "mssql".into(),
        kind: ServiceKind::Mssql,
        host_port: 1433,
        container_port: 1433,
        aux_ports: Default::default(),
    }];
    let mut ctx = RunnerContext::new(0, endpoints);
    ctx.compose_project = Some(compose_project());
    ctx.compose_file = Some(compose_file());

    let results = run_checks(&checks, &ctx);
    for r in &results {
        assert!(r.passed, "mssql schema check failed: {r:?}");
    }
}
