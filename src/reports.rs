//! Alternate report formats for CI: JUnit XML (so CI shows each check as a test
//! case) and a self-contained HTML page (shareable summary). Both are pure
//! functions of the [`ReportSummary`]; the `--junit`/`--html` flags write them.

use crate::report::ReportSummary;
use crate::runner::CheckResult;

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Render the summary as JUnit XML (one `<testcase>` per check).
pub fn junit_xml(summary: &ReportSummary) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str(&format!(
        "<testsuites name=\"noworries\" tests=\"{}\" failures=\"{}\">\n",
        summary.total, summary.failed
    ));
    s.push_str(&format!(
        "  <testsuite name=\"noworries\" tests=\"{}\" failures=\"{}\">\n",
        summary.total, summary.failed
    ));
    for r in &summary.results {
        s.push_str(&format!(
            "    <testcase name=\"{}\" classname=\"noworries\">\n",
            xml_escape(&r.name)
        ));
        if !r.passed {
            let detail = failure_detail(r);
            s.push_str(&format!(
                "      <failure message=\"{}\">{}</failure>\n",
                xml_escape(&first_line(&detail)),
                xml_escape(&detail)
            ));
        }
        s.push_str("    </testcase>\n");
    }
    s.push_str("  </testsuite>\n");
    s.push_str("</testsuites>\n");
    s
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").to_string()
}

fn failure_detail(r: &CheckResult) -> String {
    let mut lines = Vec::new();
    if let Some(e) = &r.error {
        lines.push(format!("error: {e}"));
    }
    for a in &r.assertions {
        if !a.passed {
            let mut l = format!("[{}] {}", a.kind, a.message);
            if let Some(exp) = &a.expected {
                l.push_str(&format!(" | expected: {}", fmt_val(exp)));
            }
            if let Some(act) = &a.actual {
                l.push_str(&format!(" | actual: {}", fmt_val(act)));
            }
            lines.push(l);
        }
    }
    if lines.is_empty() {
        "failed".to_string()
    } else {
        lines.join("\n")
    }
}

fn fmt_val(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Render the summary as a self-contained HTML report.
pub fn html_report(summary: &ReportSummary) -> String {
    let status = if summary.ready { "READY" } else { "NOT READY" };
    let status_class = if summary.ready { "ok" } else { "fail" };

    let mut rows = String::new();
    for r in &summary.results {
        let badge = if r.passed { "PASS" } else { "FAIL" };
        let cls = if r.passed { "pass" } else { "failrow" };
        rows.push_str(&format!(
            "<div class=\"check {cls}\"><div class=\"chead\"><span class=\"badge {cls}\">{badge}</span> {}</div>",
            html_escape(&r.name)
        ));
        if let Some(e) = &r.error {
            rows.push_str(&format!("<div class=\"err\">error: {}</div>", html_escape(e)));
        }
        for a in &r.assertions {
            let mark = if a.passed { "✓" } else { "✗" };
            let acls = if a.passed { "apass" } else { "afail" };
            rows.push_str(&format!(
                "<div class=\"assert {acls}\">{mark} <span class=\"kind\">[{}]</span> {}",
                html_escape(&a.kind),
                html_escape(&a.message)
            ));
            if !a.passed {
                if let Some(exp) = &a.expected {
                    rows.push_str(&format!(
                        "<div class=\"ea\">expected: <code>{}</code></div>",
                        html_escape(&fmt_val(exp))
                    ));
                }
                if let Some(act) = &a.actual {
                    rows.push_str(&format!(
                        "<div class=\"ea\">actual: <code>{}</code></div>",
                        html_escape(&fmt_val(act))
                    ));
                }
            }
            rows.push_str("</div>");
        }
        rows.push_str("</div>");
    }

    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>noworries results</title>
<style>
  :root {{ color-scheme: light dark; }}
  body {{ font: 14px/1.5 -apple-system, system-ui, sans-serif; margin: 0; padding: 24px; background: Canvas; color: CanvasText; }}
  h1 {{ font-size: 20px; margin: 0 0 4px; }}
  .summary {{ margin-bottom: 20px; }}
  .status {{ font-weight: 700; padding: 2px 10px; border-radius: 6px; }}
  .status.ok {{ background: #16794322; color: #1a7f37; }}
  .status.fail {{ background: #cf222e22; color: #cf222e; }}
  .counts {{ color: #8a8a8a; margin-top: 6px; }}
  .check {{ border: 1px solid #8884; border-radius: 8px; padding: 12px 14px; margin: 10px 0; }}
  .chead {{ font-weight: 600; }}
  .badge {{ font-size: 11px; font-weight: 700; padding: 1px 7px; border-radius: 5px; margin-right: 8px; }}
  .badge.pass {{ background: #16794322; color: #1a7f37; }}
  .badge.failrow {{ background: #cf222e22; color: #cf222e; }}
  .assert {{ margin: 6px 0 0 6px; }}
  .assert.afail {{ color: #cf222e; }}
  .kind {{ color: #8a8a8a; }}
  .ea {{ margin-left: 20px; color: #8a8a8a; }}
  .err {{ color: #cf222e; margin: 6px 0; }}
  code {{ background: #8882; padding: 1px 5px; border-radius: 4px; }}
</style></head>
<body>
  <div class="summary">
    <h1>noworries results</h1>
    <div><span class="status {status_class}">{status}</span></div>
    <div class="counts">{passed} passed · {failed} failed · {total} total</div>
  </div>
  {rows}
</body></html>
"#,
        status = status,
        status_class = status_class,
        passed = summary.passed,
        failed = summary.failed,
        total = summary.total,
        rows = rows,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{AssertionResult, CheckResult};

    fn summary() -> ReportSummary {
        ReportSummary {
            ready: false,
            passed: 1,
            failed: 1,
            total: 2,
            results: vec![
                CheckResult {
                    name: "ok check".into(),
                    passed: true,
                    assertions: vec![AssertionResult {
                        kind: "http".into(),
                        passed: true,
                        message: "GET / -> 200".into(),
                        expected: None,
                        actual: None,
                    }],
                    error: None,
                },
                CheckResult {
                    name: "bad & <check>".into(),
                    passed: false,
                    assertions: vec![AssertionResult {
                        kind: "http".into(),
                        passed: false,
                        message: "expected 200, got 500".into(),
                        expected: Some(serde_json::json!(200)),
                        actual: Some(serde_json::json!(500)),
                    }],
                    error: None,
                },
            ],
        }
    }

    #[test]
    fn junit_has_cases_and_escapes() {
        let x = junit_xml(&summary());
        assert!(x.contains("tests=\"2\" failures=\"1\""));
        assert!(x.contains("<testcase name=\"ok check\""));
        // failure only on the failing case, with XML-escaped name
        assert!(x.contains("bad &amp; &lt;check&gt;"));
        assert_eq!(x.matches("<failure").count(), 1);
    }

    #[test]
    fn html_renders_status_and_escapes() {
        let h = html_report(&summary());
        assert!(h.contains("NOT READY"));
        assert!(h.contains("1 passed"));
        assert!(h.contains("bad &amp; &lt;check&gt;"));
    }
}
