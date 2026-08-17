//! Defensive security assertion — abuse-case verification of the app under test.
//!
//! This checks that *your own* endpoint behaves safely under hostile input; it
//! only ever talks to the app noworries started (localhost, ephemeral) and it
//! asserts **safe behaviour**, it does not exploit anything:
//!
//! - `require_auth` — a request with the auth stripped is rejected (401/403).
//! - `reject_bad_input` — malformed / hostile input never causes a 5xx crash, and a malformed body is rejected with a 4xx.
//! - `no_error_leak` — responses don't leak stack traces / DB error internals.
//! - `require_headers` — declared security response headers are present.
//!
//! The "hostile" inputs are benign probe strings used to assert the app handles
//! them gracefully (reject or safely ignore) — not to break into anything.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value as Json;

use super::{fail, pass, yaml_json, Assertion};
use crate::runner::{interpolate, interpolate_json, AssertionResult, RunnerContext};
use crate::spec::CheckSpec;

pub struct Security;

/// Probe strings that must be handled without a server error. Each is a classic
/// abuse pattern; the assertion is that the app rejects or safely ignores it,
/// never that it "works".
const HOSTILE: &[(&str, &str)] = &[
    ("sql-ish", "' OR '1'='1' --"),
    ("script-ish", "\"><script>alert(1)</script>"),
    ("path-traversal", "../../../../../../etc/passwd"),
    ("template-injection", "${jndi:ldap://127.0.0.1/a}"),
];

/// Signatures of leaked server internals (stack traces, DB driver errors). Kept
/// specific to avoid flagging ordinary content that merely says "error".
const LEAK_SIGNATURES: &[&str] = &[
    "Traceback (most recent call last)",
    "at java.",
    "at org.springframework",
    "org.hibernate",
    "goroutine ",
    "panic: ",
    "SQLSTATE",
    "syntax error at or near",
    "ORA-0",
    "System.Exception",
    "at System.",
    "Microsoft.Data.SqlClient",
    "Fatal error:", // PHP
    "Stack trace:",
];

impl Assertion for Security {
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, _retry: Duration) -> Vec<AssertionResult> {
        let Some(s) = &check.security else { return vec![] };
        if ctx.app_port == 0 {
            return vec![fail(
                "security",
                "security check needs a running app (no app_port)".into(),
                None,
                None,
            )];
        }

        let path = s
            .path
            .clone()
            .or_else(|| check.request.as_ref().map(|r| r.path.clone()))
            .unwrap_or_else(|| "/".to_string());
        let path = interpolate(&path, &ctx.env);
        let method = s
            .method
            .clone()
            .or_else(|| check.request.as_ref().map(|r| r.method.clone()))
            .unwrap_or_else(|| "GET".to_string())
            .to_uppercase();
        let base_body: Option<Json> = s
            .body
            .clone()
            .or_else(|| check.request.as_ref().and_then(|r| r.body.clone()))
            .map(|b| interpolate_json(&yaml_json(&b), &ctx.env));

        let base_url = format!("http://127.0.0.1:{}{}", ctx.app_port, path);
        let (id_headers, id_query) = ctx.auth_for(check);
        let authed_url = with_query(&base_url, id_query);
        let auth_headers: BTreeMap<String, String> = id_headers.iter().cloned().collect();

        let mut out = Vec::new();

        // --- require_auth: strip all auth, expect a rejection ---------------
        if s.require_auth == Some(true) {
            let body = base_body.as_ref().map(|b| b.to_string());
            match send(&method, &base_url, &BTreeMap::new(), body.as_deref(), &[]) {
                Ok(p) if p.status == 401 || p.status == 403 => {
                    out.push(pass("security", format!("unauthenticated request rejected ({})", p.status)))
                }
                Ok(p) => out.push(fail(
                    "security",
                    format!("endpoint served an unauthenticated request: HTTP {}", p.status),
                    Some(Json::from("401 or 403")),
                    Some(Json::from(p.status)),
                )),
                Err(e) => out.push(fail("security", format!("auth probe failed: {e}"), None, None)),
            }
        }

        // --- hostile-input probes (shared by reject_bad_input & no_error_leak)
        let need_hostile = s.reject_bad_input == Some(true) || s.no_error_leak == Some(true);
        let mut probes: Vec<(String, Probe)> = Vec::new();
        if need_hostile {
            // Malformed body (broken JSON) — must be a 4xx, not a crash.
            if let Ok(p) = send(&method, &authed_url, &auth_headers, Some("{\"broken\": "), &[]) {
                probes.push(("malformed-json".to_string(), p));
            }
            // Oversized field.
            let big = "A".repeat(100_000);
            if let Ok(p) = send(&method, &authed_url, &auth_headers, Some(&mutate_body(&base_body, &big)), &[]) {
                probes.push(("oversized".to_string(), p));
            }
            // Classic abuse strings.
            for (label, payload) in HOSTILE {
                let body = mutate_body(&base_body, payload);
                if let Ok(p) = send(&method, &authed_url, &auth_headers, Some(&body), &[]) {
                    probes.push((label.to_string(), p));
                }
            }
        }

        // --- reject_bad_input: no 5xx, malformed => 4xx ---------------------
        if s.reject_bad_input == Some(true) {
            let mut crashed = Vec::new();
            for (label, p) in &probes {
                if p.status >= 500 {
                    crashed.push(format!("{label} → {}", p.status));
                }
            }
            if !crashed.is_empty() {
                out.push(fail(
                    "security",
                    format!("hostile input caused a server error: {}", crashed.join(", ")),
                    Some(Json::from("no 5xx")),
                    Some(Json::from(crashed.join(", "))),
                ));
            } else {
                out.push(pass(
                    "security",
                    format!("{} hostile input(s) handled without a 5xx", probes.len()),
                ));
            }
            if let Some((_, p)) = probes.iter().find(|(l, _)| l == "malformed-json") {
                if (400..500).contains(&p.status) {
                    out.push(pass("security", format!("malformed body rejected ({})", p.status)));
                } else {
                    out.push(fail(
                        "security",
                        format!("malformed body not rejected with 4xx (got {})", p.status),
                        Some(Json::from("4xx")),
                        Some(Json::from(p.status)),
                    ));
                }
            }
        }

        // --- no_error_leak: scan probe bodies for leaked internals ----------
        if s.no_error_leak == Some(true) {
            let mut leaked = None;
            for (label, p) in &probes {
                if let Some(sig) = leak_signature(&p.body) {
                    leaked = Some((label.clone(), sig));
                    break;
                }
            }
            match leaked {
                Some((label, sig)) => out.push(fail(
                    "security",
                    format!("response leaked server internals ({label}): matched \"{sig}\""),
                    None,
                    Some(Json::from(sig)),
                )),
                None => out.push(pass("security", "no stack traces / error internals leaked".into())),
            }
        }

        // --- require_headers: normal request, assert headers present --------
        if !s.require_headers.is_empty() {
            let body = base_body.as_ref().map(|b| b.to_string());
            match send(&method, &authed_url, &auth_headers, body.as_deref(), &s.require_headers) {
                Ok(p) => {
                    let missing: Vec<&String> =
                        s.require_headers.iter().filter(|h| !p.headers.contains_key(h.to_ascii_lowercase().as_str())).collect();
                    if missing.is_empty() {
                        out.push(pass("security", "all required security headers present".into()));
                    } else {
                        out.push(fail(
                            "security",
                            format!("missing security header(s): {}", missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")),
                            None,
                            None,
                        ));
                    }
                }
                Err(e) => out.push(fail("security", format!("header probe failed: {e}"), None, None)),
            }
        }

        if out.is_empty() {
            out.push(fail(
                "security",
                "security check declares no probes (set require_auth / reject_bad_input / no_error_leak / require_headers)".into(),
                None,
                None,
            ));
        }
        out
    }
}

struct Probe {
    status: u16,
    body: String,
    /// Requested headers found on the response, keyed lowercase.
    headers: BTreeMap<String, String>,
}

fn send(
    method: &str,
    url: &str,
    headers: &BTreeMap<String, String>,
    body: Option<&str>,
    want_headers: &[String],
) -> Result<Probe, String> {
    let agent = super::http_agent(Duration::from_secs(15));
    let mut req = agent.request(method, url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let res = match body {
        Some(b) => req.set("content-type", "application/json").send_string(b),
        None => req.call(),
    };
    let resp = match res {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(ureq::Error::Transport(t)) => return Err(format!("{t}")),
    };
    let status = resp.status();
    let mut hm = BTreeMap::new();
    for h in want_headers {
        if let Some(v) = resp.header(h) {
            hm.insert(h.to_ascii_lowercase(), v.to_string());
        }
    }
    let body = resp.into_string().unwrap_or_default();
    Ok(Probe { status, body, headers: hm })
}

/// Append auth query params to a URL (mirrors `run_http`).
fn with_query(url: &str, auth_query: &[(String, String)]) -> String {
    if auth_query.is_empty() {
        return url.to_string();
    }
    let sep = if url.contains('?') { '&' } else { '?' };
    let qs = auth_query.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
    format!("{url}{sep}{qs}")
}

/// Build a request body that carries the probe string: set the first string
/// field of a JSON object body to `value`; otherwise send `value` as the body.
fn mutate_body(base: &Option<Json>, value: &str) -> String {
    if let Some(Json::Object(map)) = base {
        let mut m = map.clone();
        if let Some(key) = m
            .iter()
            .find(|(_, v)| v.is_string())
            .map(|(k, _)| k.clone())
            .or_else(|| m.keys().next().cloned())
        {
            m.insert(key, Json::String(value.to_string()));
        }
        return Json::Object(m).to_string();
    }
    Json::String(value.to_string()).to_string()
}

/// First leaked-internals signature found in `body`, if any.
fn leak_signature(body: &str) -> Option<&'static str> {
    LEAK_SIGNATURES.iter().copied().find(|sig| body.contains(sig))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_leak_signatures() {
        assert_eq!(leak_signature("ok"), None);
        assert!(leak_signature("...\nTraceback (most recent call last):\n ...").is_some());
        assert!(leak_signature("ERROR: syntax error at or near \"SELCT\"").is_some());
        assert!(leak_signature("panic: runtime error: index out of range").is_some());
        // Ordinary content that merely mentions "error" must not trip it.
        assert_eq!(leak_signature("{\"error\":\"invalid sku\"}"), None);
    }

    #[test]
    fn mutate_sets_first_string_field() {
        let base = Some(serde_json::json!({ "qty": 2, "sku": "ABC" }));
        let out = mutate_body(&base, "' OR '1'='1");
        let v: Json = serde_json::from_str(&out).unwrap();
        assert_eq!(v.get("sku").and_then(|x| x.as_str()), Some("' OR '1'='1"));
        assert_eq!(v.get("qty").and_then(|x| x.as_i64()), Some(2)); // untouched
    }

    #[test]
    fn mutate_without_object_body_sends_string() {
        let out = mutate_body(&None, "x");
        assert_eq!(out, "\"x\"");
    }

    #[test]
    fn with_query_appends() {
        assert_eq!(with_query("http://h/p", &[]), "http://h/p");
        assert_eq!(
            with_query("http://h/p", &[("k".into(), "v".into())]),
            "http://h/p?k=v"
        );
        assert_eq!(
            with_query("http://h/p?a=1", &[("k".into(), "v".into())]),
            "http://h/p?a=1&k=v"
        );
    }
}
