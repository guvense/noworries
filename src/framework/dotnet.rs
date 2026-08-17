//! ASP.NET Core framework adapter (.NET web apps / minimal APIs).
//!
//! Detects a .NET project by a `*.csproj`, `*.fsproj`, or `*.sln` file. ASP.NET
//! Core has no universal health path (health checks are opt-in middleware), so
//! readiness is TCP-based (`health: "none"`) unless the spec sets one. The port
//! is controlled by `ASPNETCORE_HTTP_PORTS` (Kestrel reads it directly). Apps
//! are wired through [`conventional_env`]; configuration binds `ConnectionStrings`
//! from env via the `ConnectionStrings__<name>` convention, and the standard
//! `DATABASE_URL` / `REDIS_URL` vars plus `NOWORRIES_*` fallbacks are present.

use std::collections::BTreeMap;
use std::path::Path;

use super::{conventional_env, Framework};
use crate::lifecycle::ServiceEndpoint;

pub struct DotNet;

impl Framework for DotNet {
    fn name(&self) -> &'static str {
        "dotnet"
    }

    fn detect(&self, dir: &Path) -> bool {
        // Scan the top level for a project/solution file. `.csproj` covers C#,
        // `.fsproj` F#; `.sln` a multi-project solution.
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        entries.flatten().any(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.ends_with(".csproj") || name.ends_with(".fsproj") || name.ends_with(".sln")
        })
    }

    fn default_start_command(&self, _dir: &Path) -> Option<String> {
        // `dotnet run` builds and runs the project in the current directory.
        // `--no-launch-profile` is not optional: `dotnet new web` scaffolds a
        // `Properties/launchSettings.json` whose `applicationUrl` (5000/7001)
        // is applied as ASPNETCORE_URLS in Development and beats the port we
        // assign, so the app binds the wrong port and never looks ready.
        Some("dotnet run --no-launch-profile".to_string())
    }

    fn default_health_path(&self) -> &'static str {
        "none"
    }

    fn default_port_env(&self) -> &'static str {
        // Kestrel binds the HTTP port from ASPNETCORE_HTTP_PORTS (.NET 8+).
        "ASPNETCORE_HTTP_PORTS"
    }

    fn port_env_extra(&self, port: u16) -> BTreeMap<String, String> {
        // ASPNETCORE_URLS is the unambiguous knob (it also outranks
        // ASPNETCORE_HTTP_PORTS), and 0.0.0.0 keeps the app reachable from the
        // host regardless of the container/loopback setup.
        let mut m = BTreeMap::new();
        m.insert("ASPNETCORE_URLS".to_string(), format!("http://0.0.0.0:{port}"));
        m
    }

    fn env_wiring(&self, endpoints: &[ServiceEndpoint]) -> BTreeMap<String, String> {
        conventional_env(endpoints)
    }
}
