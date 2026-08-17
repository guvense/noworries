//! GraphQL query/mutation assertion (HTTP POST `{query, variables}`).

use std::time::Duration;

use serde_json::{json, Value as Json};

use super::{fail, pass, resolve_target, retry_until, yaml_json, Assertion, Phase};
use crate::runner::{interpolate_json, matches_subset, AssertionResult, RunnerContext};
use crate::spec::CheckSpec;

pub struct GraphQl;

impl Assertion for GraphQl {
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, retry: Duration) -> Vec<AssertionResult> {
        let Some(g) = &check.graphql else { return vec![] };
        let url = resolve_target(&g.path, ctx, &["http://", "https://"]);
        let mut payload = json!({ "query": g.query });
        if let Some(v) = &g.variables {
            payload["variables"] = interpolate_json(&yaml_json(v), &ctx.env);
        }
        let body = serde_json::to_string(&payload).unwrap_or_default();

        retry_until(retry, || evaluate(g, ctx, &url, &body))
    }

    /// A GraphQL call — a mutation especially — acts on the app, so it runs in
    /// the trigger slot. Left among the observers it would fire *after* `db:` /
    /// `kafka.expect_message` had already given up on its effect.
    fn phase(&self, check: &CheckSpec) -> Phase {
        if check.graphql.is_some() && super::observes_elsewhere(check) {
            Phase::Trigger
        } else {
            Phase::Observe
        }
    }
}

fn evaluate(
    g: &crate::spec::GraphqlAssertion,
    ctx: &RunnerContext,
    url: &str,
    body: &str,
) -> Vec<AssertionResult> {
    let agent = super::http_agent(Duration::from_secs(15));
    let mut req = agent.post(url).set("content-type", "application/json");
    for (k, v) in &ctx.auth_headers {
        req = req.set(k, v);
    }
    let resp = match req.send_string(body) {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(ureq::Error::Transport(t)) => {
            return vec![fail("graphql", format!("request to {url} failed: {t}"), None, None)];
        }
    };
    let text = resp.into_string().unwrap_or_default();
    let parsed: Json = serde_json::from_str(&text).unwrap_or(Json::Null);

    let mut out = Vec::new();
    if g.expect_no_errors {
        let has_errors = parsed
            .get("errors")
            .and_then(|e| e.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        out.push(if has_errors {
            fail(
                "graphql",
                "response has GraphQL errors".into(),
                None,
                parsed.get("errors").cloned(),
            )
        } else {
            pass("graphql", "no GraphQL errors".into())
        });
    }
    if let Some(exp) = &g.expect_data {
        let expected = yaml_json(exp);
        let data = parsed.get("data").cloned().unwrap_or(Json::Null);
        let passed = matches_subset(&expected, &data);
        out.push(if passed {
            pass("graphql", "data contains expected fields".into())
        } else {
            fail("graphql", "data did not match".into(), Some(expected), Some(data))
        });
    }
    out
}
