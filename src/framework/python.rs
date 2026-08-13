//! Python framework adapter (FastAPI / Uvicorn-based ASGI apps).
//!
//! Detects a Python web project by `pyproject.toml`, `requirements.txt`, or a
//! conventional `main.py`. FastAPI has no standard health path, so readiness is
//! TCP-based (`health: "none"`) unless the spec sets one. Apps are wired through
//! [`conventional_env`] and read the `PORT` env var by convention.

use std::collections::BTreeMap;
use std::path::Path;

use super::{conventional_env, Framework};
use crate::lifecycle::ServiceEndpoint;

pub struct FastApi;

impl Framework for FastApi {
    fn name(&self) -> &'static str {
        "fastapi"
    }

    fn detect(&self, dir: &Path) -> bool {
        // `main.py` / `app.py` are the FastAPI quickstart conventions. Failing
        // that, a Python manifest that actually references FastAPI/Uvicorn is a
        // strong signal — this keeps us from claiming every Python repo.
        let has_entry = dir.join("main.py").exists() || dir.join("app.py").exists();
        has_entry || manifest_mentions_fastapi(dir)
    }

    fn default_start_command(&self, dir: &Path) -> Option<String> {
        // Uvicorn honours $PORT via --port; point it at the conventional module.
        let module = if dir.join("main.py").exists() {
            "main:app"
        } else if dir.join("app.py").exists() {
            "app:app"
        } else {
            "main:app"
        };
        Some(format!(
            "uvicorn {module} --host 0.0.0.0 --port ${{PORT:-8000}}"
        ))
    }

    fn default_health_path(&self) -> &'static str {
        "none"
    }

    fn default_port_env(&self) -> &'static str {
        "PORT"
    }

    fn env_wiring(&self, endpoints: &[ServiceEndpoint]) -> BTreeMap<String, String> {
        conventional_env(endpoints)
    }
}

/// Does a Python manifest reference FastAPI or Uvicorn? Best-effort substring
/// scan of `pyproject.toml` / `requirements.txt`.
fn manifest_mentions_fastapi(dir: &Path) -> bool {
    for manifest in ["pyproject.toml", "requirements.txt"] {
        if let Ok(text) = std::fs::read_to_string(dir.join(manifest)) {
            let lower = text.to_ascii_lowercase();
            if lower.contains("fastapi") || lower.contains("uvicorn") {
                return true;
            }
        }
    }
    false
}
