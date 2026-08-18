//! `email:` and `s3:` against REAL containers — the half the in-process tests
//! can't cover: that Mailpit accepts SMTP and exposes what it received, and
//! that MinIO accepts our SigV4 signature.
//!
//! Runs only when the ports are pointed at running containers (as
//! ci/docker-compose.yml sets up):
//!   NOWORRIES_IT_SMTP_PORT   — Mailpit SMTP (1025)
//!   NOWORRIES_IT_SMTP_API    — Mailpit HTTP API (8025)
//!   NOWORRIES_IT_MINIO_PORT  — MinIO S3 API (9000)

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use noworries::lifecycle::ServiceEndpoint;
use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::{NoworriesSpec, ServiceKind};

fn port(var: &str) -> Option<u16> {
    std::env::var(var).ok().and_then(|v| v.parse().ok())
}

/// Speak just enough SMTP to deliver one message.
fn send_mail(smtp_port: u16, from: &str, to: &str, subject: &str, body: &str) {
    let stream = TcpStream::connect(("127.0.0.1", smtp_port)).expect("connect to the SMTP sink");
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut line = String::new();
    let expect = |reader: &mut BufReader<TcpStream>, line: &mut String, code: &str| {
        line.clear();
        reader.read_line(line).expect("SMTP reply");
        assert!(line.starts_with(code), "expected {code}, got {line}");
    };

    expect(&mut reader, &mut line, "220");
    for (cmd, code) in [
        ("EHLO noworries-test\r\n", "250"),
        (&format!("MAIL FROM:<{from}>\r\n"), "250"),
        (&format!("RCPT TO:<{to}>\r\n"), "250"),
        ("DATA\r\n", "354"),
    ] {
        writer.write_all(cmd.as_bytes()).unwrap();
        // EHLO answers with several 250- lines; read until the final one.
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line.starts_with(&format!("{code}-")) {
                continue;
            }
            assert!(line.starts_with(code), "expected {code} for {cmd:?}, got {line}");
            break;
        }
    }
    let message = format!("From: {from}\r\nTo: {to}\r\nSubject: {subject}\r\n\r\n{body}\r\n.\r\n");
    writer.write_all(message.as_bytes()).unwrap();
    expect(&mut reader, &mut line, "250");
    writer.write_all(b"QUIT\r\n").unwrap();
}

#[test]
fn email_check_sees_a_message_delivered_over_smtp() {
    let (Some(smtp), Some(api)) = (port("NOWORRIES_IT_SMTP_PORT"), port("NOWORRIES_IT_SMTP_API"))
    else {
        eprintln!("skipping: NOWORRIES_IT_SMTP_PORT / _API not set");
        return;
    };

    let subject = format!("Order confirmed: IT-{}", std::process::id());
    send_mail(
        smtp,
        "no-reply@shop.io",
        "buyer@example.com",
        &subject,
        "Your order SKU: IT-IMAP is confirmed.",
    );

    let yaml = format!(
        r#"
version: 1
services: [smtp]
checks:
  - name: "the delivered mail is visible to the check"
    email:
      to: buyer@example.com
      from: no-reply@shop.io
      subject_contains: "{subject}"
      body_contains: "SKU: IT-IMAP"
      expect_count: 1
      timeout_ms: 10000
"#
    );
    let checks = NoworriesSpec::parse(&yaml).unwrap().checks;
    let mut aux = BTreeMap::new();
    aux.insert(8025u16, api);
    let ctx = RunnerContext::new(
        0,
        vec![ServiceEndpoint {
            service: "smtp".into(),
            kind: ServiceKind::Smtp,
            host_port: smtp,
            container_port: 1025,
            aux_ports: aux,
        }],
    );
    let results = run_checks(&checks, &ctx);
    assert!(results[0].passed, "{:?}", results[0]);
}

/// Put an object with a signed request, then assert the check reads it back.
/// This is the test that proves the signature MinIO accepts is the one we send.
#[test]
fn s3_check_reads_back_an_object_minio_accepted() {
    let Some(s3_port) = port("NOWORRIES_IT_MINIO_PORT") else {
        eprintln!("skipping: NOWORRIES_IT_MINIO_PORT not set");
        return;
    };
    let base = format!("http://127.0.0.1:{s3_port}");
    let agent = ureq::builder().timeout(Duration::from_secs(20)).build();

    // MinIO starts empty: create the bucket, then the object. Both are signed
    // through the same code path the check uses.
    for (method, path, body) in [
        ("PUT", "/it-uploads", ""),
        ("PUT", "/it-uploads/invoices/it-001.txt", "INVOICE it-001"),
    ] {
        let headers = noworries::checks::sigv4::sign_with_payload(
            method,
            path,
            &[],
            &format!("127.0.0.1:{s3_port}"),
            "us-east-1",
            "noworries",
            "noworries",
            body.as_bytes(),
        );
        let mut req = agent.request(method, &format!("{base}{path}"));
        for (k, v) in &headers {
            req = req.set(k, v);
        }
        let status = match req.send_bytes(body.as_bytes()) {
            Ok(r) => r.status(),
            Err(ureq::Error::Status(c, r)) => {
                let detail = r.into_string().unwrap_or_default();
                // 409 = bucket already exists from an earlier run; fine.
                assert!(c == 409, "{method} {path} failed: {c} {detail}");
                c
            }
            Err(e) => panic!("{method} {path} transport error: {e}"),
        };
        assert!(status < 300 || status == 409, "{method} {path} → {status}");
    }

    let yaml = format!(
        r#"
version: 1
services: [minio]
checks:
  - name: "the object is readable through the s3 check"
    s3:
      endpoint: "{base}"
      bucket: it-uploads
      key: "invoices/it-001.txt"
      body_contains: "INVOICE it-001"
      min_size: 5
      timeout_ms: 10000
  - name: "and it is counted under its prefix"
    s3:
      endpoint: "{base}"
      bucket: it-uploads
      prefix: "invoices/"
      timeout_ms: 10000
"#
    );
    let checks = NoworriesSpec::parse(&yaml).unwrap().checks;
    let ctx = RunnerContext::new(
        0,
        vec![ServiceEndpoint {
            service: "minio".into(),
            kind: ServiceKind::Minio,
            host_port: s3_port,
            container_port: 9000,
            aux_ports: BTreeMap::new(),
        }],
    );
    let results = run_checks(&checks, &ctx);
    for r in &results {
        assert!(r.passed, "{}: {:?}", r.name, r);
    }
}
