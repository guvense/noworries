//! Schema assertion: verify a table's columns and (optionally) types via
//! `information_schema.columns`. Works against Postgres or MySQL — whichever
//! service the run declares (Postgres preferred if both are present).

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value as Json;

use super::{fail, pass, retry_until, Assertion};
use crate::runner::{mysql_conn, AssertionResult, RunnerContext};
use crate::services::{PG_DB, PG_PASSWORD, PG_USER};
use crate::spec::{CheckSpec, ServiceKind};

enum Backend {
    Postgres(u16),
    Mysql(u16),
}

pub struct Schema;

impl Assertion for Schema {
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, retry: Duration) -> Vec<AssertionResult> {
        let Some(s) = &check.schema else { return vec![] };
        let backend = ctx
            .endpoints
            .iter()
            .find(|e| e.kind == ServiceKind::Postgres)
            .map(|e| Backend::Postgres(e.host_port))
            .or_else(|| {
                ctx.endpoints
                    .iter()
                    .find(|e| e.kind == ServiceKind::Mysql)
                    .map(|e| Backend::Mysql(e.host_port))
            });
        let Some(backend) = backend else {
            return vec![fail(
                "schema",
                "schema assertion but no Postgres/MySQL service is running".into(),
                None,
                None,
            )];
        };
        retry_until(retry, || evaluate(s, &backend))
    }
}

fn evaluate(s: &crate::spec::SchemaAssertion, backend: &Backend) -> Vec<AssertionResult> {
    let loaded = match backend {
        Backend::Postgres(port) => load_columns_pg(s, *port),
        Backend::Mysql(port) => load_columns_mysql(s, *port),
    };
    let cols = match loaded {
        Ok(c) => c,
        Err(e) => return vec![fail("schema", e, None, None)],
    };

    let mut out = Vec::new();
    for name in &s.has_columns {
        let present = cols.contains_key(name);
        out.push(if present {
            pass("schema", format!("{}.{} exists", s.table, name))
        } else {
            fail("schema", format!("{}.{} missing", s.table, name), None, None)
        });
    }
    for (name, want_type) in &s.columns {
        match cols.get(name) {
            None => out.push(fail("schema", format!("{}.{} missing", s.table, name), None, None)),
            Some(actual) => {
                let passed = type_matches(want_type, actual);
                out.push(if passed {
                    pass("schema", format!("{}.{} is {actual}", s.table, name))
                } else {
                    fail(
                        "schema",
                        format!("{}.{}: expected {want_type}, is {actual}", s.table, name),
                        Some(Json::String(want_type.clone())),
                        Some(Json::String(actual.clone())),
                    )
                });
            }
        }
    }
    if out.is_empty() {
        out.push(pass("schema", format!("table {} inspected", s.table)));
    }
    out
}

fn load_columns_pg(
    s: &crate::spec::SchemaAssertion,
    port: u16,
) -> Result<BTreeMap<String, String>, String> {
    let conn = format!(
        "host=127.0.0.1 port={port} user={PG_USER} password={PG_PASSWORD} dbname={PG_DB}"
    );
    let mut client = postgres::Client::connect(&conn, postgres::NoTls)
        .map_err(|e| format!("could not connect to Postgres: {e}"))?;
    let schema = s.schema.clone().unwrap_or_else(|| "public".to_string());
    let rows = client
        .query(
            "SELECT column_name, data_type FROM information_schema.columns \
             WHERE table_schema = $1 AND table_name = $2",
            &[&schema, &s.table],
        )
        .map_err(|e| format!("information_schema query failed: {e}"))?;
    let mut map = BTreeMap::new();
    for row in rows {
        let name: String = row.get(0);
        let dtype: String = row.get(1);
        map.insert(name, dtype);
    }
    if map.is_empty() {
        return Err(format!("table \"{}.{}\" not found (no columns)", schema, s.table));
    }
    Ok(map)
}

fn load_columns_mysql(
    s: &crate::spec::SchemaAssertion,
    port: u16,
) -> Result<BTreeMap<String, String>, String> {
    use crate::services::mysql::MYSQL_DB;
    use mysql::prelude::Queryable;

    let mut conn = mysql_conn(port).map_err(|e| format!("could not connect to MySQL: {e}"))?;
    // On MySQL, `table_schema` is the database name.
    let schema = s.schema.clone().unwrap_or_else(|| MYSQL_DB.to_string());
    let rows: Vec<(String, String)> = conn
        .exec(
            "SELECT column_name, data_type FROM information_schema.columns \
             WHERE table_schema = ? AND table_name = ?",
            (&schema, &s.table),
        )
        .map_err(|e| format!("information_schema query failed: {e}"))?;
    let mut map = BTreeMap::new();
    for (name, dtype) in rows {
        map.insert(name, dtype);
    }
    if map.is_empty() {
        return Err(format!("table \"{}.{}\" not found (no columns)", schema, s.table));
    }
    Ok(map)
}

/// Loose type match: accept common aliases so a user can write `text`, `int`,
/// `bigint`, `timestamp`, `bool`, `uuid`, `json`, etc.
fn type_matches(want: &str, actual: &str) -> bool {
    let a = actual.to_lowercase();
    let w = want.to_lowercase();
    if a == w || a.contains(&w) {
        return true;
    }
    alias(&w) == alias(&a)
}

fn alias(t: &str) -> &str {
    match t {
        "int" | "int4" | "integer" | "serial" => "integer",
        "int8" | "bigint" | "bigserial" => "bigint",
        "int2" | "smallint" => "smallint",
        "bool" | "boolean" => "boolean",
        "string" | "text" | "varchar" | "character varying" => "text",
        "timestamp" | "timestamptz" | "datetime" | "timestamp without time zone" => "timestamp",
        "float" | "double" | "double precision" | "float8" => "double precision",
        "numeric" | "decimal" => "numeric",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_aliases_match() {
        assert!(type_matches("int", "integer"));
        assert!(type_matches("text", "character varying"));
        assert!(type_matches("bool", "boolean"));
        assert!(type_matches("timestamp", "timestamp without time zone"));
        assert!(!type_matches("int", "text"));
    }
}
