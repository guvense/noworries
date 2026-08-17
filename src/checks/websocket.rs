//! WebSocket assertion: connect, optionally send a message, then read until a
//! received message matches (deep-subset on its JSON) or the timeout elapses.

use std::time::{Duration, Instant};

use serde_json::Value as Json;

use super::{fail, pass, resolve_target, yaml_json, Assertion, Phase};
use crate::runner::{interpolate_json, matches_subset, AssertionResult, RunnerContext};
use crate::spec::CheckSpec;

pub struct WebSocket;

impl Assertion for WebSocket {
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, _retry: Duration) -> Vec<AssertionResult> {
        let Some(w) = &check.websocket else { return vec![] };
        // Relative path → ws://127.0.0.1:<app_port>; ws://host is kept.
        let url = {
            let s = resolve_target(&w.url, ctx, &["ws://", "wss://", "http://", "https://"]);
            s.replacen("http://", "ws://", 1).replacen("https://", "wss://", 1)
        };
        let timeout = Duration::from_millis(w.timeout_ms.unwrap_or(5000));
        let expected = yaml_json(&w.expect_message);

        match evaluate(w, ctx, &url, &expected, timeout) {
            Ok(true) => vec![pass("websocket", format!("matching message from {}", w.url))],
            Ok(false) => vec![fail(
                "websocket",
                format!("no matching message from {} within {}ms", w.url, timeout.as_millis()),
                Some(expected),
                None,
            )],
            Err(e) => vec![fail("websocket", e, None, None)],
        }
    }

    /// A socket that only listens is an observer; one that `send`s a message is
    /// acting on the app, so it runs in the trigger slot.
    fn phase(&self, check: &CheckSpec) -> Phase {
        match &check.websocket {
            Some(w) if w.send.is_some() && super::observes_elsewhere(check) => Phase::Trigger,
            _ => Phase::Observe,
        }
    }
}

fn evaluate(
    w: &crate::spec::WebsocketAssertion,
    ctx: &RunnerContext,
    url: &str,
    expected: &Json,
    timeout: Duration,
) -> Result<bool, String> {
    use tungstenite::client::IntoClientRequest;
    use tungstenite::http::header::{HeaderName, HeaderValue};
    use tungstenite::stream::MaybeTlsStream;
    use tungstenite::Message;

    // Build the handshake request so auth headers (from `auth`) can be attached.
    let mut request = url
        .into_client_request()
        .map_err(|e| format!("bad ws url {url}: {e}"))?;
    for (k, v) in &ctx.auth_headers {
        if let (Ok(name), Ok(val)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            request.headers_mut().insert(name, val);
        }
    }
    let (mut socket, _resp) =
        tungstenite::connect(request).map_err(|e| format!("connect {url} failed: {e}"))?;

    // Bound each read so a quiet stream can't hang past the timeout.
    if let MaybeTlsStream::Plain(s) = socket.get_ref() {
        let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
    }

    if let Some(send) = &w.send {
        let text = match send {
            serde_yaml::Value::String(s) => s.clone(),
            other => serde_json::to_string(&interpolate_json(&yaml_json(other), &ctx.env))
                .unwrap_or_default(),
        };
        socket
            .send(Message::Text(text))
            .map_err(|e| format!("send failed: {e}"))?;
    }

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match socket.read() {
            Ok(Message::Text(t)) => {
                let parsed: Json = serde_json::from_str(&t).unwrap_or(Json::String(t));
                if matches_subset(expected, &parsed) {
                    let _ = socket.close(None);
                    return Ok(true);
                }
            }
            Ok(Message::Binary(b)) => {
                if let Ok(parsed) = serde_json::from_slice::<Json>(&b) {
                    if matches_subset(expected, &parsed) {
                        let _ = socket.close(None);
                        return Ok(true);
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {} // ping/pong/frame — ignore
            Err(tungstenite::Error::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // read timed out — keep waiting until the overall deadline
            }
            Err(e) => return Err(format!("read failed: {e}")),
        }
    }
    let _ = socket.close(None);
    Ok(false)
}
