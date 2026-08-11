//! Starts the app under test as a subprocess wired to the ephemeral
//! containers, waits until it is ready, and stops the whole process group on
//! teardown.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::net::TcpStream;
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

    /// Stop the app's process group (SIGTERM, then SIGKILL). Idempotent.
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        unsafe {
            // Negative pid signals the whole process group (we spawned as leader).
            libc::kill(-self.pid, libc::SIGTERM);
        }
        for _ in 0..10 {
            if !process_group_alive(self.pid) {
                return;
            }
            sleep(Duration::from_millis(300));
        }
        unsafe {
            libc::kill(-self.pid, libc::SIGKILL);
        }
    }
}

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
/// every resolved service endpoint (falling back to generic vars).
pub fn build_env(
    app: Option<&AppSpec>,
    fw: Option<&dyn Framework>,
    endpoints: &[ServiceEndpoint],
    port: u16,
) -> BTreeMap<String, String> {
    let mut env = match fw {
        Some(f) => f.env_wiring(endpoints),
        None => framework::generic_env(endpoints),
    };
    let port_env = app
        .and_then(|a| a.port_env.clone())
        .unwrap_or_else(|| DEFAULT_PORT_ENV.to_string());
    env.insert(port_env, port.to_string());
    if let Some(a) = app {
        for (k, v) in &a.env {
            env.insert(k.clone(), v.clone());
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
pub fn start_app(
    app: Option<&AppSpec>,
    fw: Option<&dyn Framework>,
    dir: &Path,
    endpoints: &[ServiceEndpoint],
    out_dir: &Path,
) -> Result<StartedApp> {
    let command = resolve_start_command(app, fw, dir)?;
    let port = pick_free_port()?;
    let env = build_env(app, fw, endpoints, port);
    let health_path = app
        .and_then(|a| a.health.clone())
        .or_else(|| fw.map(|f| f.default_health_path().to_string()));
    let ready_timeout = Duration::from_secs(
        app.and_then(|a| a.ready_timeout).unwrap_or(DEFAULT_READY_TIMEOUT_SEC),
    );

    let log_path = out_dir.join("app.log");
    let log = File::create(&log_path)?;
    let log_err = log.try_clone()?;

    // process_group(0) makes the child its own group leader so we can signal the
    // whole tree (e.g. mvnw -> JVM child) at teardown.
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .current_dir(dir)
        .envs(&env)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()?;

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
