//! In-process mock server test — no Docker needed, always runs. Verifies the
//! tiny_http-based mock serves stubs, records requests, and that the
//! `external_calls` assertion matches recorded calls end to end.

use std::collections::BTreeMap;

use noworries::mock::{start_mock, RecordedRequest};
use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::{MockRespond, MockSpec, MockStub, MockWhen, NoworriesSpec};

fn charge_stub() -> MockSpec {
    MockSpec {
        stubs: vec![MockStub {
            when_: MockWhen {
                method: Some("POST".into()),
                path: "/charge".into(),
            },
            respond: MockRespond {
                status: Some(201),
                body: Some(serde_yaml::from_str("{ id: \"ch_1\", status: \"ok\" }").unwrap()),
                headers: BTreeMap::new(),
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
