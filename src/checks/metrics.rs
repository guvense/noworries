//! Prometheus metric assertion: scrape a metrics endpoint, match one series by
//! name + labels, and compare its value.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value as Json;

use super::{fail, pass, resolve_target, retry_until, Assertion};
use crate::runner::{AssertionResult, RunnerContext};
use crate::spec::CheckSpec;

pub struct Metrics;

impl Assertion for Metrics {
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, retry: Duration) -> Vec<AssertionResult> {
        let Some(m) = &check.metrics else { return vec![] };
        let url = resolve_target(&m.path, ctx, &["http://", "https://"]);
        let (auth_headers, _) = ctx.auth_for(check);
        retry_until(retry, || vec![evaluate(m, auth_headers, &url)])
    }
}

fn evaluate(
    m: &crate::spec::MetricsAssertion,
    auth_headers: &[(String, String)],
    url: &str,
) -> AssertionResult {
    let agent = super::http_agent(Duration::from_secs(15));
    let mut req = agent.get(url);
    for (k, v) in auth_headers {
        req = req.set(k, v);
    }
    let text = match req.call() {
        Ok(r) => r.into_string().unwrap_or_default(),
        Err(ureq::Error::Status(_, r)) => r.into_string().unwrap_or_default(),
        Err(ureq::Error::Transport(t)) => {
            return fail("metrics", format!("scrape {url} failed: {t}"), None, None);
        }
    };

    let matched = find_series(&text, &m.metric, &m.labels);
    let Some(value) = matched else {
        return fail(
            "metrics",
            format!("metric `{}` with labels {:?} not found", m.metric, m.labels),
            None,
            None,
        );
    };

    match &m.expect {
        None => pass("metrics", format!("`{}` present = {value}", m.metric)),
        Some(expr) => match compare(value, expr) {
            Ok(true) => pass("metrics", format!("`{}` = {value} {expr}", m.metric)),
            Ok(false) => fail(
                "metrics",
                format!("`{}` = {value}, expected {expr}", m.metric),
                Some(Json::String(expr.clone())),
                Some(Json::from(value)),
            ),
            Err(e) => fail("metrics", e, None, None),
        },
    }
}

/// Find the value of the first series matching `name` and a subset of `labels`.
/// Parses the Prometheus text exposition format (comment lines ignored).
fn find_series(text: &str, name: &str, want: &BTreeMap<String, String>) -> Option<f64> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // <name>{labels} <value> [timestamp]   OR   <name> <value>
        let (series, value_str) = match line.rsplit_once(' ') {
            Some((s, v)) => (s.trim(), v.trim()),
            None => continue,
        };
        let (lname, labels) = match series.split_once('{') {
            Some((n, rest)) => (n.trim(), parse_labels(rest.trim_end_matches('}'))),
            None => (series, BTreeMap::new()),
        };
        if lname != name {
            continue;
        }
        if want.iter().all(|(k, v)| labels.get(k).map(|x| x == v).unwrap_or(false)) {
            if let Ok(val) = value_str.parse::<f64>() {
                return Some(val);
            }
        }
    }
    None
}

fn parse_labels(s: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    // key="value",key2="value2"
    for part in split_labels(s) {
        if let Some((k, v)) = part.split_once('=') {
            let v = v.trim().trim_matches('"');
            m.insert(k.trim().to_string(), v.to_string());
        }
    }
    m
}

/// Split on commas that are not inside quotes.
fn split_labels(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_q = !in_q;
                cur.push(c);
            }
            ',' if !in_q => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Evaluate `<value> <expr>` where expr is like ">= 1", "== 3", "< 10", "= 2".
fn compare(value: f64, expr: &str) -> Result<bool, String> {
    let expr = expr.trim();
    let (op, num) = if let Some(r) = expr.strip_prefix(">=") {
        (">=", r)
    } else if let Some(r) = expr.strip_prefix("<=") {
        ("<=", r)
    } else if let Some(r) = expr.strip_prefix("==") {
        ("==", r)
    } else if let Some(r) = expr.strip_prefix('>') {
        (">", r)
    } else if let Some(r) = expr.strip_prefix('<') {
        ("<", r)
    } else if let Some(r) = expr.strip_prefix('=') {
        ("==", r)
    } else {
        return Err(format!("bad metrics comparison \"{expr}\""));
    };
    let n: f64 = num
        .trim()
        .parse()
        .map_err(|_| format!("bad number in metrics comparison \"{expr}\""))?;
    Ok(match op {
        ">=" => value >= n,
        "<=" => value <= n,
        ">" => value > n,
        "<" => value < n,
        _ => (value - n).abs() < f64::EPSILON,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# HELP http_requests_total total
# TYPE http_requests_total counter
http_requests_total{method="POST",status="200"} 3
http_requests_total{method="GET",status="200"} 10
up 1
"#;

    #[test]
    fn finds_series_by_labels() {
        let mut want = BTreeMap::new();
        want.insert("method".into(), "POST".into());
        assert_eq!(find_series(SAMPLE, "http_requests_total", &want), Some(3.0));
        assert_eq!(find_series(SAMPLE, "up", &BTreeMap::new()), Some(1.0));
        want.insert("status".into(), "500".into());
        assert_eq!(find_series(SAMPLE, "http_requests_total", &want), None);
    }

    #[test]
    fn comparisons() {
        assert_eq!(compare(3.0, ">= 1"), Ok(true));
        assert_eq!(compare(3.0, "== 3"), Ok(true));
        assert_eq!(compare(3.0, "< 3"), Ok(false));
        assert_eq!(compare(3.0, "= 3"), Ok(true));
        assert!(compare(3.0, "bogus").is_err());
    }
}
