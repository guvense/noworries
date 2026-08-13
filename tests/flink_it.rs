//! Flink integration test against a REAL session cluster. Exercises the whole
//! submit path — multipart jar upload, `POST /jars/:id/run`, and polling until
//! the job reaches RUNNING — which no unit test can cover (this is exactly the
//! kind of client/REST-contract bug the Kafka offset-storage regression taught
//! us to catch with a live test).
//!
//! It uses the Flink image's built-in `TopSpeedWindowing.jar` example so no
//! sample project needs building. Runs only when both env vars are set:
//!   NOWORRIES_IT_FLINK_PORT — host port mapped to the JobManager REST API
//!   NOWORRIES_IT_FLINK_JAR  — path to an example jar copied out of the image
//! (CI copies /opt/flink/examples/streaming/TopSpeedWindowing.jar and sets these.)

use std::path::Path;

use noworries::flink::submit_all;
use noworries::spec::{FlinkJob, FlinkSpec};

#[test]
fn submits_example_job_and_reaches_running() {
    let Some(port) = std::env::var("NOWORRIES_IT_FLINK_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
    else {
        eprintln!("skipping flink IT: set NOWORRIES_IT_FLINK_PORT to the JobManager REST port");
        return;
    };
    let Some(jar_path) = std::env::var("NOWORRIES_IT_FLINK_JAR").ok() else {
        eprintln!("skipping flink IT: set NOWORRIES_IT_FLINK_JAR to an example jar path");
        return;
    };

    let jar = Path::new(&jar_path);
    let dir = jar.parent().expect("jar path should have a parent dir");
    let jar_name = jar
        .file_name()
        .expect("jar path should have a file name")
        .to_string_lossy()
        .to_string();

    let spec = FlinkSpec {
        image: None,
        taskmanagers: None,
        slots: None,
        submit_timeout: Some(120),
        jobs: vec![FlinkJob {
            name: Some("example".to_string()),
            build: None,
            jar: jar_name,
            entry_class: None,
            args: vec![],
            parallelism: Some(1),
        }],
    };

    let submitted = submit_all(dir, port, &spec).expect("example job should submit and run");
    assert_eq!(submitted.len(), 1);
    assert!(
        !submitted[0].job_id.is_empty(),
        "submitted job should have a job id, got {:?}",
        submitted[0].job_id
    );
}
