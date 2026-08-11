//! Integration test for the container lifecycle against a REAL Docker daemon.
//!
//! It needs a local image (with a `/svc health` healthcheck) because public
//! registries are blocked in CI sandboxes. Set NOWORRIES_IT_IMAGE to that image
//! to enable the test; if it's unset, the test skips (prints and returns).

use std::io::Write;
use std::net::TcpStream;
use std::time::Duration;

use noworries::compose::GeneratedCompose;
use noworries::lifecycle::{down, up};
use noworries::services::ComposeService;
use noworries::spec::ServiceKind;

#[test]
fn postgres_lifecycle_up_health_port_teardown() {
    let image = match std::env::var("NOWORRIES_IT_IMAGE") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("skipping lifecycle IT: set NOWORRIES_IT_IMAGE to a local image with a `/svc health` healthcheck");
            return;
        }
    };

    let dir = std::env::temp_dir().join("noworries-rsit");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("compose.test.yml");
    let yaml = format!(
        r#"name: noworries-rsit
networks:
  noworries:
    name: noworries-rsit_net
    driver: bridge
services:
  postgres:
    image: {image}
    ports:
      - "5432"
    networks:
      - noworries
    healthcheck:
      test: ["CMD", "/svc", "health"]
      interval: 2s
      timeout: 3s
      retries: 15
      start_period: 1s
"#
    );
    std::fs::File::create(&file)
        .unwrap()
        .write_all(yaml.as_bytes())
        .unwrap();
    let file_str = file.to_string_lossy().to_string();

    let compose = GeneratedCompose {
        project_name: "noworries-rsit".to_string(),
        yaml: String::new(), // up() reads the file on disk, not this field
        services: vec![ComposeService {
            name: "postgres".to_string(),
            kind: ServiceKind::Postgres,
            container_port: 5432,
            has_healthcheck: true,
            host_port: None,
            body: serde_yaml::Value::Null,
        }],
    };

    let handles = up(&compose, &file_str, Duration::from_secs(60))
        .expect("up() should bring the container to healthy and resolve ports");

    let ep = handles
        .endpoints
        .iter()
        .find(|e| e.kind == ServiceKind::Postgres)
        .expect("postgres endpoint resolved");
    assert!(ep.host_port > 0, "host port should be a real random port");

    // The resolved host port should actually accept a TCP connection.
    let addr = format!("127.0.0.1:{}", ep.host_port);
    let reachable = TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(3)).is_ok();

    // Always tear down before asserting, so a failure doesn't leak containers.
    down(&handles.project_name, &file_str).unwrap();

    assert!(reachable, "resolved host port {} was not reachable", ep.host_port);
}
