//! Executes noworries.yml checks against the running app + containers and
//! collects structured pass/fail results with actual-vs-expected detail.
//!
//! Phase 2: HTTP assertions (status + body_contains) and Postgres DB assertions
//! (expect_row / expect_row_count). Kafka (phase 3) and Redis (phase 4) are
//! recorded as failing "not implemented" so a check that declares them can't
//! silently pass.

use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value as Json;

use crate::lifecycle::ServiceEndpoint;
use crate::services::{PG_DB, PG_PASSWORD, PG_USER};
use crate::spec::{AuthSpec, CheckSpec, ServiceKind};

#[derive(Debug, Clone, Serialize)]
pub struct AssertionResult {
    pub kind: String,
    pub passed: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<Json>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<Json>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    pub assertions: Vec<AssertionResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct RunnerContext {
    pub app_port: u16,
    pub endpoints: Vec<ServiceEndpoint>,
    /// Variables available for `${VAR}` interpolation (from `app.env`; process
    /// env is also consulted as a fallback).
    pub env: BTreeMap<String, String>,
    /// Headers injected into every check request (from `auth`).
    pub auth_headers: Vec<(String, String)>,
    /// Query params injected into every check request (from `auth`, e.g. API key).
    pub auth_query: Vec<(String, String)>,
    /// The app's captured log file, for `logs:` assertions.
    pub app_log_path: Option<std::path::PathBuf>,
    /// Recorded requests per mocked external (name → shared recorder), for
    /// `external_calls:` assertions.
    pub external_mocks: BTreeMap<String, crate::mock::Recorder>,
    /// Project directory (for file-based assertions like `snapshot`).
    pub dir: std::path::PathBuf,
    /// Whether `--update-snapshots` was passed (write goldens instead of failing).
    pub update_snapshots: bool,
    /// The most recent check `request` response body, so response-dependent
    /// assertions (e.g. `snapshot`) can read it. Set by `run_http` each check.
    pub http_capture: std::sync::Mutex<Option<String>>,
}

impl RunnerContext {
    /// Minimal context (no env/auth) — convenient for tests.
    pub fn new(app_port: u16, endpoints: Vec<ServiceEndpoint>) -> Self {
        RunnerContext {
            app_port,
            endpoints,
            env: BTreeMap::new(),
            auth_headers: Vec::new(),
            auth_query: Vec::new(),
            app_log_path: None,
            external_mocks: BTreeMap::new(),
            dir: std::path::PathBuf::from("."),
            update_snapshots: false,
            http_capture: std::sync::Mutex::new(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Env interpolation + auth
// ---------------------------------------------------------------------------

fn lookup_var(name: &str, env: &BTreeMap<String, String>) -> Option<String> {
    // Process env wins (so CI / shell exports can override), then the merged
    // app.env + .noworries.env map.
    std::env::var(name).ok().or_else(|| env.get(name).cloned())
}

/// Expand `${VAR}` references in a string using `env` then the process env.
/// An unset variable is left as-is and a warning is printed.
pub fn interpolate(s: &str, env: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find('}') {
            let name = &after[..end];
            match lookup_var(name, env) {
                Some(v) => out.push_str(&v),
                None => {
                    crate::report::warn(&format!("env var ${{{name}}} is not set"));
                    out.push_str(&rest[start..start + 2 + end + 1]);
                }
            }
            rest = &after[end + 1..];
        } else {
            out.push_str(&rest[start..]);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

/// Parse dotenv-style content (`KEY=VALUE` lines, `#` comments, optional
/// surrounding quotes) into a map. Used for the gitignored `.noworries.env`.
pub fn parse_env_file(content: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            let mut v = v.trim();
            if v.len() >= 2
                && ((v.starts_with('"') && v.ends_with('"'))
                    || (v.starts_with('\'') && v.ends_with('\'')))
            {
                v = &v[1..v.len() - 1];
            }
            if !k.is_empty() {
                m.insert(k.to_string(), v.to_string());
            }
        }
    }
    m
}

/// Interpolate `${VAR}` in every string leaf of a JSON value.
pub fn interpolate_json(v: &Json, env: &BTreeMap<String, String>) -> Json {
    match v {
        Json::String(s) => Json::String(interpolate(s, env)),
        Json::Array(a) => Json::Array(a.iter().map(|x| interpolate_json(x, env)).collect()),
        Json::Object(o) => {
            Json::Object(o.iter().map(|(k, val)| (k.clone(), interpolate_json(val, env))).collect())
        }
        other => other.clone(),
    }
}

/// Standard-alphabet base64 (for HTTP Basic auth) — avoids a dependency.
pub(crate) fn base64_encode(input: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(T[(b0 >> 2) as usize] as char);
        out.push(T[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 { T[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(b2 & 0x3f) as usize] as char } else { '=' });
    }
    out
}

/// Follow a simple JSON path (`$.a.b`, `a.b`, numeric segments = array index).
fn json_path<'a>(v: &'a Json, path: &str) -> Option<&'a Json> {
    let p = path.trim_start_matches('$').trim_start_matches('.');
    if p.is_empty() {
        return Some(v);
    }
    let mut cur = v;
    for seg in p.split('.') {
        cur = if let Ok(i) = seg.parse::<usize>() { cur.get(i)? } else { cur.get(seg)? };
    }
    Some(cur)
}

/// Resolved auth to apply to every check request.
pub struct ResolvedAuth {
    pub headers: Vec<(String, String)>,
    pub query: Vec<(String, String)>,
}

fn header_scheme(header: &Option<String>, scheme: &Option<String>, token: String) -> (String, String) {
    let h = header.clone().unwrap_or_else(|| "Authorization".to_string());
    let s = scheme.clone().unwrap_or_else(|| "Bearer".to_string());
    let value = if s.is_empty() { token } else { format!("{s} {token}") };
    (h, value)
}

/// Turn the `auth` spec into concrete headers/query params. Runs the login
/// request (against the app) when `auth.login` is set.
pub fn resolve_auth(
    auth: Option<&AuthSpec>,
    app_port: u16,
    env: &BTreeMap<String, String>,
) -> anyhow::Result<ResolvedAuth> {
    let mut headers = Vec::new();
    let mut query = Vec::new();
    let Some(auth) = auth else {
        return Ok(ResolvedAuth { headers, query });
    };

    if let Some(b) = &auth.basic {
        let u = interpolate(&b.username, env);
        let p = interpolate(&b.password, env);
        let token = base64_encode(format!("{u}:{p}").as_bytes());
        headers.push(("Authorization".to_string(), format!("Basic {token}")));
    }

    if let Some(b) = &auth.bearer {
        headers.push(header_scheme(&b.header, &b.scheme, interpolate(&b.token, env)));
    }

    if let Some(k) = &auth.api_key {
        let value = interpolate(&k.value, env);
        match (&k.header, &k.query) {
            (None, None) => headers.push(("X-API-Key".to_string(), value)),
            (h, q) => {
                if let Some(h) = h {
                    headers.push((h.clone(), value.clone()));
                }
                if let Some(q) = q {
                    query.push((q.clone(), value));
                }
            }
        }
    }

    if let Some(login) = &auth.login {
        let req = &login.request;
        // The login endpoint may be the app under test (relative path) OR a
        // separate auth server (absolute URL, possibly via `${AUTH_URL}`).
        let raw = interpolate(&req.path, env);
        let url = if raw.starts_with("http://") || raw.starts_with("https://") {
            raw
        } else {
            format!("http://127.0.0.1:{app_port}{raw}")
        };
        let agent = ureq::builder().timeout(Duration::from_secs(15)).build();
        let mut r = agent.request(&req.method.to_uppercase(), &url);
        for (hk, hv) in &req.headers {
            r = r.set(hk, &interpolate(hv, env));
        }
        let send = if let Some(body) = &req.body {
            let bj = interpolate_json(&yaml_to_json(body), env);
            r.set("content-type", "application/json").send_string(&serde_json::to_string(&bj)?)
        } else {
            r.call()
        };
        let (status, body) = match send {
            Ok(resp) => (resp.status(), resp.into_string().unwrap_or_default()),
            Err(ureq::Error::Status(c, resp)) => (c, resp.into_string().unwrap_or_default()),
            Err(ureq::Error::Transport(t)) => anyhow::bail!("auth login request failed: {t}"),
        };
        match login.expect.as_ref().and_then(|e| e.status) {
            Some(want) if status != want => {
                anyhow::bail!("auth login: expected status {want}, got {status}")
            }
            None if !(200..300).contains(&status) => {
                anyhow::bail!("auth login returned HTTP {status}")
            }
            _ => {}
        }
        let parsed: Json = serde_json::from_str(&body).unwrap_or(Json::Null);
        let token = json_path(&parsed, &login.token_from)
            .map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string()))
            .ok_or_else(|| {
                anyhow::anyhow!("auth login: token_from \"{}\" not found in response", login.token_from)
            })?;
        headers.push(header_scheme(&login.header, &login.scheme, token));
    }

    Ok(ResolvedAuth { headers, query })
}

/// serde_yaml value -> serde_json value, so YAML expectations compare uniformly.
fn yaml_to_json(v: &serde_yaml::Value) -> Json {
    serde_json::to_value(v).unwrap_or(Json::Null)
}

fn loose_equal(a: &Json, b: &Json) -> bool {
    if a == b {
        return true;
    }
    // Compare scalars by their string form (YAML 201 vs JSON 201, "2" vs 2, ...).
    fn scalar_str(v: &Json) -> Option<String> {
        match v {
            Json::String(s) => Some(s.clone()),
            Json::Number(n) => Some(n.to_string()),
            Json::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }
    match (scalar_str(a), scalar_str(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// Deep partial match: every key/value in `expected` must appear in `actual`.
pub fn matches_subset(expected: &Json, actual: &Json) -> bool {
    match expected {
        Json::Object(exp) => {
            let Some(act) = actual.as_object() else { return false };
            exp.iter()
                .all(|(k, v)| act.get(k).map(|av| matches_subset(v, av)).unwrap_or(false))
        }
        Json::Array(exp) => {
            let Some(act) = actual.as_array() else { return false };
            if act.len() < exp.len() {
                return false;
            }
            exp.iter().enumerate().all(|(i, v)| matches_subset(v, &act[i]))
        }
        _ => loose_equal(expected, actual),
    }
}

fn truncate_json(v: Json) -> Json {
    let s = v.to_string();
    if s.len() > 500 {
        Json::String(format!("{}…(truncated)", &s[..500]))
    } else {
        v
    }
}

fn run_http(check: &CheckSpec, ctx: &RunnerContext, out: &mut Vec<AssertionResult>) -> anyhow::Result<()> {
    let Some(req) = &check.request else { return Ok(()) };
    let path = interpolate(&req.path, &ctx.env);
    let mut url = format!("http://127.0.0.1:{}{}", ctx.app_port, path);
    // Auth query params (e.g. an API key) applied to every request.
    if !ctx.auth_query.is_empty() {
        let sep = if url.contains('?') { '&' } else { '?' };
        let qs = ctx
            .auth_query
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        url = format!("{url}{sep}{qs}");
    }

    // Headers: auth first, then the check's own headers (which override), with
    // ${VAR} interpolation on the check's header values.
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in &ctx.auth_headers {
        headers.insert(k.clone(), v.clone());
    }
    for (k, v) in &req.headers {
        headers.insert(k.clone(), interpolate(v, &ctx.env));
    }

    let agent = ureq::builder().timeout(Duration::from_secs(15)).build();
    let mut request = agent.request(&req.method.to_uppercase(), &url);
    for (k, v) in &headers {
        request = request.set(k, v);
    }

    let started = Instant::now();
    let send_result = if let Some(body) = &req.body {
        let body_str = match body {
            serde_yaml::Value::String(s) => interpolate(s, &ctx.env),
            other => serde_json::to_string(&interpolate_json(&yaml_to_json(other), &ctx.env))
                .unwrap_or_default(),
        };
        request.set("content-type", "application/json").send_string(&body_str)
    } else {
        request.call()
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;

    let (status, body_text) = match send_result {
        Ok(resp) => {
            let s = resp.status();
            (s, resp.into_string().unwrap_or_default())
        }
        Err(ureq::Error::Status(code, resp)) => (code, resp.into_string().unwrap_or_default()),
        Err(ureq::Error::Transport(t)) => {
            anyhow::bail!("HTTP {} {} failed: {}", req.method, req.path, t);
        }
    };

    // Stash the response body so response-dependent assertions (snapshot) can
    // read it after run_http returns.
    if let Ok(mut cap) = ctx.http_capture.lock() {
        *cap = Some(body_text.clone());
    }

    if let Some(expect) = &check.expect {
        if let Some(expected_status) = expect.status {
            let passed = status == expected_status;
            out.push(AssertionResult {
                kind: "http".into(),
                passed,
                message: if passed {
                    format!("{} {} -> {}", req.method, req.path, status)
                } else {
                    format!("{} {}: expected status {}, got {}", req.method, req.path, expected_status, status)
                },
                expected: Some(Json::from(expected_status)),
                actual: Some(Json::from(status)),
            });
        }
        if let Some(body_contains) = &expect.body_contains {
            let parsed: Json = serde_json::from_str(&body_text).unwrap_or(Json::String(body_text.clone()));
            let expected = yaml_to_json(body_contains);
            let passed = matches_subset(&expected, &parsed);
            out.push(AssertionResult {
                kind: "http-body".into(),
                passed,
                message: if passed {
                    "body contains expected fields".into()
                } else {
                    "response body did not contain expected fields".into()
                },
                expected: Some(expected),
                actual: Some(truncate_json(parsed)),
            });
        }
        if let Some(max_ms) = expect.max_ms {
            let passed = elapsed_ms <= max_ms;
            out.push(AssertionResult {
                kind: "http-latency".into(),
                passed,
                message: if passed {
                    format!("{} {} responded in {elapsed_ms}ms (<= {max_ms}ms)", req.method, req.path)
                } else {
                    format!("{} {} took {elapsed_ms}ms (> {max_ms}ms budget)", req.method, req.path)
                },
                expected: Some(Json::from(max_ms)),
                actual: Some(Json::from(elapsed_ms)),
            });
        }
    }
    Ok(())
}

fn cell_to_json(row: &postgres::Row, idx: usize) -> Json {
    if let Ok(v) = row.try_get::<_, Option<String>>(idx) {
        return v.map(Json::String).unwrap_or(Json::Null);
    }
    if let Ok(v) = row.try_get::<_, Option<i64>>(idx) {
        return v.map(Json::from).unwrap_or(Json::Null);
    }
    if let Ok(v) = row.try_get::<_, Option<i32>>(idx) {
        return v.map(Json::from).unwrap_or(Json::Null);
    }
    if let Ok(v) = row.try_get::<_, Option<bool>>(idx) {
        return v.map(Json::Bool).unwrap_or(Json::Null);
    }
    if let Ok(v) = row.try_get::<_, Option<f64>>(idx) {
        return serde_json::to_value(v).unwrap_or(Json::Null);
    }
    Json::String("<unreadable value>".into())
}

fn row_to_json(row: &postgres::Row) -> Json {
    let mut map = serde_json::Map::new();
    for (i, col) in row.columns().iter().enumerate() {
        map.insert(col.name().to_string(), cell_to_json(row, i));
    }
    Json::Object(map)
}

/// One evaluation of a check's DB assertions against a fresh connection.
fn evaluate_db(db: &crate::spec::DbAssertion, host_port: u16) -> Vec<AssertionResult> {
    let mut out = Vec::new();
    let conn = format!(
        "host=127.0.0.1 port={host_port} user={PG_USER} password={PG_PASSWORD} dbname={PG_DB}"
    );
    let mut client = match postgres::Client::connect(&conn, postgres::NoTls) {
        Ok(c) => c,
        Err(e) => {
            out.push(fail_db(format!("could not connect to Postgres: {e}")));
            return out;
        }
    };
    let rows = match client.query(db.query.as_str(), &[]) {
        Ok(r) => r,
        Err(e) => {
            out.push(fail_db(format!("db query failed: {e}")));
            return out;
        }
    };

    if let Some(expected_count) = db.expect_row_count {
        let passed = rows.len() as i64 == expected_count;
        out.push(AssertionResult {
            kind: "db".into(),
            passed,
            message: if passed {
                format!("query returned {} row(s)", rows.len())
            } else {
                format!("expected {} row(s), got {}", expected_count, rows.len())
            },
            expected: Some(Json::from(expected_count)),
            actual: Some(Json::from(rows.len() as i64)),
        });
    }

    if let Some(expect_row) = &db.expect_row {
        let expected = yaml_to_json(expect_row);
        match rows.first() {
            Some(row) => {
                let actual = row_to_json(row);
                let passed = matches_subset(&expected, &actual);
                out.push(AssertionResult {
                    kind: "db".into(),
                    passed,
                    message: if passed {
                        "first row matches expected fields".into()
                    } else {
                        "first row did not match expected fields".into()
                    },
                    expected: Some(expected),
                    actual: Some(actual),
                });
            }
            None => out.push(AssertionResult {
                kind: "db".into(),
                passed: false,
                message: "expected a matching row, but query returned no rows".into(),
                expected: Some(expected),
                actual: Some(Json::Null),
            }),
        }
    }
    out
}

fn fail_db(message: String) -> AssertionResult {
    AssertionResult { kind: "db".into(), passed: false, message, expected: None, actual: None }
}

/// Run DB assertions, retrying for a few seconds so an asynchronous side effect
/// (e.g. a Kafka consumer writing to the DB) has time to land.
fn run_db(check: &CheckSpec, ctx: &RunnerContext, retry: Duration, out: &mut Vec<AssertionResult>) {
    let Some(db) = &check.db else { return };
    let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Postgres) else {
        out.push(fail_db("db assertion but no Postgres service is running".into()));
        return;
    };

    let deadline = Instant::now() + retry;
    let mut last = evaluate_db(db, ep.host_port);
    while !last.iter().all(|a| a.passed) && Instant::now() < deadline {
        sleep(Duration::from_millis(400));
        last = evaluate_db(db, ep.host_port);
    }
    out.extend(last);
}

/// One evaluation of a check's Redis assertions against a fresh connection.
fn evaluate_redis(r: &crate::spec::RedisAssertion, port: u16) -> Vec<AssertionResult> {
    let mut out = Vec::new();
    let client = match redis::Client::open(format!("redis://127.0.0.1:{port}/")) {
        Ok(c) => c,
        Err(e) => {
            out.push(fail_redis(format!("could not open Redis client: {e}")));
            return out;
        }
    };
    let mut con = match client.get_connection() {
        Ok(c) => c,
        Err(e) => {
            out.push(fail_redis(format!("could not connect to Redis: {e}")));
            return out;
        }
    };

    let exists: bool = match redis::cmd("EXISTS").arg(&r.key).query(&mut con) {
        Ok(v) => v,
        Err(e) => {
            out.push(fail_redis(format!("EXISTS {} failed: {e}", r.key)));
            return out;
        }
    };

    // Default expectation when none is given: the key should exist.
    let want_exists = r.expect_exists.unwrap_or(true);
    if r.expect_exists.is_some() || (r.expect_value.is_none() && r.expect_value_contains.is_none()) {
        let passed = exists == want_exists;
        out.push(AssertionResult {
            kind: "redis".into(),
            passed,
            message: if passed {
                format!("key \"{}\" exists = {}", r.key, exists)
            } else {
                format!("key \"{}\": expected exists={}, got {}", r.key, want_exists, exists)
            },
            expected: Some(Json::Bool(want_exists)),
            actual: Some(Json::Bool(exists)),
        });
    }

    if r.expect_value.is_some() || r.expect_value_contains.is_some() {
        let value: Option<String> = match redis::cmd("GET").arg(&r.key).query(&mut con) {
            Ok(v) => v,
            Err(e) => {
                out.push(fail_redis(format!("GET {} failed: {e}", r.key)));
                return out;
            }
        };
        let actual_json = match &value {
            Some(s) => serde_json::from_str(s).unwrap_or_else(|_| Json::String(s.clone())),
            None => Json::Null,
        };
        if let Some(expect_value) = &r.expect_value {
            let expected = yaml_to_json(expect_value);
            let passed = loose_equal(&expected, &actual_json);
            out.push(AssertionResult {
                kind: "redis".into(),
                passed,
                message: if passed {
                    format!("key \"{}\" value matches", r.key)
                } else {
                    format!("key \"{}\" value did not match", r.key)
                },
                expected: Some(expected),
                actual: Some(actual_json.clone()),
            });
        }
        if let Some(contains) = &r.expect_value_contains {
            let expected = yaml_to_json(contains);
            let passed = matches_subset(&expected, &actual_json);
            out.push(AssertionResult {
                kind: "redis".into(),
                passed,
                message: if passed {
                    format!("key \"{}\" value contains expected fields", r.key)
                } else {
                    format!("key \"{}\" value did not contain expected fields", r.key)
                },
                expected: Some(expected),
                actual: Some(truncate_json(actual_json)),
            });
        }
    }
    out
}

fn fail_redis(message: String) -> AssertionResult {
    AssertionResult { kind: "redis".into(), passed: false, message, expected: None, actual: None }
}

/// Run Redis assertions, retrying briefly so an async cache write can land.
fn run_redis(check: &CheckSpec, ctx: &RunnerContext, retry: Duration, out: &mut Vec<AssertionResult>) {
    let Some(r) = &check.redis else { return };
    let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Redis) else {
        out.push(fail_redis("redis assertion but no Redis service is running".into()));
        return;
    };
    let deadline = Instant::now() + retry;
    let mut last = evaluate_redis(r, ep.host_port);
    while !last.iter().all(|a| a.passed) && Instant::now() < deadline {
        sleep(Duration::from_millis(300));
        last = evaluate_redis(r, ep.host_port);
    }
    out.extend(last);
}

fn yaml_to_bytes(v: &serde_yaml::Value) -> Vec<u8> {
    match v {
        serde_yaml::Value::String(s) => s.clone().into_bytes(),
        other => serde_json::to_vec(&yaml_to_json(other)).unwrap_or_default(),
    }
}

fn unique_group() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("noworries-{}-{}", std::process::id(), nanos)
}

/// Kafka errors that just mean "the topic/leader isn't ready yet". When a topic
/// is auto-created on first use, the very first produce (and a consumer's first
/// poll) races the creation and comes back with one of these — so we retry them
/// while the topic propagates instead of failing the check. This is what caused
/// the CI `UnknownTopicOrPartition` failure. Matched on the formatted error so it
/// stays correct across kafka-crate versions (no brittle enum-variant matching).
fn kafka_err_is_transient(e: &kafka::Error) -> bool {
    let s = format!("{e:?}");
    s.contains("UnknownTopicOrPartition")
        || s.contains("LeaderNotAvailable")
        || s.contains("NotLeaderForPartition")
        || s.contains("RequestTimedOut")
        || s.contains("NotEnoughReplicas")
        || s.contains("GroupCoordinatorNotAvailable")
}

/// Send one record, retrying transient "topic not ready" errors until `deadline`.
/// Only the first send to a freshly auto-created topic actually waits; once the
/// topic exists later sends return immediately.
fn kafka_send_retry(
    producer: &mut kafka::producer::Producer,
    topic: &str,
    key: Option<&[u8]>,
    payload: &[u8],
    deadline: Instant,
) -> std::result::Result<(), kafka::Error> {
    use kafka::producer::Record;
    loop {
        let res = match key {
            Some(k) => producer.send(&Record::from_key_value(topic, k, payload)),
            None => producer.send(&Record::from_value(topic, payload)),
        };
        match res {
            Ok(()) => return Ok(()),
            Err(e) => {
                if Instant::now() >= deadline || !kafka_err_is_transient(&e) {
                    return Err(e);
                }
                sleep(Duration::from_millis(500));
            }
        }
    }
}

/// How long to wait for a just-auto-created topic to become usable.
const KAFKA_TOPIC_READY_SECS: u64 = 20;

/// Ensure `topic` exists with an elected leader, triggering broker-side
/// auto-creation. This is required because the kafka crate's Producer/Consumer
/// **silently skip** unknown topics and never cause auto-creation — only an
/// explicit `KafkaClient::load_metadata(&[topic])` does (per the crate docs).
/// Without this, producing to a fresh topic fails forever with
/// `UnknownTopicOrPartition` (the CI failure).
fn kafka_ensure_topic(host: &str, topic: &str, deadline: Instant) -> std::result::Result<(), String> {
    use kafka::client::KafkaClient;
    let mut client = KafkaClient::new(vec![host.to_string()]);
    loop {
        // Requesting metadata for this specific topic auto-creates it on the
        // broker (when auto-create is enabled) and elects a leader.
        let _ = client.load_metadata(&[topic]);
        let ready = client
            .topics()
            .partitions(topic)
            .map(|p| !p.available_ids().is_empty())
            .unwrap_or(false);
        if ready {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "topic \"{topic}\" never became available — is `auto.create.topics.enable` on?"
            ));
        }
        sleep(Duration::from_millis(500));
    }
}

/// Produce the message declared by a check to its topic — used to trigger a
/// consumer the feature under test added. The DB/redis assertion in the same
/// check verifies the effect.
fn kafka_produce(check: &CheckSpec, ctx: &RunnerContext, out: &mut Vec<AssertionResult>) {
    let Some(kafka) = &check.kafka else { return };
    let Some(produce) = &kafka.produce else { return };
    let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Kafka) else {
        out.push(AssertionResult {
            kind: "kafka".into(),
            passed: false,
            message: "kafka produce declared but no Kafka service is running".into(),
            expected: None,
            actual: None,
        });
        return;
    };

    let host = format!("127.0.0.1:{}", ep.host_port);
    let payload = yaml_to_bytes(&produce.message);
    let result = (|| -> anyhow::Result<()> {
        use kafka::producer::{Producer, RequiredAcks};
        let deadline = Instant::now() + Duration::from_secs(KAFKA_TOPIC_READY_SECS);
        // Auto-create the topic (the Producer won't) before producing.
        kafka_ensure_topic(&host, &produce.topic, deadline).map_err(|e| anyhow::anyhow!(e))?;
        let mut producer = Producer::from_hosts(vec![host.clone()])
            .with_ack_timeout(Duration::from_secs(5))
            .with_required_acks(RequiredAcks::One)
            .create()?;
        kafka_send_retry(
            &mut producer,
            &produce.topic,
            produce.key.as_deref().map(|k| k.as_bytes()),
            payload.as_slice(),
            deadline,
        )?;
        Ok(())
    })();

    match result {
        Ok(()) => out.push(AssertionResult {
            kind: "kafka".into(),
            passed: true,
            message: format!("produced message to topic \"{}\"", produce.topic),
            expected: None,
            actual: None,
        }),
        Err(e) => out.push(AssertionResult {
            kind: "kafka".into(),
            passed: false,
            message: format!("failed to produce to topic \"{}\": {e}", produce.topic),
            expected: None,
            actual: None,
        }),
    }
}

/// Run an edge-case load/timing scenario (burst / concurrent / duplicates /
/// out_of_order) as the trigger. Expands the declared scenario into a plan and
/// produces its messages to Kafka, optionally across parallel producers to
/// create real races. The check's observe assertions verify the invariant.
fn run_scenario(check: &CheckSpec, ctx: &RunnerContext, out: &mut Vec<AssertionResult>) {
    let Some(scenario) = &check.scenario else { return };

    let strategy = match crate::edgecases::resolve(&scenario.kind) {
        Ok(s) => s,
        Err(e) => {
            out.push(AssertionResult {
                kind: "scenario".into(),
                passed: false,
                message: e.to_string(),
                expected: None,
                actual: None,
            });
            return;
        }
    };
    let plan = match strategy.plan(scenario) {
        Ok(p) => p,
        Err(e) => {
            out.push(AssertionResult {
                kind: "scenario".into(),
                passed: false,
                message: format!("scenario \"{}\" is invalid: {e}", scenario.kind),
                expected: None,
                actual: None,
            });
            return;
        }
    };

    let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Kafka) else {
        out.push(AssertionResult {
            kind: "scenario".into(),
            passed: false,
            message: "scenario declared but no Kafka service is running".into(),
            expected: None,
            actual: None,
        });
        return;
    };
    let host = format!("127.0.0.1:{}", ep.host_port);

    let started = Instant::now();
    match produce_plan(&host, &plan) {
        Ok(n) => {
            let secs = started.elapsed().as_secs_f64().max(0.001);
            let per_sec = (n as f64 / secs).round() as u64;
            out.push(AssertionResult {
                kind: "scenario".into(),
                passed: true,
                message: format!(
                    "{} — produced {n} message(s) in {:.2}s (~{per_sec} msg/s)",
                    plan.summary, secs
                ),
                expected: None,
                actual: None,
            });
            if let Some(min) = scenario.expect_throughput_per_sec {
                let passed = per_sec >= min as u64;
                out.push(AssertionResult {
                    kind: "scenario-throughput".into(),
                    passed,
                    message: if passed {
                        format!("throughput ~{per_sec} msg/s (>= {min} required)")
                    } else {
                        format!("throughput ~{per_sec} msg/s (< {min} required)")
                    },
                    expected: Some(Json::from(min)),
                    actual: Some(Json::from(per_sec)),
                });
            }
        }
        Err((n, e)) => out.push(AssertionResult {
            kind: "scenario".into(),
            passed: false,
            message: format!("{} — failed after {n} message(s): {e}", plan.summary),
            expected: None,
            actual: None,
        }),
    }
}

/// `logs:` assertion — grep the app's captured log for required/forbidden
/// substrings. Retried within `retry` so a message the app logs asynchronously
/// (after the triggering request returns) has time to appear.
fn run_logs(check: &CheckSpec, ctx: &RunnerContext, retry: Duration, out: &mut Vec<AssertionResult>) {
    let Some(logs) = &check.logs else { return };
    let Some(path) = &ctx.app_log_path else {
        out.push(AssertionResult {
            kind: "logs".into(),
            passed: false,
            message: "logs assertion declared but the app was not started (no app.log)".into(),
            expected: None,
            actual: None,
        });
        return;
    };

    let deadline = Instant::now() + retry;
    let mut results = evaluate_logs(logs, path);
    // Only the "contains" checks benefit from waiting; retry until they pass or
    // the deadline. "absent" is re-checked each pass too (a bad line appearing
    // later should still fail).
    while !results.iter().all(|a| a.passed) && Instant::now() < deadline {
        sleep(Duration::from_millis(300));
        results = evaluate_logs(logs, path);
    }
    out.extend(results);
}

fn evaluate_logs(logs: &crate::spec::LogAssertion, path: &std::path::Path) -> Vec<AssertionResult> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = Vec::new();
    for needle in &logs.contains {
        let passed = text.contains(needle);
        out.push(AssertionResult {
            kind: "logs".into(),
            passed,
            message: if passed {
                format!("log contains \"{needle}\"")
            } else {
                format!("log does not contain \"{needle}\"")
            },
            expected: Some(Json::String(format!("contains: {needle}"))),
            actual: None,
        });
    }
    for needle in &logs.absent {
        let passed = !text.contains(needle);
        out.push(AssertionResult {
            kind: "logs".into(),
            passed,
            message: if passed {
                format!("log is free of \"{needle}\"")
            } else {
                format!("log unexpectedly contains \"{needle}\"")
            },
            expected: Some(Json::String(format!("absent: {needle}"))),
            actual: None,
        });
    }
    out
}

/// `external_calls:` assertion — verify the app called a mocked external with a
/// matching request. Retried within `retry` for calls the app makes
/// asynchronously.
fn run_external_calls(check: &CheckSpec, ctx: &RunnerContext, retry: Duration, out: &mut Vec<AssertionResult>) {
    if check.external_calls.is_empty() {
        return;
    }
    let deadline = Instant::now() + retry;
    let mut results = evaluate_external_calls(check, ctx);
    while !results.iter().all(|a| a.passed) && Instant::now() < deadline {
        sleep(Duration::from_millis(300));
        results = evaluate_external_calls(check, ctx);
    }
    out.extend(results);
}

fn evaluate_external_calls(check: &CheckSpec, ctx: &RunnerContext) -> Vec<AssertionResult> {
    let mut out = Vec::new();
    for call in &check.external_calls {
        let Some(recorder) = ctx.external_mocks.get(&call.external) else {
            out.push(AssertionResult {
                kind: "external_calls".into(),
                passed: false,
                message: format!(
                    "external \"{}\" has no mock (declare externals[].mock)",
                    call.external
                ),
                expected: None,
                actual: None,
            });
            continue;
        };
        let expected_body = call.body_contains.as_ref().map(yaml_to_json);
        let recorded = recorder.lock().map(|r| r.clone()).unwrap_or_default();
        let matches = recorded
            .iter()
            .filter(|r| {
                let method_ok = call
                    .method
                    .as_ref()
                    .map(|m| m.eq_ignore_ascii_case(&r.method))
                    .unwrap_or(true);
                let path_ok = r.path == call.path;
                let body_ok = expected_body
                    .as_ref()
                    .map(|exp| matches_subset(exp, &r.json()))
                    .unwrap_or(true);
                method_ok && path_ok && body_ok
            })
            .count();

        let (passed, msg) = match call.times {
            Some(n) => (
                matches == n,
                format!(
                    "{} {} on \"{}\": {matches} call(s), expected {n}",
                    call.method.as_deref().unwrap_or("ANY"),
                    call.path,
                    call.external
                ),
            ),
            None => (
                matches >= 1,
                format!(
                    "{} {} on \"{}\": {matches} matching call(s)",
                    call.method.as_deref().unwrap_or("ANY"),
                    call.path,
                    call.external
                ),
            ),
        };
        out.push(AssertionResult {
            kind: "external_calls".into(),
            passed,
            message: msg,
            expected: call.times.map(Json::from),
            actual: Some(Json::from(matches)),
        });
    }
    out
}

/// Produce every action in a plan, distributing them round-robin across
/// `concurrency` producer threads (each with its own Kafka producer, so writes
/// genuinely race). Returns the number produced, or (produced_so_far, error).
fn produce_plan(
    host: &str,
    plan: &crate::edgecases::ScenarioPlan,
) -> std::result::Result<usize, (usize, String)> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let concurrency = plan.concurrency.max(1);
    let sent = AtomicUsize::new(0);
    let delay = Duration::from_millis(plan.per_message_delay_ms);

    // Auto-create every distinct topic once (the Producer won't) before the
    // producer threads flood it, so the first sends don't hit UnknownTopic.
    let deadline = Instant::now() + Duration::from_secs(KAFKA_TOPIC_READY_SECS);
    let mut topics: Vec<&str> = plan.actions.iter().map(|a| a.topic.as_str()).collect();
    topics.sort_unstable();
    topics.dedup();
    for topic in topics {
        if let Err(e) = kafka_ensure_topic(host, topic, deadline) {
            return Err((0, e));
        }
    }

    let result = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for t in 0..concurrency {
            let host = host.to_string();
            let actions = &plan.actions;
            let sent = &sent;
            handles.push(scope.spawn(move || -> std::result::Result<(), String> {
                use kafka::producer::{Producer, RequiredAcks};
                let mut producer = Producer::from_hosts(vec![host])
                    .with_ack_timeout(Duration::from_secs(10))
                    .with_required_acks(RequiredAcks::One)
                    .create()
                    .map_err(|e| e.to_string())?;
                // Round-robin: thread `t` owns actions t, t+concurrency, ...
                let mut idx = t;
                while idx < actions.len() {
                    let a = &actions[idx];
                    let payload = serde_json::to_vec(&a.payload).map_err(|e| e.to_string())?;
                    // Retry the first send while the (possibly auto-created) topic
                    // becomes ready; later sends return immediately.
                    let deadline = Instant::now() + Duration::from_secs(KAFKA_TOPIC_READY_SECS);
                    kafka_send_retry(
                        &mut producer,
                        &a.topic,
                        a.key.as_deref().map(|k| k.as_bytes()),
                        payload.as_slice(),
                        deadline,
                    )
                    .map_err(|e| e.to_string())?;
                    sent.fetch_add(1, Ordering::Relaxed);
                    if !delay.is_zero() {
                        sleep(delay);
                    }
                    idx += concurrency;
                }
                Ok(())
            }));
        }
        let mut first_err: Option<String> = None;
        for h in handles {
            match h.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    first_err.get_or_insert(e);
                }
                Err(_) => {
                    first_err.get_or_insert_with(|| "producer thread panicked".to_string());
                }
            }
        }
        first_err
    });

    let n = sent.load(Ordering::Relaxed);
    match result {
        None => Ok(n),
        Some(e) => Err((n, e)),
    }
}

/// Consume from a topic and check a matching message arrives — used to verify a
/// producer the feature under test added.
fn kafka_expect(check: &CheckSpec, ctx: &RunnerContext, out: &mut Vec<AssertionResult>) {
    let Some(kafka) = &check.kafka else { return };
    let Some(expect) = &kafka.expect_message else { return };
    let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Kafka) else {
        out.push(AssertionResult {
            kind: "kafka".into(),
            passed: false,
            message: "kafka expect_message declared but no Kafka service is running".into(),
            expected: None,
            actual: None,
        });
        return;
    };

    let host = format!("127.0.0.1:{}", ep.host_port);
    let expected = yaml_to_json(&expect.contains);
    let timeout = Duration::from_millis(expect.timeout_ms.unwrap_or(5000));
    let expected_for_match = expected.clone();

    let found = (|| -> anyhow::Result<bool> {
        use kafka::consumer::{Consumer, FetchOffset, GroupOffsetStorage};
        let deadline = Instant::now() + timeout;
        // Make sure the topic exists (auto-create) before subscribing, so a
        // consumer-only check doesn't fail just because nothing produced yet.
        let ensure_deadline =
            Instant::now() + Duration::from_secs(KAFKA_TOPIC_READY_SECS).min(timeout.max(Duration::from_secs(5)));
        kafka_ensure_topic(&host, &expect.topic, ensure_deadline).map_err(|e| anyhow::anyhow!(e))?;
        // Build the consumer, retrying while a just-created topic propagates.
        let mut consumer = loop {
            match Consumer::from_hosts(vec![host.clone()])
                .with_topic(expect.topic.clone())
                .with_fallback_offset(FetchOffset::Earliest)
                .with_group(unique_group())
                // Required by the kafka crate once a group is set — without it,
                // polling fails with "Operation requires offset storage but no
                // offset storage was set".
                .with_offset_storage(Some(GroupOffsetStorage::Kafka))
                .create()
            {
                Ok(c) => break c,
                Err(e) => {
                    if Instant::now() >= deadline || !kafka_err_is_transient(&e) {
                        return Err(e.into());
                    }
                    sleep(Duration::from_millis(400));
                }
            }
        };
        while Instant::now() < deadline {
            // A transient error here just means the topic isn't fully ready —
            // keep polling until the deadline rather than failing the check.
            match consumer.poll() {
                Ok(sets) => {
                    for set in sets.iter() {
                        for m in set.messages() {
                            let parsed: Json = serde_json::from_slice(m.value).unwrap_or_else(|_| {
                                Json::String(String::from_utf8_lossy(m.value).into_owned())
                            });
                            if matches_subset(&expected_for_match, &parsed) {
                                return Ok(true);
                            }
                        }
                    }
                }
                Err(e) if kafka_err_is_transient(&e) => {}
                Err(e) => return Err(e.into()),
            }
            sleep(Duration::from_millis(200));
        }
        Ok(false)
    })();

    match found {
        Ok(true) => out.push(AssertionResult {
            kind: "kafka".into(),
            passed: true,
            message: format!("matching message received on topic \"{}\"", expect.topic),
            expected: Some(expected),
            actual: None,
        }),
        Ok(false) => out.push(AssertionResult {
            kind: "kafka".into(),
            passed: false,
            message: format!(
                "no matching message on topic \"{}\" within {}ms",
                expect.topic,
                timeout.as_millis()
            ),
            expected: Some(expected),
            actual: None,
        }),
        Err(e) => out.push(AssertionResult {
            kind: "kafka".into(),
            passed: false,
            message: format!("kafka consume failed on topic \"{}\": {e}", expect.topic),
            expected: Some(expected),
            actual: None,
        }),
    }
}

// ---------------------------------------------------------------------------
// Elasticsearch
// ---------------------------------------------------------------------------

fn fail_elastic(message: String) -> AssertionResult {
    AssertionResult { kind: "elastic".into(), passed: false, message, expected: None, actual: None }
}

fn es_request(
    base: &str,
    method: &str,
    path: &str,
    body: Option<&Json>,
) -> anyhow::Result<(u16, String)> {
    let url = format!("{base}{path}");
    let agent = ureq::builder().timeout(Duration::from_secs(15)).build();
    let req = agent.request(method, &url).set("content-type", "application/json");
    let result = match body {
        Some(b) => req.send_string(&serde_json::to_string(b)?),
        None => req.call(),
    };
    match result {
        Ok(resp) => Ok((resp.status(), resp.into_string().unwrap_or_default())),
        Err(ureq::Error::Status(code, resp)) => Ok((code, resp.into_string().unwrap_or_default())),
        Err(ureq::Error::Transport(t)) => anyhow::bail!("Elasticsearch request failed: {t}"),
    }
}

fn elastic_base_from(endpoints: &[ServiceEndpoint]) -> Option<String> {
    endpoints
        .iter()
        .find(|e| e.kind == ServiceKind::Elastic)
        .map(|e| format!("http://127.0.0.1:{}", e.host_port))
}

fn elastic_base(ctx: &RunnerContext) -> Option<String> {
    elastic_base_from(&ctx.endpoints)
}

/// Apply the index templates declared by the selected checks. Run BEFORE the
/// app starts so app-created indices pick up the mapping. Uses `_index_template`
/// (ES 7.8+/8) or the legacy `_template` when `legacy: true`.
pub fn apply_elastic_templates(checks: &[CheckSpec], endpoints: &[ServiceEndpoint]) {
    let Some(base) = elastic_base_from(endpoints) else { return };
    for check in checks {
        let Some(e) = &check.elastic else { continue };
        let Some(tpl) = &e.template else { continue };
        let body = yaml_to_json(&tpl.body);
        let path = if tpl.legacy {
            format!("/_template/{}", tpl.name)
        } else {
            format!("/_index_template/{}", tpl.name)
        };
        match es_request(&base, "PUT", &path, Some(&body)) {
            Ok((s, _)) if (200..300).contains(&s) => {
                crate::report::ok(&format!("applied Elasticsearch template \"{}\"", tpl.name));
            }
            Ok((s, b)) => crate::report::warn(&format!(
                "Elasticsearch template \"{}\" -> HTTP {s}: {}",
                tpl.name,
                b.chars().take(200).collect::<String>()
            )),
            Err(e) => crate::report::warn(&format!("Elasticsearch template \"{}\" failed: {e}", tpl.name)),
        }
    }
}

fn es_ok(message: String) -> AssertionResult {
    AssertionResult { kind: "elastic".into(), passed: true, message, expected: None, actual: None }
}

fn exec_elastic_op(base: &str, index: &str, op: &crate::spec::ElasticOp) -> AssertionResult {
    if let Some(ins) = &op.insert {
        let doc = yaml_to_json(&ins.document);
        let (method, path) = match &ins.id {
            Some(i) => ("PUT", format!("/{index}/_doc/{i}?refresh=true")),
            None => ("POST", format!("/{index}/_doc?refresh=true")),
        };
        return match es_request(base, method, &path, Some(&doc)) {
            Ok((s, _)) if (200..300).contains(&s) => es_ok(format!("indexed document into \"{index}\"")),
            Ok((s, b)) => fail_elastic(format!("insert into \"{index}\" -> HTTP {s}: {}", b.chars().take(200).collect::<String>())),
            Err(e) => fail_elastic(format!("insert failed: {e}")),
        };
    }
    if let Some(up) = &op.update {
        let body = serde_json::json!({ "doc": yaml_to_json(&up.document) });
        let path = format!("/{index}/_update/{}?refresh=true", up.id);
        return match es_request(base, "POST", &path, Some(&body)) {
            Ok((s, _)) if (200..300).contains(&s) => es_ok(format!("updated document \"{}\" in \"{index}\"", up.id)),
            Ok((s, b)) => fail_elastic(format!("update \"{}\" -> HTTP {s}: {}", up.id, b.chars().take(200).collect::<String>())),
            Err(e) => fail_elastic(format!("update failed: {e}")),
        };
    }
    if let Some(del) = &op.delete {
        let path = format!("/{index}/_doc/{}?refresh=true", del.id);
        return match es_request(base, "DELETE", &path, None) {
            // 404 = already absent; treat delete as idempotent success.
            Ok((s, _)) if (200..300).contains(&s) || s == 404 => es_ok(format!("deleted document \"{}\" from \"{index}\"", del.id)),
            Ok((s, b)) => fail_elastic(format!("delete \"{}\" -> HTTP {s}: {}", del.id, b.chars().take(200).collect::<String>())),
            Err(e) => fail_elastic(format!("delete failed: {e}")),
        };
    }
    fail_elastic("operation has no insert/update/delete".into())
}

fn verify_elastic(base: &str, e: &crate::spec::ElasticAssertion) -> Vec<AssertionResult> {
    let mut out = Vec::new();

    if let Some(id) = &e.doc_id {
        match es_request(base, "GET", &format!("/{}/_doc/{}", e.index, id), None) {
            Ok((status, body)) => {
                let found = status == 200;
                if let Some(want) = e.expect_exists {
                    out.push(AssertionResult {
                        kind: "elastic".into(),
                        passed: found == want,
                        message: if found == want {
                            format!("document \"{id}\" exists = {found}")
                        } else {
                            format!("document \"{id}\": expected exists={want}, got {found}")
                        },
                        expected: Some(Json::Bool(want)),
                        actual: Some(Json::Bool(found)),
                    });
                }
                if let Some(exp) = &e.expect_source_contains {
                    let parsed: Json = serde_json::from_str(&body).unwrap_or(Json::Null);
                    let source = parsed.get("_source").cloned().unwrap_or(Json::Null);
                    let expected = yaml_to_json(exp);
                    let passed = matches_subset(&expected, &source);
                    out.push(AssertionResult {
                        kind: "elastic".into(),
                        passed,
                        message: if passed {
                            format!("document \"{id}\" _source contains expected fields")
                        } else {
                            format!("document \"{id}\" _source did not contain expected fields")
                        },
                        expected: Some(expected),
                        actual: Some(truncate_json(source)),
                    });
                }
            }
            Err(err) => out.push(fail_elastic(err.to_string())),
        }
    }

    if let Some(q) = &e.query {
        let body = serde_json::json!({ "query": yaml_to_json(q) });
        match es_request(base, "POST", &format!("/{}/_search", e.index), Some(&body)) {
            Ok((_status, resp)) => {
                let parsed: Json = serde_json::from_str(&resp).unwrap_or(Json::Null);
                // hits.total.value (object form) or hits.total (rest_total_hits_as_int).
                let hits = parsed
                    .pointer("/hits/total/value")
                    .and_then(|v| v.as_i64())
                    .or_else(|| parsed.pointer("/hits/total").and_then(|v| v.as_i64()))
                    .unwrap_or(-1);
                if let Some(expect) = e.expect_hits {
                    out.push(AssertionResult {
                        kind: "elastic".into(),
                        passed: hits == expect,
                        message: if hits == expect {
                            format!("search returned {hits} hit(s)")
                        } else {
                            format!("expected {expect} hit(s), got {hits}")
                        },
                        expected: Some(Json::from(expect)),
                        actual: Some(Json::from(hits)),
                    });
                }
            }
            Err(err) => out.push(fail_elastic(err.to_string())),
        }
    }

    out
}

fn run_elastic(check: &CheckSpec, ctx: &RunnerContext, retry: Duration, out: &mut Vec<AssertionResult>) {
    let Some(e) = &check.elastic else { return };
    let Some(base) = elastic_base(ctx) else {
        out.push(fail_elastic("elastic assertion but no Elasticsearch service is running".into()));
        return;
    };

    // noworries-run operations (setup / trigger), executed in order.
    for op in &e.operations {
        out.push(exec_elastic_op(&base, &e.index, op));
    }

    // Verification (of the operations' effect and/or the app's writes), retried
    // for eventual consistency.
    let has_verification =
        e.doc_id.is_some() || e.query.is_some();
    if !has_verification {
        return;
    }
    let deadline = Instant::now() + retry;
    let mut last = verify_elastic(&base, e);
    while !last.iter().all(|a| a.passed) && Instant::now() < deadline {
        sleep(Duration::from_millis(400));
        last = verify_elastic(&base, e);
    }
    out.extend(last);
}

// ---------------------------------------------------------------------------
// MySQL  (seed -> [request] -> verify)
// ---------------------------------------------------------------------------

fn fail_mysql(message: String) -> AssertionResult {
    AssertionResult { kind: "mysql".into(), passed: false, message, expected: None, actual: None }
}

pub(crate) fn mysql_conn(port: u16) -> mysql::Result<mysql::Conn> {
    use crate::services::mysql::{MYSQL_DB, MYSQL_PASSWORD, MYSQL_USER};
    let url = format!("mysql://{MYSQL_USER}:{MYSQL_PASSWORD}@127.0.0.1:{port}/{MYSQL_DB}");
    mysql::Conn::new(mysql::Opts::from_url(&url)?)
}

fn mysql_value_to_json(v: mysql::Value) -> Json {
    match v {
        mysql::Value::NULL => Json::Null,
        mysql::Value::Bytes(b) => Json::String(String::from_utf8_lossy(&b).into_owned()),
        mysql::Value::Int(i) => Json::from(i),
        mysql::Value::UInt(u) => Json::from(u),
        mysql::Value::Float(f) => serde_json::to_value(f).unwrap_or(Json::Null),
        mysql::Value::Double(d) => serde_json::to_value(d).unwrap_or(Json::Null),
        mysql::Value::Date(y, mo, d, h, mi, s, us) => {
            Json::String(format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{us:06}"))
        }
        mysql::Value::Time(neg, days, h, mi, s, us) => Json::String(format!(
            "{}{days}d {h:02}:{mi:02}:{s:02}.{us:06}",
            if neg { "-" } else { "" }
        )),
    }
}

fn mysql_row_to_json(row: &mysql::Row) -> Json {
    let mut map = serde_json::Map::new();
    for (i, col) in row.columns_ref().iter().enumerate() {
        let val: mysql::Value = row.get(i).unwrap_or(mysql::Value::NULL);
        map.insert(col.name_str().to_string(), mysql_value_to_json(val));
    }
    Json::Object(map)
}

/// Run the check's MySQL `seed` statements. Called before the request.
fn run_mysql_seed(check: &CheckSpec, ctx: &RunnerContext, out: &mut Vec<AssertionResult>) {
    use mysql::prelude::Queryable;
    let Some(m) = &check.mysql else { return };
    if m.seed.is_empty() {
        return;
    }
    let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Mysql) else {
        out.push(fail_mysql("mysql seed but no MySQL service is running".into()));
        return;
    };
    let mut conn = match mysql_conn(ep.host_port) {
        Ok(c) => c,
        Err(e) => {
            out.push(fail_mysql(format!("could not connect to MySQL: {e}")));
            return;
        }
    };
    for stmt in &m.seed {
        match conn.query_drop(stmt) {
            Ok(()) => out.push(AssertionResult {
                kind: "mysql".into(),
                passed: true,
                message: format!("seeded: {}", stmt.chars().take(60).collect::<String>()),
                expected: None,
                actual: None,
            }),
            Err(e) => out.push(fail_mysql(format!("seed failed ({stmt}): {e}"))),
        }
    }
}

fn evaluate_mysql(m: &crate::spec::MySqlAssertion, port: u16) -> Vec<AssertionResult> {
    use mysql::prelude::Queryable;
    let mut out = Vec::new();
    let Some(query) = &m.query else { return out };
    let mut conn = match mysql_conn(port) {
        Ok(c) => c,
        Err(e) => {
            out.push(fail_mysql(format!("could not connect to MySQL: {e}")));
            return out;
        }
    };
    let rows: Vec<mysql::Row> = match conn.query(query) {
        Ok(r) => r,
        Err(e) => {
            out.push(fail_mysql(format!("query failed: {e}")));
            return out;
        }
    };

    if let Some(expected_count) = m.expect_row_count {
        let passed = rows.len() as i64 == expected_count;
        out.push(AssertionResult {
            kind: "mysql".into(),
            passed,
            message: if passed {
                format!("query returned {} row(s)", rows.len())
            } else {
                format!("expected {expected_count} row(s), got {}", rows.len())
            },
            expected: Some(Json::from(expected_count)),
            actual: Some(Json::from(rows.len() as i64)),
        });
    }
    if let Some(expect_row) = &m.expect_row {
        let expected = yaml_to_json(expect_row);
        match rows.first() {
            Some(row) => {
                let actual = mysql_row_to_json(row);
                let passed = matches_subset(&expected, &actual);
                out.push(AssertionResult {
                    kind: "mysql".into(),
                    passed,
                    message: if passed {
                        "first row matches expected fields".into()
                    } else {
                        "first row did not match expected fields".into()
                    },
                    expected: Some(expected),
                    actual: Some(actual),
                });
            }
            None => out.push(AssertionResult {
                kind: "mysql".into(),
                passed: false,
                message: "expected a matching row, but query returned no rows".into(),
                expected: Some(expected),
                actual: Some(Json::Null),
            }),
        }
    }
    out
}

fn run_mysql_verify(check: &CheckSpec, ctx: &RunnerContext, retry: Duration, out: &mut Vec<AssertionResult>) {
    let Some(m) = &check.mysql else { return };
    if m.query.is_none() {
        return;
    }
    let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Mysql) else {
        out.push(fail_mysql("mysql assertion but no MySQL service is running".into()));
        return;
    };
    let deadline = Instant::now() + retry;
    let mut last = evaluate_mysql(m, ep.host_port);
    while !last.iter().all(|a| a.passed) && Instant::now() < deadline {
        sleep(Duration::from_millis(400));
        last = evaluate_mysql(m, ep.host_port);
    }
    out.extend(last);
}

// ---------------------------------------------------------------------------
// MongoDB  (seed -> [request] -> verify)
// ---------------------------------------------------------------------------

fn fail_mongo(message: String) -> AssertionResult {
    AssertionResult { kind: "mongodb".into(), passed: false, message, expected: None, actual: None }
}

fn mongo_client(port: u16) -> mongodb::error::Result<mongodb::sync::Client> {
    mongodb::sync::Client::with_uri_str(format!("mongodb://127.0.0.1:{port}"))
}

fn yaml_to_bson_doc(v: &serde_yaml::Value) -> anyhow::Result<mongodb::bson::Document> {
    let json = yaml_to_json(v);
    mongodb::bson::to_document(&json).map_err(|e| anyhow::anyhow!("expected a document: {e}"))
}

fn run_mongo_seed(check: &CheckSpec, ctx: &RunnerContext, out: &mut Vec<AssertionResult>) {
    use mongodb::bson::{Bson, Document};
    let Some(m) = &check.mongodb else { return };
    if m.seed.is_empty() {
        return;
    }
    let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Mongodb) else {
        out.push(fail_mongo("mongodb seed but no MongoDB service is running".into()));
        return;
    };
    let client = match mongo_client(ep.host_port) {
        Ok(c) => c,
        Err(e) => {
            out.push(fail_mongo(format!("could not connect to MongoDB: {e}")));
            return;
        }
    };
    let coll = client.database(&m.database).collection::<Document>(&m.collection);

    for op in &m.seed {
        let result: anyhow::Result<String> = (|| {
            if let Some(doc) = &op.insert {
                coll.insert_one(yaml_to_bson_doc(doc)?, None)?;
                Ok(format!("inserted into {}.{}", m.database, m.collection))
            } else if let Some(up) = &op.update {
                let mut update = Document::new();
                update.insert("$set", Bson::Document(yaml_to_bson_doc(&up.set)?));
                let r = coll.update_many(yaml_to_bson_doc(&up.filter)?, update, None)?;
                Ok(format!("updated {} doc(s)", r.modified_count))
            } else if let Some(del) = &op.delete {
                let r = coll.delete_many(yaml_to_bson_doc(del)?, None)?;
                Ok(format!("deleted {} doc(s)", r.deleted_count))
            } else {
                anyhow::bail!("operation has no insert/update/delete")
            }
        })();
        match result {
            Ok(msg) => out.push(AssertionResult {
                kind: "mongodb".into(),
                passed: true,
                message: msg,
                expected: None,
                actual: None,
            }),
            Err(e) => out.push(fail_mongo(format!("seed op failed: {e}"))),
        }
    }
}

fn evaluate_mongo(m: &crate::spec::MongoAssertion, port: u16) -> Vec<AssertionResult> {
    use mongodb::bson::Document;
    let mut out = Vec::new();
    let client = match mongo_client(port) {
        Ok(c) => c,
        Err(e) => {
            out.push(fail_mongo(format!("could not connect to MongoDB: {e}")));
            return out;
        }
    };
    let coll = client.database(&m.database).collection::<Document>(&m.collection);
    let filter = match m.find.as_ref().map(yaml_to_bson_doc).transpose() {
        Ok(f) => f.unwrap_or_default(),
        Err(e) => {
            out.push(fail_mongo(format!("invalid find filter: {e}")));
            return out;
        }
    };

    if let Some(expect_count) = m.expect_count {
        match coll.count_documents(filter.clone(), None) {
            Ok(n) => out.push(AssertionResult {
                kind: "mongodb".into(),
                passed: n as i64 == expect_count,
                message: if n as i64 == expect_count {
                    format!("matched {n} document(s)")
                } else {
                    format!("expected {expect_count} document(s), got {n}")
                },
                expected: Some(Json::from(expect_count)),
                actual: Some(Json::from(n as i64)),
            }),
            Err(e) => out.push(fail_mongo(format!("count failed: {e}"))),
        }
    }

    if let Some(exp) = &m.expect_doc_contains {
        let expected = yaml_to_json(exp);
        match coll.find(filter, None) {
            Ok(mut cursor) => match cursor.next() {
                Some(Ok(doc)) => {
                    let actual = serde_json::to_value(&doc).unwrap_or(Json::Null);
                    let passed = matches_subset(&expected, &actual);
                    out.push(AssertionResult {
                        kind: "mongodb".into(),
                        passed,
                        message: if passed {
                            "first document contains expected fields".into()
                        } else {
                            "first document did not contain expected fields".into()
                        },
                        expected: Some(expected),
                        actual: Some(truncate_json(actual)),
                    });
                }
                Some(Err(e)) => out.push(fail_mongo(format!("cursor error: {e}"))),
                None => out.push(AssertionResult {
                    kind: "mongodb".into(),
                    passed: false,
                    message: "expected a matching document, but the query returned none".into(),
                    expected: Some(expected),
                    actual: Some(Json::Null),
                }),
            },
            Err(e) => out.push(fail_mongo(format!("find failed: {e}"))),
        }
    }
    out
}

fn run_mongo_verify(check: &CheckSpec, ctx: &RunnerContext, retry: Duration, out: &mut Vec<AssertionResult>) {
    let Some(m) = &check.mongodb else { return };
    if m.find.is_none() && m.expect_count.is_none() {
        return;
    }
    let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Mongodb) else {
        out.push(fail_mongo("mongodb assertion but no MongoDB service is running".into()));
        return;
    };
    let deadline = Instant::now() + retry;
    let mut last = evaluate_mongo(m, ep.host_port);
    while !last.iter().all(|a| a.passed) && Instant::now() < deadline {
        sleep(Duration::from_millis(400));
        last = evaluate_mongo(m, ep.host_port);
    }
    out.extend(last);
}

/// Run every selected check sequentially and collect results.
/// How long the observe assertions should retry for eventual consistency.
/// A plain async effect gets ~6s; an edge-case scenario floods the pipeline, so
/// the window grows with the message count (a 500-burst can take longer to
/// drain than a single write) — capped so a hang still fails in bounded time.
fn observe_retry_budget(check: &CheckSpec, async_effect: bool) -> Duration {
    if !async_effect {
        return Duration::from_secs(0);
    }
    let base = 6u64;
    let extra = check
        .scenario
        .as_ref()
        .map(|s| (s.count.unwrap_or(100) as u64) / 50)
        .unwrap_or(0);
    Duration::from_secs((base + extra).min(120))
}

pub fn run_checks(checks: &[CheckSpec], ctx: &RunnerContext) -> Vec<CheckResult> {
    let mut results = Vec::new();
    for check in checks {
        let mut assertions = Vec::new();
        let mut error = None;

        // SEED phase: write initial data BEFORE the request, so a feature that
        // reads-then-updates has something to act on ("seed -> hit API -> check").
        run_mysql_seed(check, ctx, &mut assertions);
        run_mongo_seed(check, ctx, &mut assertions);

        if let Err(e) = run_http(check, ctx, &mut assertions) {
            error = Some(e.to_string());
        } else {
            // Actions that trigger behaviour run before assertions that observe
            // it. A Kafka produce (which fires an async consumer) means the DB
            // assertion should be patient (retry) for eventual consistency.
            kafka_produce(check, ctx, &mut assertions);
            // Edge-case scenarios (burst / concurrent / duplicates / out_of_order)
            // are a richer trigger — they flood Kafka to stress the pipeline.
            run_scenario(check, ctx, &mut assertions);
            // Any async trigger (a Kafka produce, a scenario flood, or an HTTP
            // call that kicks off background work) means the DB/Redis observers
            // should be patient.
            let async_effect = check.request.is_some()
                || check.scenario.is_some()
                || check
                    .kafka
                    .as_ref()
                    .map(|k| k.produce.is_some())
                    .unwrap_or(false)
                // Eventual-consistency observers deserve the retry window even
                // without an explicit trigger in the same check (the effect may
                // have been set up by a prior check or an already-running app).
                || check.metrics.is_some()
                || check.traces.is_some()
                || check.schema.is_some();
            let retry_budget = observe_retry_budget(check, async_effect);
            run_db(check, ctx, retry_budget, &mut assertions);
            run_mysql_verify(check, ctx, retry_budget, &mut assertions);
            run_mongo_verify(check, ctx, retry_budget, &mut assertions);
            run_redis(check, ctx, retry_budget, &mut assertions);
            run_elastic(check, ctx, retry_budget, &mut assertions);
            kafka_expect(check, ctx, &mut assertions);
            // Observe the app's side effects: what it logged and which mocked
            // externals it called (both may lag the trigger → retry-aware).
            run_external_calls(check, ctx, retry_budget, &mut assertions);
            run_logs(check, ctx, retry_budget, &mut assertions);
            // Extensible assertion types (graphql / metrics / snapshot / schema /
            // sse / websocket / grpc / traces) — each pluggable via the checks
            // registry (open/closed: new type = new file + one registry line).
            crate::checks::run_all(check, ctx, retry_budget, &mut assertions);
            if assertions.is_empty() {
                error = Some("check declares no assertions (add request/expect, db, ...)".into());
            }
        }

        let passed = error.is_none() && !assertions.is_empty() && assertions.iter().all(|a| a.passed);
        results.push(CheckResult {
            name: check.name.clone(),
            passed,
            assertions,
            error,
        });
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subset_matches_nested_objects() {
        let exp = json!({"status": "PENDING"});
        let act = json!({"id": 1, "status": "PENDING", "sku": "ABC"});
        assert!(matches_subset(&exp, &act));
    }

    #[test]
    fn subset_rejects_mismatch() {
        let exp = json!({"status": "PENDING"});
        let act = json!({"status": "NEW"});
        assert!(!matches_subset(&exp, &act));
    }

    #[test]
    fn subset_matches_array_prefix_and_loose_scalars() {
        let exp = json!([{"sku": "ABC"}]);
        let act = json!([{"sku": "ABC", "qty": 2}, {"sku": "XYZ"}]);
        assert!(matches_subset(&exp, &act));
        // loose scalar: yaml number vs json string
        assert!(loose_equal(&json!(201), &json!("201")));
    }

    #[test]
    fn subset_rejects_when_actual_missing_key() {
        let exp = json!({"status": "PENDING"});
        let act = json!({"other": 1});
        assert!(!matches_subset(&exp, &act));
    }

    #[test]
    fn retry_budget_scales_with_scenario_and_is_capped() {
        use crate::spec::NoworriesSpec;
        let plain = &NoworriesSpec::parse(
            "version: 1\nservices: [postgres]\nchecks:\n  - name: x\n    db: { query: \"SELECT 1\" }\n",
        )
        .unwrap()
        .checks[0];
        // No async effect => no retry.
        assert_eq!(observe_retry_budget(plain, false), Duration::from_secs(0));
        // Async but no scenario => base 6s.
        assert_eq!(observe_retry_budget(plain, true), Duration::from_secs(6));

        let big = &NoworriesSpec::parse(
            "version: 1\nservices: [kafka, postgres]\nchecks:\n  - name: b\n    scenario: { kind: burst, count: 500, kafka: { topic: t, message: { id: \"${seq}\" } } }\n    db: { query: \"SELECT 1\" }\n",
        )
        .unwrap()
        .checks[0];
        // 6 + 500/50 = 16s.
        assert_eq!(observe_retry_budget(big, true), Duration::from_secs(16));

        let huge = &NoworriesSpec::parse(
            "version: 1\nservices: [kafka, postgres]\nchecks:\n  - name: h\n    scenario: { kind: burst, count: 100000, kafka: { topic: t, message: { id: \"${seq}\" } } }\n    db: { query: \"SELECT 1\" }\n",
        )
        .unwrap()
        .checks[0];
        // capped at 120s.
        assert_eq!(observe_retry_budget(huge, true), Duration::from_secs(120));
    }

    #[test]
    fn parse_env_file_handles_comments_and_quotes() {
        let content = "# c\nA=1\nB = two words \nC=\"quoted\"\nD='q2'\n\nBAD_LINE\n";
        let m = parse_env_file(content);
        assert_eq!(m.get("A").unwrap(), "1");
        assert_eq!(m.get("B").unwrap(), "two words");
        assert_eq!(m.get("C").unwrap(), "quoted");
        assert_eq!(m.get("D").unwrap(), "q2");
        assert!(!m.contains_key("BAD_LINE"));
    }

    #[test]
    fn interpolate_expands_from_env_map() {
        let mut env = BTreeMap::new();
        env.insert("TOKEN".to_string(), "abc123".to_string());
        assert_eq!(interpolate("Bearer ${TOKEN}", &env), "Bearer abc123");
        // unknown var is left verbatim
        assert_eq!(interpolate("x ${NOPE} y", &env), "x ${NOPE} y");
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b"user:pass"), "dXNlcjpwYXNz");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
    }

    #[test]
    fn json_path_traverses_objects_and_arrays() {
        let v = json!({"data": {"tokens": ["t0", "t1"]}});
        assert_eq!(json_path(&v, "$.data.tokens.1").unwrap(), &json!("t1"));
        assert_eq!(json_path(&v, "data.tokens.0").unwrap(), &json!("t0"));
        assert!(json_path(&v, "data.missing").is_none());
    }

    #[test]
    fn resolve_basic_bearer_apikey_without_app() {
        use crate::spec::{AuthApiKey, AuthBasic, AuthBearer, AuthSpec};
        let env = BTreeMap::new();
        let auth = AuthSpec {
            basic: Some(AuthBasic { username: "user".into(), password: "pass".into() }),
            bearer: Some(AuthBearer { token: "T".into(), header: None, scheme: None }),
            api_key: Some(AuthApiKey { header: Some("X-API-Key".into()), query: None, value: "K".into() }),
            login: None,
        };
        let r = resolve_auth(Some(&auth), 0, &env).unwrap();
        assert!(r.headers.iter().any(|(k, v)| k == "Authorization" && v == "Basic dXNlcjpwYXNz"));
        assert!(r.headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer T"));
        assert!(r.headers.iter().any(|(k, v)| k == "X-API-Key" && v == "K"));
    }
}
