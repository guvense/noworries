//! Phase-2 integration test: real app subprocess + real Postgres, driving the
//! actual `app::start_app` + `runner::run_checks` code paths.
//!
//! Enabled only when the environment provides a Postgres port and an app start
//! command (public registries are blocked in the build sandbox, so we use a
//! native Postgres + a stub app instead of a container + Spring Boot):
//!   NOWORRIES_IT_PGPORT       - host port of a running Postgres (user/pass/db = noworries)
//!   NOWORRIES_IT_APP_CMD      - command to start a PASSING stub app
//!   NOWORRIES_IT_APP_CMD_BUGGY - (optional) command to start a FAILING stub app

use std::path::Path;

use noworries::app::start_app;
use noworries::lifecycle::ServiceEndpoint;
use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::{AppSpec, CheckSpec, NoworriesSpec, ServiceKind};

fn endpoints(pg_port: u16) -> Vec<ServiceEndpoint> {
    vec![ServiceEndpoint {
        service: "postgres".to_string(),
        kind: ServiceKind::Postgres,
        host_port: pg_port,
        container_port: 5432,
    }]
}

fn checks() -> Vec<CheckSpec> {
    let yaml = r#"
version: 1
services:
  - postgres:16-alpine
checks:
  - name: "app health responds"
    request: { method: GET, path: /actuator/health }
    expect: { status: 200 }
  - name: "create order returns 201 and persists as PENDING"
    request: { method: POST, path: /orders, body: { sku: "ABC123", qty: 2 } }
    expect: { status: 201 }
    db:
      query: "SELECT status FROM orders WHERE sku = 'ABC123'"
      expect_row: { status: "PENDING" }
"#;
    NoworriesSpec::parse(yaml).unwrap().checks
}

fn run_scenario(app_cmd: &str, pg_port: u16) -> noworries::report::ReportSummary {
    let out_dir = std::env::temp_dir().join("noworries-rs-runner-it");
    std::fs::create_dir_all(&out_dir).unwrap();
    let app_spec = AppSpec {
        start: Some(app_cmd.to_string()),
        health: Some("/actuator/health".to_string()),
        ready_timeout: Some(20),
        ..Default::default()
    };
    // reset the orders table for a clean scenario
    let conn = format!(
        "host=127.0.0.1 port={pg_port} user=noworries password=noworries dbname=noworries"
    );
    if let Ok(mut c) = postgres::Client::connect(&conn, postgres::NoTls) {
        let _ = c.batch_execute("DROP TABLE IF EXISTS orders");
    }

    let eps = endpoints(pg_port);
    let mut app = start_app(Some(&app_spec), None, Path::new("."), &eps, &std::collections::BTreeMap::new(), &std::collections::BTreeMap::new(), &out_dir)
        .expect("app should start and become ready");
    let ctx = RunnerContext::new(app.port, eps);
    let results = run_checks(&checks(), &ctx);
    app.stop();
    // Build a summary without printing (mirror report::print_results counting).
    let passed = results.iter().filter(|r| r.passed).count();
    let failed = results.len() - passed;
    noworries::report::ReportSummary {
        ready: failed == 0 && !results.is_empty(),
        passed,
        failed,
        total: results.len(),
        results,
    }
}

#[test]
fn passing_scenario_is_ready() {
    let (pg_port, app_cmd) = match (
        std::env::var("NOWORRIES_IT_PGPORT"),
        std::env::var("NOWORRIES_IT_APP_CMD"),
    ) {
        (Ok(p), Ok(c)) if !p.is_empty() && !c.is_empty() => (p.parse::<u16>().unwrap(), c),
        _ => {
            eprintln!("skipping runner IT: set NOWORRIES_IT_PGPORT and NOWORRIES_IT_APP_CMD");
            return;
        }
    };
    let summary = run_scenario(&app_cmd, pg_port);
    assert!(summary.ready, "expected READY, got {summary:?}");
    assert_eq!(summary.failed, 0);
    assert!(summary.total >= 2);
}

#[test]
fn buggy_scenario_is_not_ready() {
    let (pg_port, app_cmd) = match (
        std::env::var("NOWORRIES_IT_PGPORT"),
        std::env::var("NOWORRIES_IT_APP_CMD_BUGGY"),
    ) {
        (Ok(p), Ok(c)) if !p.is_empty() && !c.is_empty() => (p.parse::<u16>().unwrap(), c),
        _ => {
            eprintln!("skipping buggy runner IT: set NOWORRIES_IT_PGPORT and NOWORRIES_IT_APP_CMD_BUGGY");
            return;
        }
    };
    let summary = run_scenario(&app_cmd, pg_port);
    assert!(!summary.ready, "expected NOT READY, got {summary:?}");
    assert!(summary.failed >= 1);
}
