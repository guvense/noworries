//! Phase-4 integration test: real app + real Postgres + real Redis, driving the
//! actual runner (HTTP + DB + Redis assertions).
//!
//! Enabled when the environment provides:
//!   NOWORRIES_IT_PGPORT      - running Postgres host port (user/pass/db = noworries)
//!   NOWORRIES_IT_REDISPORT   - running Redis host port
//!   NOWORRIES_IT_APP_CMD     - command starting a stub app that writes pg + redis
//!   NOWORRIES_IT_APP_CMD_NOREDIS - (optional) stub that skips the redis write

use std::path::Path;

use noworries::app::start_app;
use noworries::lifecycle::ServiceEndpoint;
use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::{AppSpec, NoworriesSpec, ServiceKind};

fn endpoints(pg: u16, redis: u16) -> Vec<ServiceEndpoint> {
    vec![
        ServiceEndpoint { service: "postgres".into(), kind: ServiceKind::Postgres, host_port: pg, container_port: 5432 },
        ServiceEndpoint { service: "redis".into(), kind: ServiceKind::Redis, host_port: redis, container_port: 6379 },
    ]
}

fn checks() -> Vec<noworries::spec::CheckSpec> {
    let yaml = r#"
version: 1
services: [postgres:16-alpine, redis:7-alpine]
checks:
  - name: "create order persists and caches to redis"
    request: { method: POST, path: /orders, body: { sku: "ABC123", qty: 2 } }
    expect: { status: 201 }
    db:
      query: "SELECT status FROM orders WHERE sku = 'ABC123'"
      expect_row: { status: "PENDING" }
    redis:
      key: "cache:order:ABC123"
      expect_exists: true
      expect_value_contains: { status: "PENDING" }
"#;
    NoworriesSpec::parse(yaml).unwrap().checks
}

fn run(app_cmd: &str, pg: u16, redis: u16) -> (usize, usize, bool) {
    let out_dir = std::env::temp_dir().join("noworries-rs-redis-it");
    std::fs::create_dir_all(&out_dir).unwrap();
    // clean state
    let conn = format!("host=127.0.0.1 port={pg} user=noworries password=noworries dbname=noworries");
    if let Ok(mut c) = postgres::Client::connect(&conn, postgres::NoTls) {
        let _ = c.batch_execute("DROP TABLE IF EXISTS orders");
    }
    if let Ok(client) = redis::Client::open(format!("redis://127.0.0.1:{redis}/")) {
        if let Ok(mut con) = client.get_connection() {
            let _: Result<(), _> = redis::cmd("DEL").arg("cache:order:ABC123").query(&mut con);
        }
    }
    let app_spec = AppSpec {
        start: Some(app_cmd.to_string()),
        health: Some("/actuator/health".into()),
        ready_timeout: Some(20),
        ..Default::default()
    };
    let eps = endpoints(pg, redis);
    let mut app = start_app(Some(&app_spec), None, Path::new("."), &eps, &std::collections::BTreeMap::new(), &std::collections::BTreeMap::new(), &out_dir).expect("app starts");
    let ctx = RunnerContext::new(app.port, eps);
    let results = run_checks(&checks(), &ctx);
    app.stop();
    let passed = results.iter().filter(|r| r.passed).count();
    let failed = results.len() - passed;
    (passed, failed, failed == 0 && !results.is_empty())
}

fn ports() -> Option<(u16, u16)> {
    let pg = std::env::var("NOWORRIES_IT_PGPORT").ok()?.parse().ok()?;
    let redis = std::env::var("NOWORRIES_IT_REDISPORT").ok()?.parse().ok()?;
    Some((pg, redis))
}

#[test]
fn cache_written_is_ready() {
    let Some((pg, redis)) = ports() else {
        eprintln!("skipping redis IT: set NOWORRIES_IT_PGPORT + NOWORRIES_IT_REDISPORT");
        return;
    };
    let Ok(cmd) = std::env::var("NOWORRIES_IT_APP_CMD") else {
        eprintln!("skipping redis IT: set NOWORRIES_IT_APP_CMD");
        return;
    };
    let (_p, failed, ready) = run(&cmd, pg, redis);
    assert!(ready, "expected READY, failed={failed}");
}

#[test]
fn cache_missing_is_not_ready() {
    let Some((pg, redis)) = ports() else { return };
    let Ok(cmd) = std::env::var("NOWORRIES_IT_APP_CMD_NOREDIS") else {
        eprintln!("skipping redis-missing IT: set NOWORRIES_IT_APP_CMD_NOREDIS");
        return;
    };
    let (_p, failed, ready) = run(&cmd, pg, redis);
    assert!(!ready, "expected NOT READY when cache is not written");
    assert!(failed >= 1);
}
