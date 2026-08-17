//! gRPC assertion via `grpcurl` (shelled out). Keeps noworries free of a heavy
//! async gRPC/proto stack: it builds a `grpcurl` invocation, runs it, and
//! deep-subset-matches the JSON response. Requires `grpcurl` on PATH and either
//! server reflection or explicit `protos`/`import_paths`.

use std::process::Command;
use std::time::Duration;

use serde_json::Value as Json;

use super::{fail, pass, yaml_json, Assertion, Phase};
use crate::runner::{interpolate, interpolate_json, matches_subset, AssertionResult, RunnerContext};
use crate::spec::{CheckSpec, GrpcAssertion};

pub struct Grpc;

impl Assertion for Grpc {
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, _retry: Duration) -> Vec<AssertionResult> {
        let Some(g) = &check.grpc else { return vec![] };
        let target = interpolate(&g.target, &ctx.env);
        let data = g
            .data
            .as_ref()
            .map(|d| serde_json::to_string(&interpolate_json(&yaml_json(d), &ctx.env)).unwrap_or_default());

        let mut args = grpcurl_args(g, &target, data.as_deref());
        // Forward auth headers (from `auth`) as gRPC metadata.
        for (k, v) in ctx.auth_for(check).0 {
            args.insert(0, format!("{k}: {v}"));
            args.insert(0, "-H".to_string());
        }
        let output = match Command::new("grpcurl").args(&args).output() {
            Ok(o) => o,
            Err(e) => {
                return vec![fail(
                    "grpc",
                    format!("could not run grpcurl (install it / add to PATH): {e}"),
                    None,
                    None,
                )]
            }
        };

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return vec![fail(
                "grpc",
                format!("{} failed: {}", g.method, err.trim()),
                None,
                None,
            )];
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: Json = serde_json::from_str(stdout.trim()).unwrap_or(Json::Null);

        match &g.expect_contains {
            None => vec![pass("grpc", format!("{} returned OK", g.method))],
            Some(exp) => {
                let expected = yaml_json(exp);
                if matches_subset(&expected, &parsed) {
                    vec![pass("grpc", format!("{} response matched", g.method))]
                } else {
                    vec![fail(
                        "grpc",
                        format!("{} response did not match", g.method),
                        Some(expected),
                        Some(parsed),
                    )]
                }
            }
        }
    }

    /// An RPC acts on the app, so it runs in the trigger slot — a check that
    /// calls a gRPC method and then asserts `kafka.expect_message` / `db:` on
    /// its effect works as one check.
    fn phase(&self, check: &CheckSpec) -> Phase {
        if check.grpc.is_some() && super::observes_elsewhere(check) {
            Phase::Trigger
        } else {
            Phase::Observe
        }
    }
}

/// Build the `grpcurl` argument vector (pure — unit tested).
fn grpcurl_args(g: &GrpcAssertion, target: &str, data: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if g.plaintext {
        args.push("-plaintext".into());
    }
    for p in &g.import_paths {
        args.push("-import-path".into());
        args.push(p.clone());
    }
    for p in &g.protos {
        args.push("-proto".into());
        args.push(p.clone());
    }
    if let Some(d) = data {
        args.push("-d".into());
        args.push(d.to_string());
    }
    args.push(target.to_string());
    args.push(g.method.clone());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> GrpcAssertion {
        GrpcAssertion {
            target: "127.0.0.1:50051".into(),
            method: "orders.OrderService/Get".into(),
            data: None,
            expect_contains: None,
            plaintext: true,
            import_paths: vec![],
            protos: vec![],
        }
    }

    #[test]
    fn args_include_plaintext_data_and_method() {
        let args = grpcurl_args(&g(), "127.0.0.1:50051", Some("{\"id\":\"1\"}"));
        assert_eq!(args[0], "-plaintext");
        assert!(args.iter().any(|a| a == "-d"));
        assert!(args.contains(&"{\"id\":\"1\"}".to_string()));
        assert_eq!(args[args.len() - 2], "127.0.0.1:50051");
        assert_eq!(args[args.len() - 1], "orders.OrderService/Get");
    }

    #[test]
    fn protos_and_import_paths_expand() {
        let mut a = g();
        a.plaintext = false;
        a.import_paths = vec!["proto".into()];
        a.protos = vec!["orders.proto".into()];
        let args = grpcurl_args(&a, "t", None);
        assert!(args.contains(&"-import-path".to_string()) && args.contains(&"proto".to_string()));
        assert!(args.contains(&"-proto".to_string()) && args.contains(&"orders.proto".to_string()));
        assert!(!args.iter().any(|x| x == "-plaintext"));
    }
}
