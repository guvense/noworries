//! Django framework adapter (Django / Django REST Framework).
//!
//! Detects a Django project by its `manage.py` entry point. Django has no
//! conventional health endpoint, so readiness is TCP-based (`health: "none"`)
//! unless the spec sets one. Apps are wired through [`conventional_env`]: Django
//! settings typically read `DATABASE_URL` (dj-database-url) / `REDIS_URL`, and
//! the framework-agnostic `NOWORRIES_*` fallbacks are always present.

use std::collections::BTreeMap;
use std::path::Path;

use super::{conventional_env, Framework};
use crate::lifecycle::ServiceEndpoint;

pub struct Django;

impl Framework for Django {
    fn name(&self) -> &'static str {
        "django"
    }

    fn detect(&self, dir: &Path) -> bool {
        // `manage.py` is the universal Django project marker. Guard against a
        // stray file by also accepting the common `pyproject.toml`/`requirements`
        // + manage.py combination, but manage.py alone is authoritative.
        dir.join("manage.py").exists()
    }

    fn default_start_command(&self, _dir: &Path) -> Option<String> {
        // runserver honours the positional addr:port; bind all interfaces so the
        // host can reach it. `--noreload` avoids the autoreloader forking a child
        // the harness can't track for teardown.
        Some("python manage.py runserver 0.0.0.0:${PORT:-8000} --noreload".to_string())
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
