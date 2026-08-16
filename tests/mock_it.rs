//! In-process mock server test — no Docker needed, always runs. Verifies the
//! tiny_http-based mock serves stubs, records requests, and that the
//! `external_calls` assertion matches recorded calls end to end.

use std::collections::BTreeMap;
use std::time::Instant;

use noworries::mock::{start_mock, RecordedRequest};
use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::{MockRespond, MockSpec, MockStub, MockWhen, NoworriesSpec};

/// Parse a mock spec from YAML (exercises the real spec path too).
fn mock_from_yaml(yaml: &str) -> MockSpec {
    let full = format!(
        "version: 1\nservices: [postgres]\nexternals:\n  - name: m\n    mock:\n{}\n",
        yaml.lines().map(|l| format!("      {l}")).collect::<Vec<_>>().join("\n")
    );
    NoworriesSpec::parse(&full).unwrap().externals[0].mock.clone().unwrap()
}

#[test]
fn stub_matches_by_body_and_applies_delay() {
    // Same path, different body → different response; plus a response delay.
    let spec = mock_from_yaml(
        r#"stubs:
  - when: { path: /charge, body_contains: { amount: 200 } }
    respond: { status: 402, body: { error: "too big" } }
  - when: { path: /charge, body_contains: { amount: 100 } }
    respond: { status: 200, body: { status: "PAID" }, delay_ms: 250 }
  - when: { path: /charge }
    respond: { status: 500 }"#,
    );
    let mut mock = start_mock("payments", &spec).unwrap();
    let agent = ureq::builder().build();

    // amount:100 → first *matching* stub is the 200 (with delay).
    let t = Instant::now();
    let r100 = agent.post(&format!("{}/charge", mock.url)).send_string("{\"amount\":100}").unwrap();
    assert_eq!(r100.status(), 200);
    assert!(r100.into_string().unwrap().contains("PAID"));
    assert!(t.elapsed().as_millis() >= 200, "delay_ms should have applied");

    // amount:200 → the 402 stub (body match wins over the catch-all).
    let r200 = agent.post(&format!("{}/charge", mock.url)).send_string("{\"amount\":200}").unwrap_or_else(|e| match e {
        ureq::Error::Status(_, r) => r,
        _ => panic!("transport"),
    });
    assert_eq!(r200.status(), 402);

    // no amount → catch-all 500.
    let r0 = agent.post(&format!("{}/charge", mock.url)).send_string("{\"x\":1}").unwrap_or_else(|e| match e {
        ureq::Error::Status(_, r) => r,
        _ => panic!("transport"),
    });
    assert_eq!(r0.status(), 500);

    mock.stop();
}

fn charge_stub() -> MockSpec {
    MockSpec {
        stubs: vec![MockStub {
            when_: MockWhen {
                method: Some("POST".into()),
                path: "/charge".into(),
                body_contains: None,
            },
            respond: MockRespond {
                status: Some(201),
                body: Some(serde_yaml::from_str("{ id: \"ch_1\", status: \"ok\" }").unwrap()),
                headers: BTreeMap::new(),
                delay_ms: None,
            },
        }],
    }
}

#[test]
fn mock_serves_stub_and_records_requests() {
    let mut mock = start_mock("payments", &charge_stub()).unwrap();
    let agent = ureq::builder().build();

    // Matched stub → 201 + JSON body.
    let resp = agent
        .post(&format!("{}/charge", mock.url))
        .send_string("{\"amount\":100}")
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body = resp.into_string().unwrap();
    assert!(body.contains("ch_1"), "stub body: {body}");

    // Unmatched path → default 200.
    let resp2 = agent.get(&format!("{}/other", mock.url)).call().unwrap();
    assert_eq!(resp2.status(), 200);

    let recorded: Vec<RecordedRequest> = mock.recorder().lock().unwrap().clone();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0].method, "POST");
    assert_eq!(recorded[0].path, "/charge");
    assert_eq!(recorded[0].json(), serde_json::json!({"amount": 100}));
    assert_eq!(recorded[1].method, "GET");
    assert_eq!(recorded[1].path, "/other");

    mock.stop();
}

#[test]
fn external_calls_assertion_matches_recorded_call() {
    let mut mock = start_mock("payments", &charge_stub()).unwrap();
    let agent = ureq::builder().build();
    let _ = agent
        .post(&format!("{}/charge", mock.url))
        .send_string("{\"amount\":100,\"currency\":\"USD\"}")
        .unwrap();

    let yaml = r#"
version: 1
services: [postgres]
externals:
  - name: payments
    mock: { stubs: [] }
checks:
  - name: "app charged the customer"
    external_calls:
      - external: payments
        method: POST
        path: /charge
        body_contains: { amount: 100 }
        times: 1
"#;
    let checks = NoworriesSpec::parse(yaml).unwrap().checks;

    let mut ctx = RunnerContext::new(0, vec![]);
    ctx.external_mocks.insert("payments".into(), mock.recorder());

    let results = run_checks(&checks, &ctx);
    assert!(
        results[0].passed,
        "external_calls should match the recorded charge, got {:?}",
        results[0]
    );

    mock.stop();
}
