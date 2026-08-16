//! Elasticsearch integration test against a REAL cluster. Exercises template
//! application, insert/update/delete operations, and document + search
//! verification through the runner — the ES REST paths that can't be validated
//! by compile checks. Runs only when NOWORRIES_IT_ES_PORT points at ES on
//! localhost (security disabled).

use noworries::lifecycle::ServiceEndpoint;
use noworries::runner::{apply_elastic_templates, run_checks, RunnerContext};
use noworries::spec::{NoworriesSpec, ServiceKind};

#[test]
fn template_ops_and_verification_roundtrip() {
    let Some(port) = std::env::var("NOWORRIES_IT_ES_PORT").ok().and_then(|p| p.parse::<u16>().ok())
    else {
        eprintln!("skipping elastic IT: set NOWORRIES_IT_ES_PORT to an Elasticsearch on localhost");
        return;
    };

    let yaml = r#"
version: 1
services: [elasticsearch]
checks:
  - name: "index template + ops + verify"
    elastic:
      index: "noworries-ci"
      template:
        name: "noworries-ci-template"
        body:
          index_patterns: ["noworries-ci*"]
          template:
            mappings:
              properties:
                sku:    { type: keyword }
                status: { type: keyword }
      operations:
        - insert: { id: "IT1", document: { sku: "IT1", status: "PENDING" } }
        - update: { id: "IT1", document: { status: "SHIPPED" } }
        - insert: { id: "IT2", document: { sku: "IT2", status: "PENDING" } }
        - delete: { id: "IT2" }
      doc_id: "IT1"
      expect_exists: true
      expect_source_contains: { status: "SHIPPED" }
      query: { term: { sku: "IT1" } }
      expect_hits: 1
"#;
    let spec = NoworriesSpec::parse(yaml).unwrap();
    let endpoints = vec![ServiceEndpoint {
        service: "elastic".into(),
        kind: ServiceKind::Elastic,
        host_port: port,
        container_port: 9200,
        aux_ports: Default::default(),
    }];

    // Templates are applied before the app starts in a real run.
    apply_elastic_templates(&spec.checks, &endpoints);

    let ctx = RunnerContext::new(0, endpoints);
    let results = run_checks(&spec.checks, &ctx);
    assert!(
        results[0].passed,
        "elastic ops+verify should pass, got {:?}",
        results[0]
    );
}
