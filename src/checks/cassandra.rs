//! Cassandra assertion: run a CQL query via `cqlsh` *inside* the container
//! (`docker compose exec`) — no native CQL driver, consistent with the SQL
//! Server backend. Also covers ScyllaDB, which ships the same `cqlsh`.
//!
//! For parseable output the query is rewritten from `SELECT ...` to
//! `SELECT JSON ...`, so `cqlsh` prints one JSON object per row. The bordered
//! table, header, and `(N rows)` footer `cqlsh` also prints are ignored: only
//! lines that parse as JSON objects are treated as rows.

use std::time::Duration;

use serde_json::Value as Json;

use super::{fail, loose_scalar_eq, pass, retry_until, yaml_json, Assertion};
use crate::runner::{AssertionResult, RunnerContext};
use crate::spec::{CassandraAssertion, CheckSpec, ServiceKind};

pub struct Cassandra;

impl Assertion for Cassandra {
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, retry: Duration) -> Vec<AssertionResult> {
        let Some(c) = &check.cassandra else { return vec![] };
        let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Cassandra) else {
            return vec![fail(
                "cassandra",
                "cassandra assertion but no Cassandra service is running".into(),
                None,
                None,
            )];
        };
        let (Some(project), Some(file)) = (&ctx.compose_project, &ctx.compose_file) else {
            return vec![fail(
                "cassandra",
                "cassandra assertion needs the compose context to exec cqlsh".into(),
                None,
                None,
            )];
        };
        let service = ep.service.clone();
        retry_until(retry, || match run_cqlsh(file, project, &service, &c.query) {
            Ok(rows) => check_result(&rows, c),
            Err(e) => vec![fail("cassandra", e, None, None)],
        })
    }
}

/// Run `cqlsh -e <query>` in the container and return the parsed rows. `SELECT`
/// queries are rewritten to `SELECT JSON` so each row prints as a JSON object.
fn run_cqlsh(file: &str, project: &str, service: &str, query: &str) -> Result<Vec<Json>, String> {
    let effective = as_json_select(query);
    let out = crate::lifecycle::compose_exec(file, project, service, &["cqlsh", "-e", &effective]);
    if out.code != 0 {
        let detail = if out.stderr.trim().is_empty() { out.stdout.trim() } else { out.stderr.trim() };
        return Err(format!("cqlsh failed: {detail}"));
    }
    Ok(parse_json_rows(&out.stdout))
}

/// Rewrite `SELECT <rest>` to `SELECT JSON <rest>` (leaving an existing
/// `SELECT JSON`, or a non-SELECT statement, untouched).
fn as_json_select(query: &str) -> String {
    let q = query.trim();
    let upper = q.to_uppercase();
    if upper.starts_with("SELECT JSON ") {
        q.to_string()
    } else if let Some(rest) = upper.strip_prefix("SELECT ") {
        // Preserve the original (non-uppercased) tail after "SELECT ".
        let tail = &q[q.len() - rest.len()..];
        format!("SELECT JSON {tail}")
    } else {
        q.to_string()
    }
}

/// Extract rows from `cqlsh` output: every line that parses as a JSON object is
/// a row; borders, headers, blank lines, and the `(N rows)` footer are skipped.
fn parse_json_rows(stdout: &str) -> Vec<Json> {
    let mut rows = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Json>(line) {
            if v.is_object() {
                rows.push(v);
            }
        }
    }
    rows
}

/// Evaluate configured expectations against parsed rows. Pure (no IO).
fn check_result(rows: &[Json], c: &CassandraAssertion) -> Vec<AssertionResult> {
    let mut out = Vec::new();

    if let Some(want) = c.expect_rows {
        if rows.len() == want {
            out.push(pass("cassandra", format!("query returned {want} row(s)")));
        } else {
            out.push(fail(
                "cassandra",
                format!("expected {want} row(s), got {}", rows.len()),
                Some(Json::from(want)),
                Some(Json::from(rows.len())),
            ));
        }
    }

    if let Some(expect_row) = &c.expect_row {
        out.push(match_first_row(rows, &yaml_json(expect_row)));
    }

    if let Some(expect_value) = &c.expect_value {
        out.push(match_scalar(rows, &yaml_json(expect_value)));
    }

    if out.is_empty() {
        out.push(pass("cassandra", format!("query ran, {} row(s)", rows.len())));
    }
    out
}

fn match_first_row(rows: &[Json], expected: &Json) -> AssertionResult {
    let Some(first) = rows.first() else {
        return fail("cassandra", "expect_row but query returned no rows".into(), None, None);
    };
    let Some(want) = expected.as_object() else {
        return fail("cassandra", "expect_row must be a column→value map".into(), None, None);
    };
    for (col, wv) in want {
        match first.get(col) {
            Some(av) if loose_scalar_eq(av, wv) => {}
            Some(av) => {
                return fail(
                    "cassandra",
                    format!("column `{col}` = {av}, expected {wv}"),
                    Some(wv.clone()),
                    Some(av.clone()),
                );
            }
            None => {
                return fail(
                    "cassandra",
                    format!("column `{col}` not in result row"),
                    Some(wv.clone()),
                    None,
                );
            }
        }
    }
    pass("cassandra", "first row matches expected columns".into())
}

/// Match a single scalar (the sole column of the first row) against `expected`.
fn match_scalar(rows: &[Json], expected: &Json) -> AssertionResult {
    let Some(first) = rows.first() else {
        return fail("cassandra", "expect_value but query returned no rows".into(), None, None);
    };
    let actual = first.as_object().and_then(|o| o.values().next());
    match actual {
        Some(av) if loose_scalar_eq(av, expected) => pass("cassandra", format!("scalar = {av}")),
        Some(av) => fail(
            "cassandra",
            format!("scalar = {av}, expected {expected}"),
            Some(expected.clone()),
            Some(av.clone()),
        ),
        None => fail("cassandra", "could not read scalar from row".into(), None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assertion(y: &str) -> CassandraAssertion {
        serde_yaml::from_str(y).unwrap()
    }

    #[test]
    fn rewrites_select_to_select_json() {
        assert_eq!(as_json_select("SELECT count(*) FROM t"), "SELECT JSON count(*) FROM t");
        assert_eq!(as_json_select("  select id from t  "), "SELECT JSON id from t");
        // Already JSON, or non-SELECT: untouched.
        assert_eq!(as_json_select("SELECT JSON id FROM t"), "SELECT JSON id FROM t");
        assert_eq!(as_json_select("TRUNCATE t"), "TRUNCATE t");
    }

    // A realistic cqlsh `SELECT JSON` transcript, borders and footer included.
    const TRANSCRIPT: &str = "\n [json]\n-------------------\n {\"count\": 1}\n\n(1 rows)\n";

    #[test]
    fn parses_only_json_object_lines() {
        let rows = parse_json_rows(TRANSCRIPT);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get("count").and_then(|v| v.as_i64()), Some(1));
    }

    #[test]
    fn expect_value_matches_scalar() {
        let rows = parse_json_rows(TRANSCRIPT);
        let a = assertion("query: \"SELECT count(*) FROM t\"\nexpect_value: 1");
        assert!(check_result(&rows, &a).iter().all(|x| x.passed));
    }

    #[test]
    fn expect_row_and_rows_are_checked() {
        let stdout = " {\"id\": 42, \"sku\": \"abc\"}\n {\"id\": 43, \"sku\": \"def\"}\n(2 rows)\n";
        let rows = parse_json_rows(stdout);
        let ok = assertion("query: q\nexpect_rows: 2\nexpect_row: { id: 42, sku: abc }");
        assert!(check_result(&rows, &ok).iter().all(|x| x.passed));
        let bad = assertion("query: q\nexpect_row: { sku: zzz }");
        assert!(check_result(&rows, &bad).iter().any(|x| !x.passed));
    }
}
