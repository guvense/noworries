//! noworries — ephemeral infra harness that lets an AI verify the changes it
//! just made. Phase 1: framework-agnostic architecture + Postgres infra
//! lifecycle (up / health / teardown). App start + checks + Kafka/Redis land in
//! later phases.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use clap::{Parser, Subcommand};

use noworries::spec::{NoworriesSpec, SPEC_FILENAME};
use noworries::{app, compose, framework, git, lifecycle, report, runner};

const OUT_DIR: &str = ".noworries";
const OUT_FILE: &str = "compose.test.yml";

/// The /noworries slash command, embedded so any install channel (curl|sh,
/// Homebrew, npm, cargo) can drop it into place via `noworries install-command`.
const SLASH_COMMAND_MD: &str = include_str!("../.claude/commands/noworries.md");

#[derive(Parser)]
#[command(
    name = "noworries",
    version,
    about = "Stand up the infra a feature needs, exercise it, and report READY / NOT READY."
)]
struct Cli {
    /// Skip the confirmation prompt (assume yes).
    #[arg(short = 'y', long, global = true)]
    yes: bool,

    /// Leave containers running after the run (for debugging).
    #[arg(long, global = true)]
    keep_alive: bool,

    /// Only include checks tagged with any of these (comma-separated).
    #[arg(long, value_delimiter = ',', global = true)]
    tags: Vec<String>,

    /// Hard cap on the whole run, in seconds.
    #[arg(long, default_value_t = 180, global = true)]
    timeout: u64,

    /// Also print machine-readable JSON where applicable.
    #[arg(long, global = true)]
    json: bool,

    /// Project directory.
    #[arg(long, default_value = ".", global = true)]
    dir: String,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
#[allow(clippy::enum_variant_names)]
enum Command {
    /// Scaffold a starter noworries.yml.
    Init,
    /// List the files the AI changed (for scoping checks). `--all` = everything.
    Changed {
        /// Cover all tracked files (the "force"/regression scope).
        #[arg(long, alias = "force")]
        all: bool,
    },
    /// Install the /noworries slash command for Claude Code.
    InstallCommand {
        /// Install into ./.claude/commands (this project) instead of ~/.claude/commands (all projects).
        #[arg(long)]
        project: bool,
    },
}

// --- teardown safety net: guarantees the app is stopped + `down -v` on
// exit / Ctrl-C, even if the run is interrupted mid-flight ---
struct Teardown {
    project: String,
    file: String,
    app_pid: Option<i32>,
}
static TEARDOWN: OnceLock<Mutex<Option<Teardown>>> = OnceLock::new();

fn teardown_slot() -> &'static Mutex<Option<Teardown>> {
    TEARDOWN.get_or_init(|| Mutex::new(None))
}

/// Record the app's process-group id so a signal handler can stop it too.
fn set_app_pid(pid: i32) {
    if let Some(t) = teardown_slot().lock().unwrap().as_mut() {
        t.app_pid = Some(pid);
    }
}

fn run_teardown() {
    if let Some(t) = teardown_slot().lock().unwrap().take() {
        if let Some(pid) = t.app_pid {
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        let _ = lifecycle::down(&t.project, &t.file);
    }
}

const ENV_FILE: &str = ".noworries.env";

/// Load `${VAR}` values from a gitignored `.noworries.env` (dotenv-style:
/// `KEY=VALUE` lines, `#` comments, optional surrounding quotes). This keeps
/// secrets (tokens, passwords) out of `noworries.yml` and out of git while
/// staying on disk so you don't have to `export` them every run.
fn load_env_file(dir: &str) -> BTreeMap<String, String> {
    let path = Path::new(dir).join(ENV_FILE);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    let m = runner::parse_env_file(&content);
    if !m.is_empty() {
        report::info(&format!("loaded {} variable(s) from {ENV_FILE}", m.len()));
    }
    m
}

fn short_run_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{:x}{:x}", millis, std::process::id())
}

fn confirm(question: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    print!("{question} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

const STARTER_SPEC: &str = r#"version: 1

# Infrastructure this feature needs. Supported: postgres, kafka, redis.
services:
  - postgres:16-alpine

# How to launch the app under test. Optional — auto-detected from the framework
# (Spring Boot today) when omitted. `framework:` can force a specific adapter.
# app:
#   start: "./mvnw spring-boot:run"
#   health: "/actuator/health"
#   ready_timeout: 180

checks:
  - name: "app health endpoint responds"
    tags: [smoke]
    request:
      method: GET
      path: /actuator/health
    expect:
      status: 200
"#;

fn cmd_init(dir: &str) -> Result<i32> {
    let path = Path::new(dir).join(SPEC_FILENAME);
    if path.exists() {
        report::warn(&format!(
            "{SPEC_FILENAME} already exists at {} — leaving it untouched.",
            path.display()
        ));
        return Ok(0);
    }
    std::fs::write(&path, STARTER_SPEC)?;
    report::ok(&format!("wrote a starter {SPEC_FILENAME} to {}", path.display()));
    report::info("Edit it to declare your services and checks, then run `noworries`.");
    Ok(0)
}

fn cmd_install_command(project: bool, dir: &str) -> Result<i32> {
    let base = if project {
        Path::new(dir).join(".claude").join("commands")
    } else {
        let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME is not set"))?;
        Path::new(&home).join(".claude").join("commands")
    };
    std::fs::create_dir_all(&base)?;
    let path = base.join("noworries.md");
    std::fs::write(&path, SLASH_COMMAND_MD)?;
    report::ok(&format!("installed /noworries slash command to {}", path.display()));
    if project {
        report::info("Available in this project. Run /noworries in Claude Code.");
    } else {
        report::info("Available in every Claude Code project. Run /noworries.");
    }
    Ok(0)
}

fn cmd_changed(dir: &str, all: bool, json: bool) -> Result<i32> {
    let files = if all {
        git::all_tracked(dir)
    } else {
        git::changed_files(dir)
    };
    if json {
        println!("{}", serde_json::to_string(&files)?);
    } else {
        for f in &files {
            println!("{f}");
        }
    }
    Ok(0)
}

fn cmd_run(cli: &Cli) -> Result<i32> {
    let dir = cli.dir.clone();
    let spec_path = Path::new(&dir).join(SPEC_FILENAME);
    let spec = NoworriesSpec::load(&spec_path)?;

    report::info("");
    report::detected_services(&spec.services);

    // Resolve the framework (flexible architecture entry point).
    let forced = spec.app.as_ref().and_then(|a| a.framework.as_deref());
    let framework = framework::detect(Path::new(&dir), forced);
    match &framework {
        Some(f) => report::info(&format!("Framework: {}", f.name())),
        None => report::warn(
            "no known framework detected — set `app.start` in noworries.yml so the app can be launched.",
        ),
    }

    let selected = spec.select_checks(&cli.tags);
    let scope = if cli.tags.is_empty() {
        "all checks".to_string()
    } else {
        format!("tags [{}]", cli.tags.join(", "))
    };
    report::info(&format!(
        "Checks selected ({scope}): {} of {}.",
        selected.len(),
        spec.checks.len()
    ));

    if !cli.yes && !confirm("Generate .noworries/compose.test.yml and start these services?") {
        report::warn("Not confirmed. Re-run with --yes to proceed.");
        return Ok(2);
    }

    lifecycle::preflight()?;

    let run_id = short_run_id();
    let compose = compose::generate(&spec.services, &run_id)?;

    let out_dir = Path::new(&dir).join(OUT_DIR);
    std::fs::create_dir_all(&out_dir)?;
    let out_path = out_dir.join(OUT_FILE);
    let out_path_str = out_path.to_string_lossy().to_string();
    std::fs::write(&out_path, &compose.yaml)?;
    report::ok(&format!(
        "generated {}/{} (project: {})",
        OUT_DIR, OUT_FILE, compose.project_name
    ));

    if !cli.keep_alive {
        *teardown_slot().lock().unwrap() = Some(Teardown {
            project: compose.project_name.clone(),
            file: out_path_str.clone(),
            app_pid: None,
        });
    }

    report::info("");
    report::info("Starting containers…");
    let health_timeout = Duration::from_secs(cli.timeout.saturating_sub(30).max(30));

    let mut started_app: Option<app::StartedApp> = None;
    let exit = match lifecycle::up(&compose, &out_path_str, health_timeout) {
        Ok(handles) => {
            report::ok(&format!("containers healthy (project: {})", handles.project_name));
            report::endpoints(&handles.endpoints);
            run_checks_flow(cli, &spec, &selected, &dir, framework.as_deref(), &handles, &out_dir, &mut started_app)
        }
        Err(e) => {
            report::error_line(&e.to_string());
            1
        }
    };

    // Teardown: stop the app first, then the containers.
    if let Some(mut a) = started_app.take() {
        a.stop();
    }
    if !cli.keep_alive {
        report::info("");
        report::info("Tearing down containers…");
        run_teardown();
        report::ok("torn down.");
    }

    Ok(exit)
}

/// After infra is healthy: start the app (if any check needs it), run the
/// checks, print + persist the structured report, and return the exit code.
#[allow(clippy::too_many_arguments)]
fn run_checks_flow(
    cli: &Cli,
    spec: &NoworriesSpec,
    selected: &[noworries::spec::CheckSpec],
    dir: &str,
    framework: Option<&dyn framework::Framework>,
    handles: &lifecycle::RunHandles,
    out_dir: &Path,
    started_app: &mut Option<app::StartedApp>,
) -> i32 {
    if selected.is_empty() {
        report::info("");
        report::warn("no checks selected — infra is up and reachable, but nothing to verify.");
        return 0;
    }

    // Apply Elasticsearch index templates while the infra is up but BEFORE the
    // app starts, so app-created indices get the right mapping.
    runner::apply_elastic_templates(selected, &handles.endpoints);

    let needs_app = selected.iter().any(|c| c.request.is_some())
        || spec.app.as_ref().and_then(|a| a.start.as_ref()).is_some()
        || spec.auth.as_ref().map(|a| a.login.is_some()).unwrap_or(false);

    // Variables for ${VAR} interpolation in requests/auth. Precedence (low->high
    // at lookup time): app.env < .noworries.env file < process env.
    let mut env = spec.app.as_ref().map(|a| a.env.clone()).unwrap_or_default();
    for (k, v) in load_env_file(dir) {
        env.insert(k, v);
    }

    let app_port = if needs_app {
        report::info("");
        report::info("Starting app…");
        match app::start_app(spec.app.as_ref(), framework, Path::new(dir), &handles.endpoints, out_dir) {
            Ok(a) => {
                report::ok(&format!(
                    "app ready on http://127.0.0.1:{} (log: {}/app.log)",
                    a.port, OUT_DIR
                ));
                set_app_pid(a.pid());
                let port = a.port;
                *started_app = Some(a);
                port
            }
            Err(e) => {
                report::error_line(&e.to_string());
                return 1;
            }
        }
    } else {
        0
    };

    // Resolve auth (runs the login request if configured) now that the app is up.
    let auth = match runner::resolve_auth(spec.auth.as_ref(), app_port, &env) {
        Ok(a) => a,
        Err(e) => {
            report::error_line(&format!("auth setup failed: {e}"));
            return 1;
        }
    };
    if !auth.headers.is_empty() || !auth.query.is_empty() {
        report::ok("authenticated (auth applied to check requests)");
    }

    report::info("");
    report::info(&format!("Running {} check(s)…", selected.len()));
    let ctx = runner::RunnerContext {
        app_port,
        endpoints: handles.endpoints.clone(),
        env,
        auth_headers: auth.headers,
        auth_query: auth.query,
    };
    let results = runner::run_checks(selected, &ctx);
    let summary = report::print_results(results);

    let results_path = out_dir.join("results.json");
    if let Err(e) = report::write_results_json(&results_path, &summary) {
        report::warn(&format!("could not write results.json: {e}"));
    } else {
        report::info(&format!("(machine-readable results: {}/results.json)", OUT_DIR));
    }
    if cli.json {
        if let Ok(j) = serde_json::to_string(&summary) {
            println!("{j}");
        }
    }

    if cli.keep_alive {
        report::warn(&format!(
            "--keep-alive: leaving containers up. Tear down with: docker compose -f {} -p {} down -v",
            handles.compose_file, handles.project_name
        ));
    }

    if summary.ready {
        0
    } else {
        1
    }
}

fn main() {
    // Guarantee teardown on Ctrl-C / SIGTERM.
    let _ = ctrlc::set_handler(|| {
        run_teardown();
        std::process::exit(130);
    });

    let cli = Cli::parse();
    let result = match &cli.command {
        Some(Command::Init) => cmd_init(&cli.dir),
        Some(Command::Changed { all }) => cmd_changed(&cli.dir, *all, cli.json),
        Some(Command::InstallCommand { project }) => cmd_install_command(*project, &cli.dir),
        None => cmd_run(&cli),
    };

    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            report::error_line(&e.to_string());
            run_teardown();
            std::process::exit(1);
        }
    }
}
