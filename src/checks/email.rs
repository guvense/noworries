//! Mail assertion: read the SMTP sink's mailbox back over its JSON API.
//!
//! Password resets, order confirmations, invitations — an app that sends mail
//! had no observable side effect noworries could assert on. With an `smtp`
//! service in the spec, the app delivers into an ephemeral Mailpit container
//! and this check reads the mailbox: who it went to, the subject, the body, how
//! many were sent.
//!
//! Mail is asynchronous (queued, sent on a worker), so the check retries within
//! the check's budget rather than failing on the first empty mailbox.

use std::time::Duration;

use serde_json::Value as Json;

use super::{fail, http_agent, pass, retry_until, Assertion};
use crate::runner::{interpolate, AssertionResult, RunnerContext};
use crate::services::mailpit::MAILPIT_API_PORT;
use crate::spec::{CheckSpec, EmailAssertion, ServiceKind};

pub struct Email;

impl Assertion for Email {
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, retry: Duration) -> Vec<AssertionResult> {
        let Some(e) = &check.email else { return vec![] };
        let Some(base) = api_base(ctx) else {
            return vec![fail(
                "email",
                "email assertion but no smtp service is running (add `smtp` to services)".into(),
                None,
                None,
            )];
        };
        let budget = e
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(retry.max(Duration::from_secs(5)));
        retry_until(budget, || vec![evaluate(e, ctx, &base)])
    }
}

/// Mailpit's HTTP API, resolved from the SMTP service's aux port.
fn api_base(ctx: &RunnerContext) -> Option<String> {
    let ep = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Smtp)?;
    let api = ep.aux_ports.get(&MAILPIT_API_PORT)?;
    Some(format!("http://127.0.0.1:{api}"))
}

fn evaluate(e: &EmailAssertion, ctx: &RunnerContext, base: &str) -> AssertionResult {
    let agent = http_agent(Duration::from_secs(10));
    let listed = match agent.get(&format!("{base}/api/v1/messages?limit=200")).call() {
        Ok(r) => r.into_string().unwrap_or_default(),
        Err(err) => return fail("email", format!("could not read the mailbox: {err}"), None, None),
    };
    let doc: Json = serde_json::from_str(&listed).unwrap_or(Json::Null);
    let messages = doc.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default();

    // Header-level filters first — cheap, and they narrow what needs a body fetch.
    let mut matched: Vec<&Json> = messages
        .iter()
        .filter(|m| header_matches(e, m, ctx))
        .collect();

    // `body_contains` needs the full message; Mailpit's list only carries a snippet.
    if let Some(needle) = &e.body_contains {
        let needle = interpolate(needle, &ctx.env);
        matched.retain(|m| {
            m.get("ID")
                .and_then(|id| id.as_str())
                .map(|id| body_contains(&agent, base, id, &needle))
                .unwrap_or(false)
        });
    }

    let count = matched.len();
    let describe = describe(e, ctx);

    if e.expect_none == Some(true) {
        return if count == 0 {
            pass("email", format!("no message matching {describe} — as expected"))
        } else {
            fail(
                "email",
                format!("expected no message matching {describe}, found {count}"),
                Some(Json::from(0)),
                Some(Json::from(count)),
            )
        };
    }

    match e.expect_count {
        Some(want) if count as u64 != want => fail(
            "email",
            format!("expected {want} message(s) matching {describe}, found {count}"),
            Some(Json::from(want)),
            Some(Json::from(count)),
        ),
        _ if count == 0 => fail(
            "email",
            format!("no message matching {describe} (mailbox holds {})", messages.len()),
            Some(Json::String(describe)),
            Some(Json::from(messages.len())),
        ),
        _ => pass("email", format!("{count} message(s) matching {describe}")),
    }
}

fn header_matches(e: &EmailAssertion, m: &Json, ctx: &RunnerContext) -> bool {
    if let Some(to) = &e.to {
        let to = interpolate(to, &ctx.env).to_ascii_lowercase();
        // A recipient counts whether it sat in To, Cc or Bcc.
        let found = ["To", "Cc", "Bcc"].iter().any(|field| {
            m.get(field)
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter().any(|a| {
                        a.get("Address")
                            .and_then(|x| x.as_str())
                            .map(|x| x.eq_ignore_ascii_case(&to))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        });
        if !found {
            return false;
        }
    }
    if let Some(from) = &e.from {
        let from = interpolate(from, &ctx.env);
        let actual = m
            .get("From")
            .and_then(|f| f.get("Address"))
            .and_then(|a| a.as_str())
            .unwrap_or_default();
        if !actual.eq_ignore_ascii_case(&from) {
            return false;
        }
    }
    if let Some(sub) = &e.subject_contains {
        let sub = interpolate(sub, &ctx.env);
        let actual = m.get("Subject").and_then(|s| s.as_str()).unwrap_or_default();
        if !actual.contains(&sub) {
            return false;
        }
    }
    true
}

/// Fetch one message and look for `needle` in its text or HTML part.
fn body_contains(agent: &ureq::Agent, base: &str, id: &str, needle: &str) -> bool {
    let Ok(resp) = agent.get(&format!("{base}/api/v1/message/{id}")).call() else {
        return false;
    };
    let body = resp.into_string().unwrap_or_default();
    let msg: Json = serde_json::from_str(&body).unwrap_or(Json::Null);
    ["Text", "HTML"].iter().any(|part| {
        msg.get(part)
            .and_then(|v| v.as_str())
            .map(|s| s.contains(needle))
            .unwrap_or(false)
    })
}

/// Human-readable form of the filter, for the assertion message.
fn describe(e: &EmailAssertion, ctx: &RunnerContext) -> String {
    let mut parts = Vec::new();
    if let Some(v) = &e.to {
        parts.push(format!("to={}", interpolate(v, &ctx.env)));
    }
    if let Some(v) = &e.from {
        parts.push(format!("from={}", interpolate(v, &ctx.env)));
    }
    if let Some(v) = &e.subject_contains {
        parts.push(format!("subject~\"{}\"", interpolate(v, &ctx.env)));
    }
    if let Some(v) = &e.body_contains {
        parts.push(format!("body~\"{}\"", interpolate(v, &ctx.env)));
    }
    if parts.is_empty() {
        "any message".to_string()
    } else {
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(to: &str, from: &str, subject: &str) -> Json {
        serde_json::json!({
            "ID": "abc",
            "From": { "Address": from },
            "To": [ { "Address": to } ],
            "Cc": [],
            "Subject": subject,
        })
    }

    fn spec(yaml: &str) -> EmailAssertion {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn header_filters_are_case_insensitive_on_addresses() {
        let ctx = RunnerContext::new(0, vec![]);
        let m = msg("User@Example.com", "no-reply@shop.io", "Order confirmed: A-1");
        assert!(header_matches(&spec("to: user@example.com\n"), &m, &ctx));
        assert!(header_matches(&spec("from: NO-REPLY@shop.io\n"), &m, &ctx));
        assert!(header_matches(&spec("subject_contains: \"Order confirmed\"\n"), &m, &ctx));
        assert!(!header_matches(&spec("to: someone@else.com\n"), &m, &ctx));
        // Subject matching stays case-sensitive — it is a substring assertion.
        assert!(!header_matches(&spec("subject_contains: \"order confirmed\"\n"), &m, &ctx));
    }

    #[test]
    fn a_recipient_counts_in_cc_too() {
        let ctx = RunnerContext::new(0, vec![]);
        let mut m = msg("primary@example.com", "s@shop.io", "hi");
        m["Cc"] = serde_json::json!([{ "Address": "boss@example.com" }]);
        assert!(header_matches(&spec("to: boss@example.com\n"), &m, &ctx));
    }

    #[test]
    fn filters_combine_as_and() {
        let ctx = RunnerContext::new(0, vec![]);
        let m = msg("u@e.io", "no-reply@shop.io", "Order confirmed");
        let both = spec("to: u@e.io\nsubject_contains: Order\n");
        assert!(header_matches(&both, &m, &ctx));
        let mismatch = spec("to: u@e.io\nsubject_contains: Shipped\n");
        assert!(!header_matches(&mismatch, &m, &ctx));
    }
}
