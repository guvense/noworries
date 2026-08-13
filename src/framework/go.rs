//! Go framework adapter (net/http, Gin, Echo, Fiber — anything built with
//! `go build`).
//!
//! Detects a Go module by `go.mod`. Go web apps have no standard health path,
//! so readiness is TCP-based (`health: "none"`) unless the spec sets one. Apps
//! are wired through [`conventional_env`] and read the `PORT` env var by
//! convention.

use std::collections::BTreeMap;
use std::path::Path;

use super::{conventional_env, Framework};
use crate::lifecycle::ServiceEndpoint;

pub struct Go;

impl Framework for Go {
    fn name(&self) -> &'static str {
        "go"
    }

    fn detect(&self, dir: &Path) -> bool {
        dir.join("go.mod").exists()
    }

    fn default_start_command(&self, _dir: &Path) -> Option<String> {
        // `go run .` builds and runs the main package in the module root.
        Some("go run .".to_string())
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
