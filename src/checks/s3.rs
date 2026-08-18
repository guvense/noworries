//! Object-storage assertion: read an object back from S3 (or MinIO).
//!
//! An upload endpoint's real side effect is an object in a bucket, which was
//! previously invisible to a check — the app returned 201 and nothing verified
//! that anything landed. With a `minio` service declared, this reads the object
//! back over the S3 REST API; point `endpoint`/credentials elsewhere and the
//! same check works against real S3 or any S3-compatible store.
//!
//! Uploads are often asynchronous (a worker, a multipart finish), so the check
//! retries within the check's budget.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value as Json;

use super::sigv4::{sign, SignRequest};
use super::{fail, http_agent, pass, retry_until, Assertion};
use crate::runner::{interpolate, AssertionResult, RunnerContext};
use crate::services::minio::{MINIO_PASSWORD, MINIO_REGION, MINIO_USER};
use crate::spec::{CheckSpec, S3Assertion, ServiceKind};

pub struct S3;

impl Assertion for S3 {
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, retry: Duration) -> Vec<AssertionResult> {
        let Some(s) = &check.s3 else { return vec![] };
        let Some(target) = Target::resolve(s, ctx) else {
            return vec![fail(
                "s3",
                "s3 assertion but no minio service is running and no `endpoint` was given \
                 (add `minio` to services, or set s3.endpoint for a remote bucket)"
                    .into(),
                None,
                None,
            )];
        };
        let budget = s
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(retry.max(Duration::from_secs(5)));
        retry_until(budget, || evaluate(s, ctx, &target))
    }
}

/// Where to send signed requests, and with which credentials.
struct Target {
    /// `http://127.0.0.1:9000`
    base: String,
    /// `127.0.0.1:9000` — the value that must be signed as `Host`.
    host: String,
    region: String,
    access_key: String,
    secret_key: String,
}

impl Target {
    fn resolve(s: &S3Assertion, ctx: &RunnerContext) -> Option<Self> {
        let base = match &s.endpoint {
            Some(e) => interpolate(e, &ctx.env),
            None => {
                let ep = ctx.endpoints.iter().find(|e| e.kind == ServiceKind::Minio)?;
                format!("http://127.0.0.1:{}", ep.host_port)
            }
        };
        let host = base
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string();
        Some(Target {
            base: base.trim_end_matches('/').to_string(),
            host,
            region: s
                .region
                .as_ref()
                .map(|r| interpolate(r, &ctx.env))
                .unwrap_or_else(|| MINIO_REGION.to_string()),
            access_key: s
                .access_key
                .as_ref()
                .map(|k| interpolate(k, &ctx.env))
                .unwrap_or_else(|| MINIO_USER.to_string()),
            secret_key: s
                .secret_key
                .as_ref()
                .map(|k| interpolate(k, &ctx.env))
                .unwrap_or_else(|| MINIO_PASSWORD.to_string()),
        })
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// What one signed GET came back with.
struct S3Response {
    status: u16,
    /// Only the headers the assertions look at.
    headers: Vec<(String, String)>,
    body: String,
}

/// Send one signed GET.
fn get(target: &Target, path: &str, query: &[(String, String)]) -> Result<S3Response, String> {
    let qs = query
        .iter()
        .map(|(k, v)| format!("{}={}", super::sigv4::uri_encode(k, false), super::sigv4::uri_encode(v, false)))
        .collect::<Vec<_>>()
        .join("&");
    let url = if qs.is_empty() {
        format!("{}{}", target.base, path)
    } else {
        format!("{}{}?{}", target.base, path, qs)
    };

    let headers = sign(&SignRequest {
        method: "GET",
        path,
        query,
        host: &target.host,
        region: &target.region,
        access_key: &target.access_key,
        secret_key: &target.secret_key,
        now: now_secs(),
    });

    let agent = http_agent(Duration::from_secs(15));
    let mut req = agent.get(&url);
    for (k, v) in &headers {
        req = req.set(k, v);
    }
    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(ureq::Error::Transport(t)) => return Err(format!("{t}")),
    };
    let status = resp.status();
    let wanted: Vec<(String, String)> = ["content-type", "content-length"]
        .iter()
        .filter_map(|h| resp.header(h).map(|v| (h.to_string(), v.to_string())))
        .collect();
    Ok(S3Response { status, headers: wanted, body: resp.into_string().unwrap_or_default() })
}

fn evaluate(s: &S3Assertion, ctx: &RunnerContext, target: &Target) -> Vec<AssertionResult> {
    let bucket = interpolate(&s.bucket, &ctx.env);

    // Prefix mode: list the bucket and count keys.
    if let Some(prefix) = &s.prefix {
        let prefix = interpolate(prefix, &ctx.env);
        let query = vec![
            ("list-type".to_string(), "2".to_string()),
            ("prefix".to_string(), prefix.clone()),
        ];
        return match get(target, &format!("/{bucket}"), &query) {
            Err(e) => vec![fail("s3", format!("list {bucket}/{prefix} failed: {e}"), None, None)],
            Ok(r) if !(200..300).contains(&r.status) => vec![fail(
                "s3",
                format!("list {bucket}/{prefix} returned HTTP {}: {}", r.status, s3_error(&r.body)),
                None,
                None,
            )],
            Ok(r) => {
                let keys = list_keys(&r.body);
                match s.expect_count {
                    Some(want) if keys.len() as u64 != want => vec![fail(
                        "s3",
                        format!("expected {want} object(s) under {bucket}/{prefix}, found {}", keys.len()),
                        Some(Json::from(want)),
                        Some(Json::from(keys.len())),
                    )],
                    None if keys.is_empty() => vec![fail(
                        "s3",
                        format!("no objects under {bucket}/{prefix}"),
                        Some(Json::String(format!(">= 1 under {prefix}"))),
                        Some(Json::from(0)),
                    )],
                    _ => vec![pass(
                        "s3",
                        format!("{} object(s) under {bucket}/{prefix}", keys.len()),
                    )],
                }
            }
        };
    }

    // Object mode.
    let Some(key) = &s.key else {
        return vec![fail(
            "s3",
            "s3 assertion needs `key` (a single object) or `prefix` (a listing)".into(),
            None,
            None,
        )];
    };
    let key = interpolate(key, &ctx.env);
    let resp = match get(target, &format!("/{bucket}/{key}"), &[]) {
        Ok(v) => v,
        Err(e) => return vec![fail("s3", format!("GET {bucket}/{key} failed: {e}"), None, None)],
    };
    let (status, headers, body) = (resp.status, resp.headers, resp.body);

    let exists = (200..300).contains(&status);
    if s.expect_exists == Some(false) {
        return if exists {
            vec![fail(
                "s3",
                format!("{bucket}/{key} exists, but the check expected it not to"),
                Some(Json::Bool(false)),
                Some(Json::Bool(true)),
            )]
        } else {
            vec![pass("s3", format!("{bucket}/{key} does not exist — as expected"))]
        };
    }
    if !exists {
        return vec![fail(
            "s3",
            format!("{bucket}/{key} not found (HTTP {status}: {})", s3_error(&body)),
            Some(Json::Bool(true)),
            Some(Json::Bool(false)),
        )];
    }

    let mut out = vec![pass("s3", format!("{bucket}/{key} exists"))];
    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };

    if let Some(want) = &s.content_type {
        let want = interpolate(want, &ctx.env);
        let actual = header("content-type");
        // Servers append charset; compare on the media type.
        let actual_media = actual.split(';').next().unwrap_or("").trim().to_string();
        out.push(if actual_media == want {
            pass("s3", format!("content-type is {want}"))
        } else {
            fail(
                "s3",
                format!("content-type is {actual_media}, expected {want}"),
                Some(Json::String(want)),
                Some(Json::String(actual)),
            )
        });
    }

    if let Some(min) = s.min_size {
        let size = header("content-length").parse::<u64>().unwrap_or(body.len() as u64);
        out.push(if size >= min {
            pass("s3", format!("object is {size} bytes (>= {min})"))
        } else {
            fail(
                "s3",
                format!("object is {size} bytes, expected at least {min}"),
                Some(Json::from(min)),
                Some(Json::from(size)),
            )
        });
    }

    if let Some(needle) = &s.body_contains {
        let needle = interpolate(needle, &ctx.env);
        out.push(if body.contains(&needle) {
            pass("s3", format!("object body contains \"{needle}\""))
        } else {
            fail(
                "s3",
                format!("object body does not contain \"{needle}\""),
                Some(Json::String(needle)),
                None,
            )
        });
    }

    out
}

/// Keys in a ListObjectsV2 response. The XML is shallow and fixed-shape, so a
/// tag scan beats pulling in an XML parser.
fn list_keys(xml: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<Key>") {
        let after = &rest[start + 5..];
        let Some(end) = after.find("</Key>") else { break };
        keys.push(after[..end].to_string());
        rest = &after[end + 6..];
    }
    keys
}

/// The `<Message>` (or `<Code>`) of an S3 error body, for a useful failure line.
fn s3_error(xml: &str) -> String {
    for tag in ["Message", "Code"] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        if let Some(s) = xml.find(&open) {
            let after = &xml[s + open.len()..];
            if let Some(e) = after.find(&close) {
                return after[..e].to_string();
            }
        }
    }
    xml.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_keys_reads_a_listobjectsv2_body() {
        let xml = r#"<?xml version="1.0"?><ListBucketResult>
            <Contents><Key>invoices/a.pdf</Key><Size>12</Size></Contents>
            <Contents><Key>invoices/b.pdf</Key><Size>13</Size></Contents>
        </ListBucketResult>"#;
        assert_eq!(list_keys(xml), vec!["invoices/a.pdf", "invoices/b.pdf"]);
        assert!(list_keys("<ListBucketResult></ListBucketResult>").is_empty());
    }

    #[test]
    fn s3_error_surfaces_the_message() {
        let xml = "<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message></Error>";
        assert_eq!(s3_error(xml), "The specified key does not exist.");
        // Falls back to the code, then to raw text.
        assert_eq!(s3_error("<Error><Code>AccessDenied</Code></Error>"), "AccessDenied");
        assert_eq!(s3_error("not xml at all"), "not xml at all");
    }

    #[test]
    fn endpoint_override_targets_a_remote_bucket() {
        let ctx = RunnerContext::new(0, vec![]);
        let s: S3Assertion = serde_yaml::from_str(
            "bucket: uploads\nkey: a.pdf\nendpoint: https://s3.eu-west-1.amazonaws.com/\nregion: eu-west-1\naccess_key: AK\nsecret_key: SK\n",
        )
        .unwrap();
        let t = Target::resolve(&s, &ctx).unwrap();
        assert_eq!(t.base, "https://s3.eu-west-1.amazonaws.com");
        assert_eq!(t.host, "s3.eu-west-1.amazonaws.com", "the signed Host carries no scheme");
        assert_eq!(t.region, "eu-west-1");
        assert_eq!(t.access_key, "AK");
    }

    #[test]
    fn without_an_endpoint_or_a_minio_service_there_is_no_target() {
        let ctx = RunnerContext::new(0, vec![]);
        let s: S3Assertion = serde_yaml::from_str("bucket: uploads\nkey: a.pdf\n").unwrap();
        assert!(Target::resolve(&s, &ctx).is_none());
    }
}
