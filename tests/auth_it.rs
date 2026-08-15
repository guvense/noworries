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
