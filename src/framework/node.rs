//! Node.js framework adapter (Express and friends).
//!
//! Detects any Node project by its `package.json`. Node has no single health
//! convention, so readiness is TCP-based (`health: "none"`) unless the spec
//! sets `app.health`. Apps are wired through [`conventional_env`] and read the
//! `PORT` env var by convention.

use std::collections::BTreeMap;
use std::path::Path;

use super::{conventional_env, Framework};
use crate::lifecycle::ServiceEndpoint;

pub struct NodeJs;

impl Framework for NodeJs {
    fn name(&self) -> &'static str {
        "node"
    }

    fn detect(&self, dir: &Path) -> bool {
        dir.join("package.json").exists()
    }

    fn default_start_command(&self, dir: &Path) -> Option<String> {
        // Prefer an explicit `start` script; fall back to a conventional entry
        // file. `npm start` is the near-universal convention.
        if package_has_start_script(dir) {
            Some("npm start".to_string())
        } else if dir.join("server.js").exists() {
            Some("node server.js".to_string())
        } else if dir.join("index.js").exists() {
            Some("node index.js".to_string())
        } else if dir.join("app.js").exists() {
            Some("node app.js".to_string())
        } else {
            // Let `npm start` surface the error rather than guessing wrong.
            Some("npm start".to_string())
        }
    }

    fn default_health_path(&self) -> &'static str {
        // No universal health endpoint in Node — wait for the TCP port instead.
        "none"
    }

    fn default_port_env(&self) -> &'static str {
        "PORT"
    }

    fn env_wiring(&self, endpoints: &[ServiceEndpoint]) -> BTreeMap<String, String> {
        conventional_env(endpoints)
    }
}

/// Does `package.json` declare a `scripts.start`? Best-effort string scan — we
/// avoid a JSON dependency and a false negative just means we fall back to a
/// conventional entry file.
fn package_has_start_script(dir: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join("package.json")) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    json.get("scripts")
        .and_then(|s| s.get("start"))
        .and_then(|v| v.as_str())
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}
