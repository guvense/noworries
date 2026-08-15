//! Postgres schema-assertion integration test against a REAL database. Creates a
//! table and verifies the `schema` check reads its columns/types. Runs only when
//! NOWORRIES_IT_PG_PORT points at a Postgres with noworries/noworries/noworries.

use noworries::lifecycle::ServiceEndpoint;
use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::{NoworriesSpec, ServiceKind};

#[test]
fn schema_check_reads_columns_and_types() {
    let Some(port) = std::env::var("NOWORRIES_IT_PG_PORT").ok().and_then(|p| p.parse::<u16>().ok())
    else {
        eprintln!("skipping schema IT: set NOWORRIES_IT_PG_PORT");
        return;
    };

    let conn = format!("host=127.0.0.1 port={port} user=noworries password=noworries dbname=noworries");
    let mut client = postgres::Client::connect(&conn, postgres::NoTls).unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS schema_it_orders; \
             CREATE TABLE schema_it_orders (id bigserial primary key, sku text, qty integer, created_at timestamptz);",
        )
        .unwrap();

    let yaml = r#"
version: 1
services: [postgres]
checks:
  - name: "orders table has the right columns"
    schema:
      table: schema_it_orders
      has_columns: [id, sku, qty]
      columns: { sku: text, qty: int, created_at: timestamp }
"#;
    let checks = NoworriesSpec::parse(yaml).unwrap().checks;
    let endpoints = vec![ServiceEndpoint {
        service: "postgres".into(),
        kind: ServiceKind::Postgres,
        host_port: port,
        container_port: 5432,
    }];
    let ctx = RunnerContext::new(0, endpoints);

    let results = run_checks(&checks, &ctx);
    assert!(results[0].passed, "schema check should pass, got {:?}", results[0]);
}

#[test]
fn schema_check_works_on_mysql() {
    let Some(port) = std::env::var("NOWORRIES_IT_MYSQL_PORT").ok().and_then(|p| p.parse::<u16>().ok())
    else {
        eprintln!("skipping mysql schema IT: set NOWORRIES_IT_MYSQL_PORT");
        return;
    };

    use mysql::prelude::Queryable;
    let url = format!("mysql://noworries:noworries@127.0.0.1:{port}/noworries");
    let mut conn = mysql::Conn::new(mysql::Opts::from_url(&url).unwrap()).unwrap();
    conn.query_drop("DROP TABLE IF EXISTS schema_it_orders").unwrap();
    conn.query_drop(
        "CREATE TABLE schema_it_orders (id bigint primary key, sku varchar(64), qty int)",
    )
    .unwrap();

    let yaml = r#"
version: 1
services: [mysql]
checks:
  - name: "mysql orders columns"
    schema:
      table: schema_it_orders
      has_columns: [id, sku, qty]
      columns: { sku: varchar, qty: int }
"#;
    let checks = NoworriesSpec::parse(yaml).unwrap().checks;
    // Only a MySQL endpoint → the schema check uses MySQL.
    let endpoints = vec![ServiceEndpoint {
        service: "mysql".into(),
        kind: ServiceKind::Mysql,
        host_port: port,
        container_port: 3306,
    }];
    let ctx = RunnerContext::new(0, endpoints);

    let results = run_checks(&checks, &ctx);
    assert!(results[0].passed, "mysql schema check should pass, got {:?}", results[0]);
}
