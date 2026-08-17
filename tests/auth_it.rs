//! Auth integration test: real app subprocess with a login endpoint + a
//! protected endpoint, driving `resolve_auth` (login flow) + `run_checks`.
//!
//! Enabled when NOWORRIES_IT_AUTH_CMD points at the auth stub app.

use std::collections::BTreeMap;
use std::path::Path;

use noworries::app::start_app;
use noworries::runner::{resolve_auth, run_checks, RunnerContext};
use noworries::spec::{AppSpec, AuthLogin, AuthSpec, CheckSpec, HttpExpectSpec, HttpRequestSpec, NoworriesSpec};

fn protected_check() -> CheckSpec {
    let yaml = r#"
version: 1
services: [postgres]
checks:
  - name: "list orders (requires auth)"
    request: { method: GET, path: /orders }
    expect: { status: 200, body_contains: [ { sku: "ABC" } ] }
"#;
    NoworriesSpec::parse(yaml).unwrap().checks.remove(0)
}

fn login_auth() -> AuthSpec {
    // password comes from ${AUTH_PASS} to exercise env interpolation.
    let req: HttpRequestSpec = serde_yaml::from_str(
        r#"{ method: POST, path: /auth/login, body: { username: "test", password: "${AUTH_PASS}" } }"#,
    )
    .unwrap();
    AuthSpec {
        login: Some(AuthLogin {
            request: req,
            expect: Some(HttpExpectSpec { status: Some(200), body_contains: None, max_ms: None }),
            token_from: "$.accessToken".to_string(),
            header: None,
            scheme: None,
        }),
        bearer: None,
        oidc: None,
        basic: None,
        api_key: None,
    }
}

#[test]
fn login_flow_authenticates_protected_endpoint() {
    let Ok(cmd) = std::env::var("NOWORRIES_IT_AUTH_CMD") else {
        eprintln!("skipping auth IT: set NOWORRIES_IT_AUTH_CMD");
        return;
    };
    let out_dir = std::env::temp_dir().join("noworries-rs-auth-it");
    std::fs::create_dir_all(&out_dir).unwrap();
    let app_spec = AppSpec {
        start: Some(cmd),
        health: Some("/actuator/health".into()),
        ready_timeout: Some(20),
        ..Default::default()
    };
    let mut env = BTreeMap::new();
    env.insert("AUTH_PASS".to_string(), "secret".to_string());

    let mut app = start_app(Some(&app_spec), None, Path::new("."), &[], &std::collections::BTreeMap::new(), &std::collections::BTreeMap::new(), &out_dir).expect("app starts");

    // Without auth, the protected check should fail (401).
    let ctx_noauth = RunnerContext::new(app.port, vec![]);
    let unauth = run_checks(&[protected_check()], &ctx_noauth);
    let unauth_ok = unauth[0].passed;

    // With the login flow, it should pass.
    let auth = resolve_auth(Some(&login_auth()), app.port, &env).expect("login succeeds");
    let has_bearer = auth.headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer TOK123");
    let mut ctx = RunnerContext::new(app.port, vec![]);
    ctx.auth_headers = auth.headers;
    let authed = run_checks(&[protected_check()], &ctx);
    let authed_ok = authed[0].passed;

    app.stop();

    assert!(!unauth_ok, "protected check should fail without auth");
    assert!(has_bearer, "login should yield a Bearer token header");
    assert!(authed_ok, "protected check should pass with auth, got {:?}", authed[0]);
}

// ---------------------------------------------------------------------------
// OIDC / OAuth2 + named identities (`users:` + `as:`)
// ---------------------------------------------------------------------------

/// A fake OIDC provider: publishes a discovery document and mints a token per
/// client, so a test can tell which identity a request was made as.
fn fake_oidc() -> (u16, std::sync::Arc<tiny_http::Server>) {
    let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let port = server.server_addr().to_ip().unwrap().port();
    let s = std::sync::Arc::clone(&server);
    std::thread::spawn(move || {
        for mut req in s.incoming_requests() {
            let path = req.url().split('?').next().unwrap_or("").to_string();
            let (code, body) = match path.as_str() {
                "/.well-known/openid-configuration" => (
                    200,
                    format!(
                        r#"{{"issuer":"http://127.0.0.1:{port}","token_endpoint":"http://127.0.0.1:{port}/token"}}"#
                    ),
                ),
                "/token" => {
                    let mut form = String::new();
                    let _ = std::io::Read::read_to_string(&mut req.as_reader(), &mut form);
                    let basic = req
                        .headers()
                        .iter()
                        .find(|h| h.field.equiv("Authorization"))
                        .map(|h| h.value.as_str().to_string())
                        .unwrap_or_default();
                    // Echo enough back that the test can assert what was sent.
                    let who = if form.contains("client_id=admin-cli") {
                        "admin"
                    } else if basic.starts_with("Basic ") {
                        "basic-client"
                    } else {
                        "reader"
                    };
                    if form.contains("client_id=bad-client") {
                        (401, r#"{"error":"invalid_client","error_description":"bad secret"}"#.to_string())
                    } else {
                        (200, format!(r#"{{"access_token":"tok-{who}","token_type":"Bearer"}}"#))
                    }
                }
                _ => (404, "{}".to_string()),
            };
            let _ = req.respond(tiny_http::Response::from_string(body).with_status_code(code));
        }
    });
    (port, server)
}

/// An app that answers with whichever bearer token it was called with, so a
/// check can assert *which identity* reached it.
fn echo_auth_app() -> (u16, std::sync::Arc<tiny_http::Server>) {
    let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let port = server.server_addr().to_ip().unwrap().port();
    let s = std::sync::Arc::clone(&server);
    std::thread::spawn(move || {
        for req in s.incoming_requests() {
            let auth = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .map(|h| h.value.as_str().to_string())
                .unwrap_or_else(|| "none".to_string());
            let body = format!(r#"{{"seen":"{auth}"}}"#);
            let _ = req.respond(tiny_http::Response::from_string(body).with_status_code(200));
        }
    });
    (port, server)
}

#[test]
fn oidc_discovers_the_token_endpoint_and_sends_the_bearer() {
    let (oidc_port, oidc) = fake_oidc();
    let (app_port, app) = echo_auth_app();

    let yaml = format!(
        r#"
version: 1
services: [postgres]
auth:
  oidc:
    issuer: "http://127.0.0.1:{oidc_port}"
    client_id: orders-app
    client_secret: s3cret
    scope: "orders:read"
checks:
  - name: "the token reaches the app"
    request: {{ method: GET, path: /whoami }}
    expect: {{ status: 200, body_contains: {{ seen: "Bearer tok-reader" }} }}
"#
    );
    let spec = NoworriesSpec::parse(&yaml).unwrap();
    let auth = resolve_auth(spec.auth.as_ref(), app_port, &BTreeMap::new()).unwrap();
    let mut ctx = RunnerContext::new(app_port, vec![]);
    ctx.auth_headers = auth.headers;
    let results = run_checks(&spec.checks, &ctx);

    oidc.unblock();
    app.unblock();
    assert!(results[0].passed, "{:?}", results[0]);
}

#[test]
fn a_bad_client_secret_fails_the_run_with_the_providers_reason() {
    // The provider's own error body (invalid_client, invalid_scope, …) is the
    // only useful diagnostic here, so it must survive into the message.
    let (oidc_port, oidc) = fake_oidc();
    let yaml = format!(
        r#"
version: 1
services: [postgres]
auth:
  oidc:
    token_url: "http://127.0.0.1:{oidc_port}/token"
    client_id: bad-client
    client_secret: wrong
checks: []
"#
    );
    let spec = NoworriesSpec::parse(&yaml).unwrap();
    let err = resolve_auth(spec.auth.as_ref(), 0, &BTreeMap::new()).unwrap_err();
    oidc.unblock();
    let msg = err.to_string();
    assert!(msg.contains("HTTP 401"), "{msg}");
    assert!(msg.contains("invalid_client"), "the provider's reason is passed through: {msg}");
}

#[test]
fn checks_run_as_the_identity_they_name() {
    let (oidc_port, oidc) = fake_oidc();
    let (app_port, app) = echo_auth_app();

    let yaml = format!(
        r#"
version: 1
services: [postgres]
auth:
  bearer: {{ token: "default-token" }}
users:
  admin:
    oidc:
      token_url: "http://127.0.0.1:{oidc_port}/token"
      client_id: admin-cli
  reader:
    bearer: {{ token: "reader-token" }}
checks:
  - name: "admin sees the admin token"
    as: admin
    request: {{ method: GET, path: /whoami }}
    expect: {{ status: 200, body_contains: {{ seen: "Bearer tok-admin" }} }}
  - name: "reader sees the reader token"
    as: reader
    request: {{ method: GET, path: /whoami }}
    expect: {{ status: 200, body_contains: {{ seen: "Bearer reader-token" }} }}
  - name: "a check with no `as` keeps the default identity"
    request: {{ method: GET, path: /whoami }}
    expect: {{ status: 200, body_contains: {{ seen: "Bearer default-token" }} }}
"#
    );
    let spec = NoworriesSpec::parse(&yaml).unwrap();
    let env = BTreeMap::new();
    let default = resolve_auth(spec.auth.as_ref(), app_port, &env).unwrap();
    let mut ctx = RunnerContext::new(app_port, vec![]);
    ctx.auth_headers = default.headers;
    for (name, s) in &spec.users {
        ctx.identities
            .insert(name.clone(), resolve_auth(Some(s), app_port, &env).unwrap());
    }
    let results = run_checks(&spec.checks, &ctx);

    oidc.unblock();
    app.unblock();
    for r in &results {
        assert!(r.passed, "{}: {:?}", r.name, r);
    }
}

#[test]
fn an_undeclared_identity_is_a_spec_error() {
    // Silently falling back to the default identity would make an RBAC check
    // pass for the wrong reason.
    let err = NoworriesSpec::parse(
        r#"
version: 1
services: [postgres]
users:
  reader: { bearer: { token: "t" } }
checks:
  - name: "delete as an admin that was never declared"
    as: admni
    request: { method: DELETE, path: /orders/1 }
"#,
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("admni"), "{msg}");
    assert!(msg.contains("reader"), "names what IS declared: {msg}");
}
