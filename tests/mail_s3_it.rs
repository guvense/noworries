//! In-process verification of the `email:` and `s3:` checks — no Docker needed.
//!
//! A fake Mailpit API serves a two-message mailbox; a fake S3 endpoint serves
//! one object plus a ListObjectsV2 listing and records whether the request
//! arrived signed. What the containers add on top is the real SMTP/S3 server,
//! which these tests deliberately don't stand in for: the point here is the
//! check's matching, counting and failure messages.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::thread;

use noworries::lifecycle::ServiceEndpoint;
use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::{NoworriesSpec, ServiceKind};

/// Mailpit's API: a list endpoint and a per-message endpoint with bodies.
fn fake_mailpit() -> (u16, Arc<tiny_http::Server>) {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let port = server.server_addr().to_ip().unwrap().port();
    let s = Arc::clone(&server);
    thread::spawn(move || {
        for req in s.incoming_requests() {
            let path = req.url().split('?').next().unwrap_or("").to_string();
            let body = match path.as_str() {
                "/api/v1/messages" => r#"{"total":2,"count":2,"messages":[
                    {"ID":"m1","From":{"Address":"no-reply@shop.io"},
                     "To":[{"Address":"user@example.com"}],"Cc":[],
                     "Subject":"Order confirmed: NET-1"},
                    {"ID":"m2","From":{"Address":"no-reply@shop.io"},
                     "To":[{"Address":"someone@else.com"}],"Cc":[],
                     "Subject":"Password reset"}
                ]}"#
                .to_string(),
                "/api/v1/message/m1" => {
                    r#"{"ID":"m1","Text":"Thanks! Your order SKU: NET-1 is confirmed.","HTML":""}"#
                        .to_string()
                }
                "/api/v1/message/m2" => {
                    r#"{"ID":"m2","Text":"Click here to reset.","HTML":""}"#.to_string()
                }
                _ => "{}".to_string(),
            };
            let resp = tiny_http::Response::from_string(body)
                .with_header("content-type: application/json".parse::<tiny_http::Header>().unwrap());
            let _ = req.respond(resp);
        }
    });
    (port, server)
}

fn smtp_endpoint(api_port: u16) -> ServiceEndpoint {
    let mut aux = BTreeMap::new();
    aux.insert(8025u16, api_port);
    ServiceEndpoint {
        service: "smtp".into(),
        kind: ServiceKind::Smtp,
        host_port: 1025,
        container_port: 1025,
        aux_ports: aux,
    }
}

fn run(yaml: &str, endpoints: Vec<ServiceEndpoint>) -> Vec<noworries::runner::CheckResult> {
    let checks = NoworriesSpec::parse(yaml).unwrap().checks;
    let ctx = RunnerContext::new(0, endpoints);
    run_checks(&checks, &ctx)
}

#[test]
fn email_matches_on_recipient_subject_and_body() {
    let (port, server) = fake_mailpit();
    let results = run(
        r#"
version: 1
services: [smtp]
checks:
  - name: "the confirmation reached the customer"
    email:
      to: user@example.com
      from: no-reply@shop.io
      subject_contains: "Order confirmed"
      body_contains: "SKU: NET-1"
      expect_count: 1
      timeout_ms: 1000
  - name: "nothing went to an unrelated address"
    email:
      to: nobody@example.com
      expect_none: true
      timeout_ms: 1000
"#,
        vec![smtp_endpoint(port)],
    );
    server.unblock();
    for r in &results {
        assert!(r.passed, "{}: {:?}", r.name, r);
    }
}

#[test]
fn email_failure_says_what_it_looked_for() {
    let (port, server) = fake_mailpit();
    let results = run(
        r#"
version: 1
services: [smtp]
checks:
  - name: "a shipping notice that was never sent"
    email:
      to: user@example.com
      subject_contains: "Shipped"
      timeout_ms: 500
"#,
        vec![smtp_endpoint(port)],
    );
    server.unblock();
    assert!(!results[0].passed);
    let msg = &results[0].assertions[0].message;
    assert!(msg.contains("to=user@example.com"), "{msg}");
    assert!(msg.contains("Shipped"), "{msg}");
    // The mailbox size is part of the message: "empty" and "2 that didn't
    // match" are very different debugging situations.
    assert!(msg.contains("mailbox holds 2"), "{msg}");
}

#[test]
fn email_without_the_service_is_an_explicit_failure() {
    let results = run(
        r#"
version: 1
services: [postgres]
checks:
  - name: "mail check with no sink"
    email: { to: u@e.io, timeout_ms: 200 }
"#,
        vec![],
    );
    assert!(!results[0].passed);
    assert!(results[0].assertions[0].message.contains("no smtp service"));
}

/// A fake S3 endpoint. Records the Authorization header of every request so the
/// test can assert requests are actually signed.
fn fake_s3(seen_auth: Arc<Mutex<Vec<String>>>) -> (u16, Arc<tiny_http::Server>) {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let port = server.server_addr().to_ip().unwrap().port();
    let s = Arc::clone(&server);
    thread::spawn(move || {
        for req in s.incoming_requests() {
            let auth = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .map(|h| h.value.as_str().to_string())
                .unwrap_or_default();
            seen_auth.lock().unwrap().push(auth);

            let url = req.url().to_string();
            let path = url.split('?').next().unwrap_or("").to_string();
            let listing = url.contains("list-type=2");

            let (code, body, ctype) = if listing {
                (
                    200,
                    r#"<?xml version="1.0"?><ListBucketResult>
                        <Contents><Key>invoices/inv-001.pdf</Key></Contents>
                        <Contents><Key>invoices/inv-002.pdf</Key></Contents>
                    </ListBucketResult>"#
                        .to_string(),
                    "application/xml",
                )
            } else if path == "/uploads/invoices/inv-001.pdf" {
                (200, "%PDF-1.4 INVOICE 001".to_string(), "application/pdf")
            } else {
                (
                    404,
                    "<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message></Error>"
                        .to_string(),
                    "application/xml",
                )
            };
            let resp = tiny_http::Response::from_string(body)
                .with_status_code(code)
                .with_header(format!("content-type: {ctype}").parse::<tiny_http::Header>().unwrap());
            let _ = req.respond(resp);
        }
    });
    (port, server)
}

#[test]
fn s3_reads_an_object_back_and_signs_the_request() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (port, server) = fake_s3(Arc::clone(&seen));
    let yaml = format!(
        r#"
version: 1
services: [postgres]
checks:
  - name: "the invoice was uploaded"
    s3:
      endpoint: "http://127.0.0.1:{port}"
      bucket: uploads
      key: "invoices/inv-001.pdf"
      content_type: application/pdf
      min_size: 10
      body_contains: "INVOICE"
      timeout_ms: 1000
"#
    );
    let results = run(&yaml, vec![]);
    server.unblock();
    assert!(results[0].passed, "{:?}", results[0]);

    let auths = seen.lock().unwrap();
    assert!(
        auths.iter().all(|a| a.starts_with("AWS4-HMAC-SHA256 Credential=")),
        "every request must be SigV4-signed: {auths:?}"
    );
}

#[test]
fn s3_counts_objects_under_a_prefix() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (port, server) = fake_s3(Arc::clone(&seen));
    let yaml = format!(
        r#"
version: 1
services: [postgres]
checks:
  - name: "both invoices are there"
    s3:
      endpoint: "http://127.0.0.1:{port}"
      bucket: uploads
      prefix: "invoices/"
      expect_count: 2
      timeout_ms: 1000
"#
    );
    let results = run(&yaml, vec![]);
    server.unblock();
    assert!(results[0].passed, "{:?}", results[0]);
}

#[test]
fn s3_missing_object_reports_the_servers_reason() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (port, server) = fake_s3(Arc::clone(&seen));
    let yaml = format!(
        r#"
version: 1
services: [postgres]
checks:
  - name: "an object that was never uploaded"
    s3:
      endpoint: "http://127.0.0.1:{port}"
      bucket: uploads
      key: "invoices/missing.pdf"
      timeout_ms: 500
"#
    );
    let results = run(&yaml, vec![]);
    server.unblock();
    assert!(!results[0].passed);
    let msg = &results[0].assertions[0].message;
    assert!(msg.contains("not found"), "{msg}");
    assert!(msg.contains("The specified key does not exist."), "the S3 reason: {msg}");
}

#[test]
fn s3_can_assert_an_object_is_absent() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (port, server) = fake_s3(Arc::clone(&seen));
    let yaml = format!(
        r#"
version: 1
services: [postgres]
checks:
  - name: "a rejected upload leaves nothing behind"
    s3:
      endpoint: "http://127.0.0.1:{port}"
      bucket: uploads
      key: "invoices/rejected.pdf"
      expect_exists: false
      timeout_ms: 500
"#
    );
    let results = run(&yaml, vec![]);
    server.unblock();
    assert!(results[0].passed, "{:?}", results[0]);
}
