//! Starts the app under test as a subprocess wired to the ephemeral
//! containers, waits until it is ready, and stops the whole process group on
//! teardown.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};

use crate::framework::{self, Framework};
use crate::lifecycle::ServiceEndpoint;
use crate::spec::AppSpec;

const DEFAULT_PORT_ENV: &str = "SERVER_PORT";
const DEFAULT_READY_TIMEOUT_SEC: u64 = 120;

pub struct StartedApp {
    pub port: u16,
    pub log_path: String,
    pid: i32,
    stopped: bool,
}

impl StartedApp {
    /// The app's process-group id (for external teardown/signal handling).
    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// Stop the app's whole process tree (SIGTERM → SIGKILL on Unix;
    /// `taskkill /T /F` on Windows). Idempotent.
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        kill_tree(self.pid);
    }
}

/// Build a [`Command`] that runs `command` through the platform shell, so
/// `app.start` / a Flink `build` / a setup hook can be an ordinary shell
/// one-liner on any OS.
pub fn shell_command(command: &str) -> Command {
    #[cfg(unix)]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    }
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(command);
        c
    }
}

/// Terminate the app's entire process tree (e.g. `mvnw` → JVM, `npm` → node).
///
/// - **Unix:** the child is its own process-group leader, so signal the whole
///   group — SIGTERM, wait briefly for a clean exit, then SIGKILL.
/// - **Windows:** `taskkill /T /F` kills the process and all its descendants.
pub fn kill_tree(pid: i32) {
    #[cfg(unix)]
    {
        unsafe {
            // Negative pid signals the whole process group (we spawned as leader).
            libc::kill(-pid, libc::SIGTERM);
        }
        for _ in 0..10 {
            if !process_group_alive(pid) {
                return;
            }
            sleep(Duration::from_millis(300));
        }
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(unix)]
fn process_group_alive(pid: i32) -> bool {
    // kill(-pid, 0) returns 0 while any member of the group exists.
    unsafe { libc::kill(-pid, 0) == 0 }
}

/// Ask the OS for a free TCP port by binding to :0.
pub fn pick_free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// Build the environment for the app: its port plus the framework's wiring for
/// every resolved service endpoint (falling back to generic vars). `vars` is the
/// `${VAR}` lookup env (`.noworries.env` + process env) used to interpolate
/// `app.env` values, so secrets referenced there resolve just like everywhere else.
/// Variables the **checks** can interpolate, on top of `app.env` /
/// `.noworries.env`.
///
/// HTTP checks and relative paths (`sse.path`, `websocket.url`, `graphql.path`)
/// resolve against the app's port on their own, but a fully-specified target —
/// `grpc.target`, an absolute `ws://` URL — cannot: the port is picked at run
/// time. Without a name for it, such a spec had to pin a fixed port through
/// `app.env`, which is exactly the workaround this removes.
///
/// Returns `(owned, fallback)`: `owned` are noworries' own `NOWORRIES_APP_*`
/// names and always win; `fallback` (the framework's port var, the per-service
/// host/port pairs) only fills names the spec hasn't defined itself.
pub fn check_runtime_vars(
    app_port: u16,
    endpoints: &[ServiceEndpoint],
    port_env: &str,
) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut owned = BTreeMap::new();
    let mut fallback = framework::generic_env(endpoints);
    if app_port != 0 {
        owned.insert("NOWORRIES_APP_HOST".to_string(), "127.0.0.1".to_string());
        owned.insert("NOWORRIES_APP_PORT".to_string(), app_port.to_string());
        owned.insert(
            "NOWORRIES_APP_URL".to_string(),
            format!("http://127.0.0.1:{app_port}"),
        );
        fallback.insert(port_env.to_string(), app_port.to_string());
    }
    (owned, fallback)
}

/// The env var the app reads its port from.
///
/// Precedence: explicit `app.port_env` > the framework's convention > generic
/// default. Node/FastAPI/Go read `PORT`; Spring Boot reads `SERVER_PORT`.
/// Shared with the runner so a check can interpolate the same name.
pub fn port_env_name(app: Option<&AppSpec>, fw: Option<&dyn Framework>) -> String {
    app.and_then(|a| a.port_env.clone())
        .or_else(|| fw.map(|f| f.default_port_env().to_string()))
        .unwrap_or_else(|| DEFAULT_PORT_ENV.to_string())
}

pub fn build_env(
    app: Option<&AppSpec>,
    fw: Option<&dyn Framework>,
    endpoints: &[ServiceEndpoint],
    extra_env: &BTreeMap<String, String>,
    vars: &BTreeMap<String, String>,
    port: u16,
) -> BTreeMap<String, String> {
    let mut env = match fw {
        Some(f) => f.env_wiring(endpoints),
        None => framework::generic_env(endpoints),
    };
    // External/upstream dependency wiring (URLs + credentials) sits above the
    // framework's service wiring but below `app.env`, so an explicit `app.env`
    // entry can still override it.
    for (k, v) in extra_env {
        env.insert(k.clone(), v.clone());
    }
    env.insert(port_env_name(app, fw), port.to_string());
    if let Some(a) = app {
        for (k, v) in &a.env {
            // Interpolate ${VAR} so `app.env: { X: "${SECRET}" }` resolves from
            // .noworries.env / the process env instead of reaching the app literal.
            env.insert(k.clone(), crate::runner::interpolate(v, vars));
        }
    }
    env
}

fn http_healthy(port: u16, path: &str) -> bool {
    let url = format!("http://127.0.0.1:{port}{path}");
    match ureq::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .get(&url)
        .call()
    {
        Ok(resp) => (200..300).contains(&resp.status()),
        Err(ureq::Error::Status(code, _)) => (200..300).contains(&code),
        Err(_) => false,
    }
}

fn tcp_open(port: u16) -> bool {
    TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_secs(2),
    )
    .is_ok()
}

fn tail_file(path: &str, lines: usize) -> String {
    let mut s = String::new();
    if File::open(path).and_then(|mut f| f.read_to_string(&mut s)).is_err() {
        return String::new();
    }
    let all: Vec<&str> = s.trim_end().lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

/// Resolve the start command: explicit `app.start` wins, else the framework's
/// default, else an error asking for `app.start`.
pub fn resolve_start_command(
    app: Option<&AppSpec>,
    fw: Option<&dyn Framework>,
    dir: &Path,
) -> Result<String> {
    if let Some(cmd) = app.and_then(|a| a.start.clone()) {
        if !cmd.trim().is_empty() {
            return Ok(cmd);
        }
    }
    if let Some(f) = fw {
        if let Some(cmd) = f.default_start_command(dir) {
            return Ok(cmd);
        }
    }
    bail!(
        "could not determine how to start the app. Add `app.start` to noworries.yml \
         (e.g. app:\\n  start: \"./mvnw spring-boot:run\")."
    )
}

/// Launch the app and wait until ready. On failure (early exit or timeout) the
/// process is stopped before returning the error.
#[allow(clippy::too_many_arguments)]
pub fn start_app(
    app: Option<&AppSpec>,
    fw: Option<&dyn Framework>,
    dir: &Path,
    endpoints: &[ServiceEndpoint],
    extra_env: &BTreeMap<String, String>,
    vars: &BTreeMap<String, String>,
    out_dir: &Path,
) -> Result<StartedApp> {
    let command = resolve_start_command(app, fw, dir)?;
    let port = pick_free_port()?;
    let env = build_env(app, fw, endpoints, extra_env, vars, port);
    let health_path = app
        .and_then(|a| a.health.clone())
        .or_else(|| fw.map(|f| f.default_health_path().to_string()));
    let ready_timeout = Duration::from_secs(
        app.and_then(|a| a.ready_timeout).unwrap_or(DEFAULT_READY_TIMEOUT_SEC),
    );

    let log_path = out_dir.join("app.log");
    let log = File::create(&log_path)?;
    let log_err = log.try_clone()?;

    // Start via the platform shell so `app.start` can be a shell one-liner.
    let mut cmd = shell_command(&command);
    cmd.current_dir(dir)
        .envs(&env)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    // Unix: make the child its own process-group leader so we can signal the
    // whole tree (mvnw -> JVM) at teardown. Windows: `taskkill /T` walks the tree.
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn()?;

    let pid = child.id() as i32;
    let log_path_str = log_path.to_string_lossy().to_string();
    let mut started = StartedApp {
        port,
        log_path: log_path_str.clone(),
        pid,
        stopped: false,
    };

    let use_http = health_path.as_deref().map(|p| p != "none").unwrap_or(false);
    let deadline = Instant::now() + ready_timeout;
    loop {
        // Did the process exit already?
        if let Ok(Some(status)) = child.try_wait() {
            let tail = tail_file(&log_path_str, 40);
            started.stop();
            bail!(
                "app exited before becoming ready (status {}).{}",
                status,
                if tail.is_empty() {
                    String::new()
                } else {
                    format!("\n---- app log (tail) ----\n{tail}\n------------------------")
                }
            );
        }

        let ready = if use_http {
            http_healthy(port, health_path.as_deref().unwrap())
        } else {
            tcp_open(port)
        };
        if ready {
            break;
        }

        if Instant::now() > deadline {
            let tail = tail_file(&log_path_str, 40);
            started.stop();
            bail!(
                "app did not become ready within {}s ({}).{}",
                ready_timeout.as_secs(),
                if use_http {
                    format!("probing GET {}", health_path.as_deref().unwrap())
                } else {
                    format!("probing TCP :{port}")
                },
                if tail.is_empty() {
                    String::new()
                } else {
                    format!("\n---- app log (tail) ----\n{tail}\n------------------------")
                }
            );
        }
        sleep(Duration::from_millis(1500));
    }

    Ok(started)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::ServiceEndpoint;
    use crate::spec::{AppSpec, ServiceKind};

    #[test]
    fn app_env_interpolates_from_vars() {
        let mut app = AppSpec::default();
        app.env.insert("API_KEY".into(), "${SECRET}".into());
        app.env.insert("PLAIN".into(), "literal".into());
        let mut vars = BTreeMap::new();
        vars.insert("SECRET".into(), "s3cret".into());

        let env = build_env(Some(&app), None, &[], &BTreeMap::new(), &vars, 8080);
        assert_eq!(env.get("API_KEY").map(String::as_str), Some("s3cret"));
        assert_eq!(env.get("PLAIN").map(String::as_str), Some("literal"));
        assert_eq!(env.get("SERVER_PORT").map(String::as_str), Some("8080"));
    }

    fn ep(kind: ServiceKind, host_port: u16) -> ServiceEndpoint {
        ServiceEndpoint {
            service: "svc".into(),
            kind,
            host_port,
            container_port: 5432,
            aux_ports: Default::default(),
        }
    }

    #[test]
    fn checks_can_name_the_apps_ephemeral_port() {
        // Without these a `grpc.target` (or any absolute URL) had no way to
        // reach the app, and the spec had to pin a port via app.env.
        let (owned, fallback) = check_runtime_vars(54321, &[], "PORT");
        assert_eq!(owned.get("NOWORRIES_APP_PORT").map(String::as_str), Some("54321"));
        assert_eq!(owned.get("NOWORRIES_APP_HOST").map(String::as_str), Some("127.0.0.1"));
        assert_eq!(
            owned.get("NOWORRIES_APP_URL").map(String::as_str),
            Some("http://127.0.0.1:54321")
        );
        // The framework's own port var is a fallback, so app.env can override it.
        assert_eq!(fallback.get("PORT").map(String::as_str), Some("54321"));
    }

    #[test]
    fn checks_can_name_service_ports_too() {
        let (_, fallback) = check_runtime_vars(1, &[ep(ServiceKind::Postgres, 5555)], "PORT");
        assert_eq!(fallback.get("NOWORRIES_POSTGRES_PORT").map(String::as_str), Some("5555"));
        assert_eq!(fallback.get("NOWORRIES_POSTGRES_HOST").map(String::as_str), Some("127.0.0.1"));
    }

    #[test]
    fn no_app_means_no_app_vars() {
        // `app_port == 0` = no app started (a Flink-only run).
        let (owned, fallback) = check_runtime_vars(0, &[], "PORT");
        assert!(owned.is_empty());
        assert!(!fallback.contains_key("PORT"));
    }

    #[test]
    fn port_env_name_follows_precedence() {
        assert_eq!(port_env_name(None, None), "SERVER_PORT");
    }
}
