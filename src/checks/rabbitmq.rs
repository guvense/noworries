//! RabbitMQ assertion: inspect a queue via the management HTTP API (port
//! 15672) with the existing `ureq` client — no AMQP driver. The provider
//! exposes the management port as a secondary port; lifecycle resolves its host
//! mapping onto `ServiceEndpoint::aux_ports`, which this check reads.
//!
//! Basic auth and path percent-encoding are hand-rolled (a few lines each) to
//! avoid pulling `base64`/`percent-encoding` crates for this one use.

use std::time::Duration;

use serde_json::Value as Json;

use super::{fail, pass, retry_until, Assertion};
use crate::runner::{AssertionResult, RunnerContext};
use crate::services::rabbitmq::{RABBITMQ_MGMT_PORT, RABBITMQ_PASSWORD, RABBITMQ_USER};
use crate::spec::{CheckSpec, RabbitmqAssertion, ServiceKind};

pub struct Rabbitmq;

impl Assertion for Rabbitmq {
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, retry: Duration) -> Vec<AssertionResult> {
        let Some(r) = &check.rabbitmq else { return vec![] };
        let Some(ep) = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Rabbitmq) else {
            return vec![fail(
                "rabbitmq",
                "rabbitmq assertion but no RabbitMQ service is running".into(),
                None,
                None,
            )];
        };
        let Some(&mgmt) = ep.aux_ports.get(&RABBITMQ_MGMT_PORT) else {
            return vec![fail(
                "rabbitmq",
                "management API port not resolved — use a `rabbitmq:*-management` image".into(),
                None,
                None,
            )];
        };
        let vhost = r.vhost.clone().unwrap_or_else(|| "/".to_string());
        retry_until(retry, || match fetch_queue(mgmt, &vhost, &r.queue) {
            Ok((status, body)) => check_queue(status, &body, r),
            Err(e) => vec![fail("rabbitmq", e, None, None)],
        })
    }
}

/// GET the queue info from the management API. Returns the HTTP status (404 for
/// a missing queue is surfaced as a status, not an error) and the parsed body.
fn fetch_queue(mgmt_port: u16, vhost: &str, queue: &str) -> Result<(u16, Json), String> {
    let url = format!(
        "http://127.0.0.1:{mgmt_port}/api/queues/{}/{}",
        pct(vhost),
        pct(queue)
    );
    let agent = super::http_agent(Duration::from_secs(15));
    let auth = format!(
        "Basic {}",
        base64(format!("{RABBITMQ_USER}:{RABBITMQ_PASSWORD}").as_bytes())
    );
    let (status, text) = match agent.get(&url).set("Authorization", &auth).call() {
        Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
        // ureq surfaces any non-2xx (incl. 404 for a missing queue) as Status.
        Err(ureq::Error::Status(code, r)) => (code, r.into_string().unwrap_or_default()),
        Err(ureq::Error::Transport(t)) => {
            return Err(format!("could not reach RabbitMQ management API: {t}"));
        }
    };
    let body = serde_json::from_str::<Json>(&text).unwrap_or(Json::Null);
    Ok((status, body))
}

/// Evaluate the assertion against a queue-info HTTP status + body. Pure (no IO)
/// so it can be unit-tested with canned responses.
fn check_queue(status: u16, body: &Json, r: &RabbitmqAssertion) -> Vec<AssertionResult> {
    let want_exists = r.expect_exists.unwrap_or(true);
    let mut out = Vec::new();

    if status == 404 {
        if want_exists {
            out.push(fail(
                "rabbitmq",
                format!("queue `{}` does not exist", r.queue),
                None,
                None,
            ));
        } else {
            out.push(pass("rabbitmq", format!("queue `{}` absent as expected", r.queue)));
        }
        return out;
    }
    if status != 200 {
        out.push(fail(
            "rabbitmq",
            format!("management API returned HTTP {status} for queue `{}`", r.queue),
            None,
            None,
        ));
        return out;
    }

    // 200: the queue exists.
    if want_exists {
        out.push(pass("rabbitmq", format!("queue `{}` exists", r.queue)));
    } else {
        out.push(fail(
            "rabbitmq",
            format!("queue `{}` exists but was expected absent", r.queue),
            None,
            None,
        ));
    }

    let messages = body.get("messages").and_then(|m| m.as_u64());
    if let Some(min) = r.min_messages {
        out.push(match messages {
            Some(n) if n >= min => pass("rabbitmq", format!("{n} messages (>= {min})")),
            Some(n) => fail(
                "rabbitmq",
                format!("{n} messages, expected >= {min}"),
                Some(Json::from(min)),
                Some(Json::from(n)),
            ),
            None => fail("rabbitmq", "queue info had no `messages` field".into(), None, None),
        });
    }
    if let Some(exact) = r.expect_messages {
        out.push(match messages {
            Some(n) if n == exact => pass("rabbitmq", format!("{n} messages (== {exact})")),
            Some(n) => fail(
                "rabbitmq",
                format!("{n} messages, expected {exact}"),
                Some(Json::from(exact)),
                Some(Json::from(n)),
            ),
            None => fail("rabbitmq", "queue info had no `messages` field".into(), None, None),
        });
    }
    out
}

/// Percent-encode a path segment (RFC 3986 unreserved set stays literal). Used
/// for the vhost — `/` must become `%2F` in the management API path.
fn pct(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

/// Standard base64 (with padding) — enough for the `user:pass` Basic auth token.
fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assertion(y: &str) -> RabbitmqAssertion {
        serde_yaml::from_str(y).unwrap()
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(base64(b"noworries:noworries"), "bm93b3JyaWVzOm5vd29ycmllcw==");
    }

    #[test]
    fn pct_encodes_default_vhost() {
        assert_eq!(pct("/"), "%2F");
        assert_eq!(pct("orders"), "orders");
    }

    #[test]
    fn missing_queue_fails_when_existence_expected() {
        let a = assertion("queue: orders");
        assert!(check_queue(404, &Json::Null, &a).iter().any(|x| !x.passed));
        let absent_ok = assertion("queue: orders\nexpect_exists: false");
        assert!(check_queue(404, &Json::Null, &absent_ok).iter().all(|x| x.passed));
    }

    #[test]
    fn message_counts_are_checked() {
        let body = serde_json::json!({"messages": 3, "messages_ready": 3});
        let exact = assertion("queue: orders\nexpect_messages: 3");
        assert!(check_queue(200, &body, &exact).iter().all(|x| x.passed));
        let min_ok = assertion("queue: orders\nmin_messages: 1");
        assert!(check_queue(200, &body, &min_ok).iter().all(|x| x.passed));
        let min_bad = assertion("queue: orders\nmin_messages: 5");
        assert!(check_queue(200, &body, &min_bad).iter().any(|x| !x.passed));
    }

    #[test]
    fn present_but_expected_absent_fails() {
        let body = serde_json::json!({"messages": 0});
        let a = assertion("queue: orders\nexpect_exists: false");
        assert!(check_queue(200, &body, &a).iter().any(|x| !x.passed));
    }
}
