//! gRPC `grpc` assertion test using a **fake `grpcurl`** on PATH — exercises the
//! real code path (build args → spawn grpcurl → parse JSON → subset-match)
//! without a live gRPC server. Unix-only (uses a shell-script stub); the Windows
//! CI job runs unit tests, not integration tests.
#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;

use noworries::runner::{run_checks, RunnerContext};
use noworries::spec::NoworriesSpec;

fn write_fake_grpcurl(dir: &std::path::Path, script_body: &str) {
    let path = dir.join("grpcurl");
    let mut f = std::fs::File::create(&path).unwrap();
    write!(f, "#!/bin/sh\n{script_body}\n").unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
}

#[test]
fn grpc_runs_grpcurl_and_matches_response() {
    let dir = std::env::temp_dir().join(format!("nw-grpc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // Prepend our stub dir to PATH (single-test binary → no parallel env race).
    let old_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{old_path}", dir.display()));

    let yaml = r#"
version: 1
services: [postgres]
checks:
  - name: "GetOrder returns PENDING"
    grpc:
      target: "127.0.0.1:50051"
      method: "orders.OrderService/GetOrder"
      data: { id: "ABC" }
      expect_contains: { status: "PENDING", id: "ABC" }
"#;
    let checks = NoworriesSpec::parse(yaml).unwrap().checks;
    let ctx = RunnerContext::new(0, vec![]);

    // Success: stub returns the expected JSON.
    write_fake_grpcurl(&dir, r#"echo '{"status":"PENDING","id":"ABC"}'"#);
    let ok = run_checks(&checks, &ctx);
    assert!(ok[0].passed, "grpc should match stub response: {:?}", ok[0]);

    // Mismatch: stub returns a different status → assertion fails.
    write_fake_grpcurl(&dir, r#"echo '{"status":"SHIPPED","id":"ABC"}'"#);
    let mismatch = run_checks(&checks, &ctx);
    assert!(!mismatch[0].passed, "grpc should fail on mismatch: {:?}", mismatch[0]);

    // Non-zero exit → assertion fails with the stderr.
    write_fake_grpcurl(&dir, "echo 'boom' 1>&2; exit 1");
    let err = run_checks(&checks, &ctx);
    assert!(!err[0].passed, "grpc should fail on non-zero exit: {:?}", err[0]);

    std::env::set_var("PATH", old_path);
    let _ = std::fs::remove_dir_all(&dir);
}
