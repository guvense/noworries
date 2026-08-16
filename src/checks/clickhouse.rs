//! ClickHouse assertion: run a SQL query over ClickHouse's HTTP interface
//! (port 8123) and check the result. Uses the existing `ureq` client — no
//! native ClickHouse driver — which is why the primary endpoint for this
//! service is HTTP rather than the native protocol.
//!
//! ClickHouse `FORMAT JSON` returns 64-bit integers as JSON *strings* (to avoid
//! precision loss in JS parsers), so `SELECT count()` yields `"1"`, not `1`.
//! Comparisons here are therefore loose: a value matches its expectation when
//! they are equal as JSON *or* as their scalar string forms.

use std::time::Duration;

use serde_json::Value as Json;

use super::{fail, loose_scalar_eq, pass, retry_until, yaml_json, Assertion};
use crate::runner::{AssertionResult, RunnerContext};
use crate::services::clickhouse::{CLICKHOUSE_DB, CLICKHOUSE_PASSWORD, CLICKHOUSE_USER};
use crate::spec::{CheckSpec, ClickhouseAssertion, ServiceKind};

pub struct Clickhouse;

impl Assertion for Clickhouse {
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, retry: Duration) -> Vec<AssertionResult> {
        let Some(c) = &check.clickhouse else { return vec![] };
        let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Clickhouse) else {
            return vec![fail(
                "clickhouse",
                "clickhouse assertion but no ClickHouse service is running".into(),
                None,
                None,
            )];
        };
        let url = format!("http://127.0.0.1:{}/?database={CLICKHOUSE_DB}", ep.host_port);
        retry_until(retry, || match query(&url, &c.query) {
            Ok(resp) => check_result(&resp, c),
            Err(e) => vec![fail("clickhouse", e, None, None)],
        })
    }
}

/// Run `sql` over the HTTP interface and return the parsed `FORMAT JSON` body.
fn query(url: &str, sql: &str) -> Result<Json, String> {
    let agent = super::http_agent(Duration::from_secs(15));
    // Append the output format unless the caller already set one.
    let body = if sql.to_ascii_uppercase().contains(" FORMAT ") {
        sql.to_string()
    } else {
        format!("{sql}\nFORMAT JSON")
    };
    let resp = agent
        .post(url)
        .set("X-ClickHouse-User", CLICKHOUSE_USER)
        .set("X-ClickHouse-Key", CLICKHOUSE_PASSWORD)
        .send_string(&body);
    let text = match resp {
        Ok(r) => r.into_string().unwrap_or_default(),
        // Bad SQL / server error: ClickHouse returns the message in the body.
        Err(ureq::Error::Status(code, r)) => {
            let msg = r.into_string().unwrap_or_default();
            return Err(format!("query failed (HTTP {code}): {}", msg.trim()));
        }
        Err(ureq::Error::Transport(t)) => return Err(format!("could not reach ClickHouse: {t}")),
    };
    serde_json::from_str(&text).map_err(|e| format!("could not parse ClickHouse JSON: {e}"))
}

/// Evaluate the configured expectations against a parsed `FORMAT JSON` response.
/// Pure (no IO) so it can be unit-tested with canned response bodies.
fn check_result(resp: &Json, c: &ClickhouseAssertion) -> Vec<AssertionResult> {
    let data = resp.get("data").and_then(|d| d.as_array());
    let Some(rows) = data else {
        return vec![fail(
            "clickhouse",
            "response had no `data` array (is the query a SELECT?)".into(),
            None,
            None,
        )];
    };

    let mut out = Vec::new();

    if let Some(want) = c.expect_rows {
        if rows.len() == want {
            out.push(pass("clickhouse", format!("query returned {want} row(s)")));
        } else {
            out.push(fail(
                "clickhouse",
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
        out.push(match_scalar(resp, rows, &yaml_json(expect_value)));
    }

    // A bare query with no expectations just asserts it ran and returned rows.
    if out.is_empty() {
        out.push(pass("clickhouse", format!("query ran, {} row(s)", rows.len())));
    }
    out
}

/// Subset-match the first row against an expected `column -> value` object.
fn match_first_row(rows: &[Json], expected: &Json) -> AssertionResult {
    let Some(first) = rows.first() else {
        return fail("clickhouse", "expect_row but query returned no rows".into(), None, None);
    };
    let Some(want) = expected.as_object() else {
        return fail("clickhouse", "expect_row must be a column→value map".into(), None, None);
    };
    for (col, wv) in want {
        match first.get(col) {
            Some(av) if loose_scalar_eq(av, wv) => {}
            Some(av) => {
                return fail(
                    "clickhouse",
                    format!("column `{col}` = {av}, expected {wv}"),
                    Some(wv.clone()),
                    Some(av.clone()),
                );
            }
            None => {
                return fail(
                    "clickhouse",
                    format!("column `{col}` not in result row"),
                    Some(wv.clone()),
                    None,
                );
            }
        }
    }
    pass("clickhouse", "first row matches expected columns".into())
}

/// Match a single scalar (first column of the first row) against `expected`.
/// Column order comes from the response `meta`, which is reliably ordered
/// (JSON object key order is not).
fn match_scalar(resp: &Json, rows: &[Json], expected: &Json) -> AssertionResult {
    let Some(first) = rows.first() else {
        return fail("clickhouse", "expect_value but query returned no rows".into(), None, None);
    };
    let col = resp
        .get("meta")
        .and_then(|m| m.as_array())
        .and_then(|m| m.first())
        .and_then(|c| c.get("name"))
        .and_then(|n| n.as_str());
    let actual = match col {
        Some(name) => first.get(name),
        // Fall back to the sole value if meta is absent.
        None => first.as_object().and_then(|o| o.values().next()),
    };
    match actual {
        Some(av) if loose_scalar_eq(av, expected) => {
            pass("clickhouse", format!("scalar = {av}"))
        }
        Some(av) => fail(
            "clickhouse",
            format!("scalar = {av}, expected {expected}"),
            Some(expected.clone()),
            Some(av.clone()),
        ),
        None => fail("clickhouse", "could not read scalar from row".into(), None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assertion(json: &str) -> ClickhouseAssertion {
        serde_yaml::from_str(json).unwrap()
    }

    // A canned `FORMAT JSON` body for `SELECT count() AS n ...` returning 3,
    // with the UInt64 rendered as a string exactly as ClickHouse does.
    fn count_resp() -> Json {
        serde_json::json!({
            "meta": [{"name": "n", "type": "UInt64"}],
            "data": [{"n": "3"}],
            "rows": 1
        })
    }

    #[test]
    fn expect_value_matches_across_string_int_boundary() {
        let a = assertion("query: \"SELECT count() AS n\"\nexpect_value: 3");
        let r = check_result(&count_resp(), &a);
        assert!(r.iter().all(|x| x.passed), "{r:?}");
    }

    #[test]
    fn expect_rows_counts_data_array() {
        let a = assertion("query: \"SELECT * FROM t\"\nexpect_rows: 1");
        assert!(check_result(&count_resp(), &a).iter().all(|x| x.passed));
        let a2 = assertion("query: \"SELECT * FROM t\"\nexpect_rows: 5");
        assert!(check_result(&count_resp(), &a2).iter().any(|x| !x.passed));
    }

    #[test]
    fn expect_row_subset_loose_compare() {
        let resp = serde_json::json!({
            "meta": [{"name": "id", "type": "UInt64"}, {"name": "name", "type": "String"}],
            "data": [{"id": "42", "name": "ada"}],
            "rows": 1
        });
        let ok = assertion("query: q\nexpect_row: { id: 42, name: ada }");
        assert!(check_result(&resp, &ok).iter().all(|x| x.passed));
        let bad = assertion("query: q\nexpect_row: { name: grace }");
        assert!(check_result(&resp, &bad).iter().any(|x| !x.passed));
    }

    #[test]
    fn missing_data_array_fails_cleanly() {
        let a = assertion("query: q\nexpect_value: 1");
        let r = check_result(&serde_json::json!({"rows": 0}), &a);
        assert!(r.iter().any(|x| !x.passed));
    }
}
