//! In-process mocks of external dependencies.
//!
//! When an [`ExternalSpec`](crate::spec::ExternalSpec) declares a `mock`,
//! noworries starts a small local HTTP server that answers the declared stubs
//! and **records every request** the app makes to it. The mock's URL is injected
//! as the external's URL (see [`crate::externals`]), so the app calls the mock
//! instead of a real sandbox, and a check's `external_calls:` assertion verifies
//! the app actually reached out with the expected method/path/body.
//!
//! The server is a blocking [`tiny_http`] listener on a random localhost port,
//! driven by a background thread; [`MockHandle::stop`] unblocks and joins it at
//! teardown.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use serde_json::Value as Json;

use crate::spec::MockSpec;

/// One request the app made to a mock.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    /// Path only (query string stripped).
    pub path: String,
    /// Raw request body bytes.
    pub body: Vec<u8>,
}

impl RecordedRequest {
    /// The body parsed as JSON (or `Null` if it isn't JSON).
    pub fn json(&self) -> Json {
        serde_json::from_slice(&self.body).unwrap_or(Json::Null)
    }
}

/// Shared, thread-safe list of recorded requests for one mock.
pub type Recorder = Arc<Mutex<Vec<RecordedRequest>>>;

/// A running mock server. Keep it alive for the duration of the run; drop or
/// [`stop`](MockHandle::stop) it at teardown.
pub struct MockHandle {
    pub name: String,
    pub url: String,
    recorder: Recorder,
    server: Arc<tiny_http::Server>,
    thread: Option<JoinHandle<()>>,
}

impl MockHandle {
    /// A clone of the shared recorder (for the runner to inspect).
    pub fn recorder(&self) -> Recorder {
        Arc::clone(&self.recorder)
    }

    /// Stop the server and join its thread. Idempotent.
    pub fn stop(&mut self) {
        self.server.unblock();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for MockHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start a mock server for `spec` on a random localhost port.
pub fn start_mock(name: &str, spec: &MockSpec) -> Result<MockHandle> {
    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| anyhow::anyhow!("failed to start mock \"{name}\": {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .context("mock server has no IP address")?
        .port();
    let url = format!("http://127.0.0.1:{port}");

    let server = Arc::new(server);
    let recorder: Recorder = Arc::new(Mutex::new(Vec::new()));

    let srv = Arc::clone(&server);
    let rec = Arc::clone(&recorder);
    let stubs = spec.stubs.clone();
    let thread = std::thread::spawn(move || {
        for mut request in srv.incoming_requests() {
            let method = request.method().as_str().to_uppercase();
            let full = request.url().to_string();
            let path = full.split('?').next().unwrap_or(&full).to_string();

            let mut body = Vec::new();
            let _ = std::io::Read::read_to_end(request.as_reader(), &mut body);

            if let Ok(mut r) = rec.lock() {
                r.push(RecordedRequest {
                    method: method.clone(),
                    path: path.clone(),
                    body: body.clone(),
                });
            }

            let (response, delay_ms) = build_response(&stubs, &method, &path, &body);
            if let Some(ms) = delay_ms {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
            let _ = request.respond(response);
        }
    });

    Ok(MockHandle {
        name: name.to_string(),
        url,
        recorder,
        server,
        thread: Some(thread),
    })
}

/// Pick the first matching stub and build its response; default 200 empty.
/// Returns the response and an optional artificial delay (ms) to apply first.
fn build_response(
    stubs: &[crate::spec::MockStub],
    method: &str,
    path: &str,
    req_body: &[u8],
) -> (tiny_http::Response<std::io::Cursor<Vec<u8>>>, Option<u64>) {
    let req_json: Option<Json> = serde_json::from_slice(req_body).ok();
    let stub = stubs.iter().find(|s| {
        let method_ok = s
            .when_
            .method
            .as_ref()
            .map(|m| m.eq_ignore_ascii_case(method))
            .unwrap_or(true);
        let path_ok = s.when_.path == path;
        let body_ok = match &s.when_.body_contains {
            None => true,
            Some(expected) => req_json
                .as_ref()
                .map(|actual| crate::runner::matches_subset(&yaml_to_json(expected), actual))
                .unwrap_or(false),
        };
        method_ok && path_ok && body_ok
    });

    struct Built {
        status: u16,
        body: Vec<u8>,
        headers: Vec<(String, String)>,
        delay: Option<u64>,
    }
    let built = match stub {
        Some(s) => {
            let body = match &s.respond.body {
                Some(serde_yaml::Value::String(text)) => text.clone().into_bytes(),
                Some(other) => serde_json::to_vec(&yaml_to_json(other)).unwrap_or_default(),
                None => Vec::new(),
            };
            let headers = s
                .respond
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            Built { status: s.respond.status.unwrap_or(200), body, headers, delay: s.respond.delay_ms }
        }
        None => Built { status: 200, body: Vec::new(), headers: Vec::new(), delay: None },
    };
    let Built { status, body, headers, delay } = built;

    let mut resp = tiny_http::Response::from_data(body).with_status_code(status);
    let has_ct = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
    for (k, v) in &headers {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp.add_header(h);
        }
    }
    if !has_ct {
        if let Ok(h) = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]) {
            resp.add_header(h);
        }
    }
    (resp, delay)
}

fn yaml_to_json(v: &serde_yaml::Value) -> Json {
    serde_json::to_value(v).unwrap_or(Json::Null)
}
