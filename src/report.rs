//! Plain, easy-to-parse output for humans and for Claude (via /noworries).

use std::path::Path;

use serde::Serialize;

use crate::lifecycle::ServiceEndpoint;
use crate::runner::CheckResult;
use crate::services::postgres_jdbc;
use crate::spec::{ServiceDecl, ServiceKind};

pub fn info(msg: &str) {
    println!("{msg}");
}

pub fn ok(msg: &str) {
    println!("\u{2713} {msg}");
}

pub fn warn(msg: &str) {
    eprintln!("! {msg}");
}

pub fn error_line(msg: &str) {
    eprintln!("\u{2717} {msg}");
}

pub fn detected_services(services: &[ServiceDecl]) {
    println!("Detected services:");
    for s in services {
        let note = if s.tag.is_some() { "" } else { "  (default image)" };
        println!("  \u{2022} {:<9} {}{}", s.kind.as_str(), s.image, note);
    }
}

pub fn endpoints(endpoints: &[ServiceEndpoint]) {
    println!();
    println!("Resolved endpoints:");
    for ep in endpoints {
        println!(
            "  \u{2022} {:<9} localhost:{}  (container port {})",
            ep.service, ep.host_port, ep.container_port
        );
        if ep.kind == ServiceKind::Postgres {
            println!("      JDBC: {}", postgres_jdbc(ep.host_port));
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ReportSummary {
    pub ready: bool,
    pub passed: usize,
    pub failed: usize,
    pub total: usize,
    pub results: Vec<CheckResult>,
}

fn fmt_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Print a delimited results block designed to be read by both a human and
/// Claude (via /noworries). Returns the summary so the caller can set exit code.
pub fn print_results(results: Vec<CheckResult>) -> ReportSummary {
    let passed = results.iter().filter(|r| r.passed).count();
    let failed = results.len() - passed;
    let ready = failed == 0 && !results.is_empty();

    println!();
    println!("=== noworries results ===");
    for r in &results {
        println!("{}  {}", if r.passed { "PASS" } else { "FAIL" }, r.name);
        if let Some(err) = &r.error {
            println!("      ! {err}");
        }
        for a in &r.assertions {
            if a.passed {
                println!("      \u{2713} [{}] {}", a.kind, a.message);
            } else {
                println!("      \u{2717} [{}] {}", a.kind, a.message);
                if let Some(e) = &a.expected {
                    println!("          expected: {}", fmt_json(e));
                }
                if let Some(ac) = &a.actual {
                    println!("          actual:   {}", fmt_json(ac));
                }
            }
        }
    }
    println!();
    println!("Summary: {passed} passed, {failed} failed ({} total)", results.len());
    println!("Result: {}", if ready { "READY" } else { "NOT READY" });
    println!("=========================");

    ReportSummary {
        ready,
        passed,
        failed,
        total: results.len(),
        results,
    }
}

/// Persist machine-readable results so Claude can parse them deterministically.
pub fn write_results_json(path: &Path, summary: &ReportSummary) -> anyhow::Result<()> {
    std::fs::write(path, serde_json::to_string_pretty(summary)?)?;
    Ok(())
}
