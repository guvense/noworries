//! Framework adapters. A [`Framework`] knows how to detect its kind of project,
//! how to start it, where its health endpoint lives, and how to translate
//! resolved service endpoints into environment variables. This module owns only
//! the interface, the registry, and shared helpers — each framework lives in
//! its own file under `framework/`. Adding one is a new file + one line in
//! [`registry`].

use std::collections::BTreeMap;
use std::path::Path;

use crate::lifecycle::ServiceEndpoint;

pub mod spring_boot;

pub use spring_boot::SpringBoot;

/// The interface every framework implements.
pub trait Framework {
    fn name(&self) -> &'static str;
    /// Does this framework appear to be used in `dir`?
    fn detect(&self, dir: &Path) -> bool;
    /// Best-guess command to start the app, if inferable from `dir`.
    fn default_start_command(&self, dir: &Path) -> Option<String>;
    /// Health endpoint polled until 2xx before checks run.
    fn default_health_path(&self) -> &'static str;
    /// Environment variables wiring the app to the resolved service endpoints.
    fn env_wiring(&self, endpoints: &[ServiceEndpoint]) -> BTreeMap<String, String>;
}

/// Registry / composition root: all known frameworks, in detection-priority
/// order. This is the single place that knows the concrete implementations.
pub fn registry() -> Vec<Box<dyn Framework>> {
    vec![Box::new(SpringBoot)]
    // Future: Box::new(Go), Box::new(NodeExpress), ...
}

/// Resolve the framework: an explicit name wins, otherwise auto-detect.
pub fn detect(dir: &Path, forced: Option<&str>) -> Option<Box<dyn Framework>> {
    let regs = registry();
    match forced {
        Some(name) => regs.into_iter().find(|f| f.name().eq_ignore_ascii_case(name)),
        None => regs.into_iter().find(|f| f.detect(dir)),
    }
}

/// Framework-agnostic vars every framework receives in addition to its own.
/// Shared helper for framework implementations.
pub fn generic_env(endpoints: &[ServiceEndpoint]) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for ep in endpoints {
        let k = ep.kind.as_str().to_uppercase();
        m.insert(format!("NOWORRIES_{k}_HOST"), "127.0.0.1".to_string());
        m.insert(format!("NOWORRIES_{k}_PORT"), ep.host_port.to_string());
    }
    m
}
