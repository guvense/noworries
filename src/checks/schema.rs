//! Schema assertion: verify a table's columns and (optionally) types via
//! `information_schema.columns`. Works against Postgres, MySQL, or SQL Server —
//! whichever service the run declares (Postgres preferred, then MySQL, then
//! SQL Server).
//!
//! Postgres and MySQL connect from the host with their lightweight sync Rust
//! drivers (already in the tree). SQL Server has no equally-light driver — the
//! mainstream one is async and pulls TLS — so instead of a host connection its
//! backend runs `sqlcmd` *inside* the container via `docker compose exec`. Same
//! `information_schema` query, same comparison logic; only the transport differs.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value as Json;

use super::{fail, pass, retry_until, Assertion};
use crate::runner::{mysql_conn, AssertionResult, RunnerContext};
use crate::spec::{CheckSpec, ServiceKind};

enum Backend {
    /// Any Postgres-wire service (postgres, timescaledb, cockroachdb) — carries
    /// the resolved connection string, since CockroachDB authenticates
    /// differently (`root`, no password, `defaultdb`).
    Postgres(String),
    Mysql(u16),
    /// SQL Server, queried via `docker compose exec <service> sqlcmd ...`.
    Mssql { service: String, project: String, file: String },
}

pub struct Schema;

impl Assertion for Schema {
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, retry: Duration) -> Vec<AssertionResult> {
        let Some(s) = &check.schema else { return vec![] };
        let backend = crate::runner::pg_wire_target(&ctx.endpoints)
            .map(|(_, conn)| Backend::Postgres(conn))
            .or_else(|| {
                ctx.endpoints
                    .iter()
                    .find(|e| e.kind == ServiceKind::Mysql)
                    .map(|e| Backend::Mysql(e.host_port))
            })
            .or_else(|| mssql_backend(ctx));
        let Some(backend) = backend else {
            return vec![fail(
                "schema",
                "schema assertion but no Postgres-wire/MySQL/SQL Server service is running".into(),
                None,
                None,
            )];
        };
        retry_until(retry, || evaluate(s, &backend))
    }
}

/// Build the SQL Server backend from a declared `mssql` endpoint + the compose
/// context needed to exec into it. Returns `None` (so `run` reports "no
/// service") if the compose handle is absent, e.g. in a unit-test context.
fn mssql_backend(ctx: &RunnerContext) -> Option<Backend> {
    let ep = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Mssql)?;
    let project = ctx.compose_project.clone()?;
    let file = ctx.compose_file.clone()?;
    Some(Backend::Mssql { service: ep.service.clone(), project, file })
}

fn evaluate(s: &crate::spec::SchemaAssertion, backend: &Backend) -> Vec<AssertionResult> {
    let loaded = match backend {
        Backend::Postgres(conn) => load_columns_pg(s, conn),
        Backend::Mysql(port) => load_columns_mysql(s, *port),
        Backend::Mssql { service, project, file } => load_columns_mssql(s, service, project, file),
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
    conn: &str,
) -> Result<BTreeMap<String, String>, String> {
    let mut client = postgres::Client::connect(conn, postgres::NoTls)
        .map_err(|e| format!("could not connect over the Postgres wire: {e}"))?;
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

/// Load a table's columns from SQL Server by running `sqlcmd` inside the
/// container. `s.schema`, if set, is treated as the **database** name (SQL
/// Server splits catalog/schema; most apps care about the database), applied
/// via a `USE` clause. Columns come from `information_schema.columns`.
fn load_columns_mssql(
    s: &crate::spec::SchemaAssertion,
    service: &str,
    project: &str,
    file: &str,
) -> Result<BTreeMap<String, String>, String> {
    use crate::services::mssql::MSSQL_SA_PASSWORD;

    // Single-quotes in an identifier are escaped by doubling for the SQL string.
    let table = s.table.replace('\'', "''");
    let use_clause = match &s.schema {
        Some(db) => format!("USE [{}]; ", db.replace(']', "]]")),
        None => String::new(),
    };
    let query = format!(
        "SET NOCOUNT ON; {use_clause}SELECT column_name, data_type \
         FROM information_schema.columns WHERE table_name = '{table}'"
    );
    // `-h -1` no headers, `-W` trim, `-s |` delimiter, `-C` trust self-signed
    // cert, `-b` return non-zero on SQL error. argv (not a shell), so no quoting.
    let argv = [
        "/opt/mssql-tools18/bin/sqlcmd",
        "-S",
        "localhost",
        "-U",
        "sa",
        "-P",
        MSSQL_SA_PASSWORD,
        "-C",
        "-b",
        "-h",
        "-1",
        "-W",
        "-s",
        "|",
        "-Q",
        &query,
    ];
    let out = crate::lifecycle::compose_exec(file, project, service, &argv);
    if out.code != 0 {
        let detail = if out.stderr.trim().is_empty() { out.stdout.trim() } else { out.stderr.trim() };
        return Err(format!("sqlcmd failed: {detail}"));
    }
    let map = parse_sqlcmd_columns(&out.stdout);
    if map.is_empty() {
        let where_db = s.schema.as_deref().map(|d| format!(" in database \"{d}\"")).unwrap_or_default();
        return Err(format!("table \"{}\" not found (no columns){where_db}", s.table));
    }
    Ok(map)
}

/// Parse `sqlcmd -h -1 -W -s "|"` output into `column -> data_type`. Lines are
/// `name|type`; blank lines and any stray status text (no `|`) are skipped.
fn parse_sqlcmd_columns(stdout: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((name, dtype)) = line.split_once('|') {
            let name = name.trim();
            let dtype = dtype.trim();
            if !name.is_empty() && !dtype.is_empty() {
                map.insert(name.to_string(), dtype.to_string());
            }
        }
    }
    map
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
        "bool" | "boolean" | "bit" => "boolean",
        "string" | "text" | "varchar" | "nvarchar" | "character varying" => "text",
        "timestamp" | "timestamptz" | "datetime" | "datetime2" | "timestamp without time zone" => {
            "timestamp"
        }
        "float" | "double" | "double precision" | "float8" => "double precision",
        "numeric" | "decimal" => "numeric",
        "uuid" | "uniqueidentifier" => "uuid",
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

    #[test]
    fn sqlserver_type_aliases_match() {
        assert!(type_matches("bool", "bit"));
        assert!(type_matches("text", "nvarchar"));
        assert!(type_matches("timestamp", "datetime2"));
        assert!(type_matches("uuid", "uniqueidentifier"));
    }

    #[test]
    fn parses_sqlcmd_delimited_output() {
        // `-h -1 -W -s "|"` output: no header, one `name|type` line per column,
        // with a trailing blank line sqlcmd tends to emit.
        let stdout = "id|bigint\nsku|nvarchar\nstatus|nvarchar\n\n";
        let cols = parse_sqlcmd_columns(stdout);
        assert_eq!(cols.len(), 3);
        assert_eq!(cols.get("id").map(String::as_str), Some("bigint"));
        assert_eq!(cols.get("sku").map(String::as_str), Some("nvarchar"));
    }

    #[test]
    fn sqlcmd_parser_skips_non_column_lines() {
        // A stray status line without a delimiter must not become a column.
        let stdout = "Changed database context to 'app'.\nid|int\n";
        let cols = parse_sqlcmd_columns(stdout);
        assert_eq!(cols.len(), 1);
        assert_eq!(cols.get("id").map(String::as_str), Some("int"));
    }
}
