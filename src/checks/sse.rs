//! Server-Sent-Events assertion: open the stream and read events until one
//! matches (deep-subset on the event's JSON `data`) or the timeout elapses.

use std::io::{BufRead, BufReader};
use std::time::{Duration, Instant};

use serde_json::Value as Json;

use super::{fail, pass, resolve_target, yaml_json, Assertion};
use crate::runner::{matches_subset, AssertionResult, RunnerContext};
use crate::spec::CheckSpec;

pub struct Sse;

impl Assertion for Sse {
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, _retry: Duration) -> Vec<AssertionResult> {
        let Some(s) = &check.sse else { return vec![] };
        let url = resolve_target(&s.path, ctx, &["http://", "https://"]);
        let timeout = Duration::from_millis(s.timeout_ms.unwrap_or(5000));
        let expected = yaml_json(&s.contains);

        let agent = super::http_agent(timeout + Duration::from_secs(2));
        let mut req = agent.get(&url).set("accept", "text/event-stream");
        for (k, v) in ctx.auth_for(check).0 {
            req = req.set(k, v);
        }
        let resp = match req.call() {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(ureq::Error::Transport(t)) => {
                return vec![fail("sse", format!("stream {url} failed: {t}"), None, None)];
            }
        };

        match scan(resp, &expected, timeout) {
            Some(_) => vec![pass("sse", format!("matching event on {}", s.path))],
            None => vec![fail(
                "sse",
                format!("no matching event on {} within {}ms", s.path, timeout.as_millis()),
                Some(expected),
                None,
            )],
        }
    }
}

/// Read `data:` lines (one event may span multiple `data:` lines) and return the
/// first whose JSON matches `expected`.
fn scan(resp: ureq::Response, expected: &Json, timeout: Duration) -> Option<Json> {
    let deadline = Instant::now() + timeout;
    let reader = BufReader::new(resp.into_reader());
    let mut data = String::new();
    for line in reader.lines() {
        if Instant::now() > deadline {
            break;
        }
        let Ok(line) = line else { break };
        let line = line.trim_end();
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        } else if line.is_empty() {
            // blank line = event boundary → evaluate accumulated data
            if !data.is_empty() {
                let parsed: Json = serde_json::from_str(&data)
                    .unwrap_or_else(|_| Json::String(data.clone()));
                if matches_subset(expected, &parsed) {
                    return Some(parsed);
                }
                data.clear();
            }
        }
    }
    // Trailing event without a blank line.
    if !data.is_empty() {
        let parsed: Json = serde_json::from_str(&data).unwrap_or_else(|_| Json::String(data.clone()));
        if matches_subset(expected, &parsed) {
            return Some(parsed);
        }
    }
    None
}
