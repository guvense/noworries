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
use noworries::{
    app, compose, externals, flink, framework, git, lifecycle, mock, report, reports, runner,
};

const OUT_DIR: &str = ".noworries";
const OUT_FILE: &str = "compose.test.yml";

/// The /noworries skill, embedded so any install channel (curl|sh, Homebrew,
/// npm, cargo) can drop it into place via `noworries install-command`.
///
/// `SKILL.md` is the entry point Claude always loads; the `references/` files
/// are read on demand, so the whole directory has to ship together.
const SKILL_FILES: &[(&str, &str)] = &[
    ("SKILL.md", include_str!("../.claude/skills/noworries/SKILL.md")),
    (
        "references/checks.md",
        include_str!("../.claude/skills/noworries/references/checks.md"),
    ),
    (
        "references/services-and-env.md",
        include_str!("../.claude/skills/noworries/references/services-and-env.md"),
    ),
    (
        "references/scenarios.md",
        include_str!("../.claude/skills/noworries/references/scenarios.md"),
    ),
    (
        "references/externals.md",
        include_str!("../.claude/skills/noworries/references/externals.md"),
    ),
    (
        "references/flink.md",
        include_str!("../.claude/skills/noworries/references/flink.md"),
    ),
    (
        "references/troubleshooting.md",
        include_str!("../.claude/skills/noworries/references/troubleshooting.md"),
    ),
];

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

    /// Write a JUnit XML report to this path (for CI).
    #[arg(long, global = true)]
    junit: Option<String>,

    /// Write a self-contained HTML report to this path.
    #[arg(long, global = true)]
    html: Option<String>,

    /// Write/refresh `snapshot` golden files instead of failing on a diff.
    #[arg(long, global = true)]
    update_snapshots: bool,

    /// Project directory.
    #[arg(long, default_value = ".", global = true)]
    dir: String,

    /// Spec file to use instead of `<dir>/noworries.yml` (lets one repo hold
    /// several scopes, e.g. `noworries --file orders.yml`).
    #[arg(long, global = true)]
    file: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
#[allow(clippy::enum_variant_names)]
enum Command {
    /// Scaffold a starter noworries.yml.
    Init {
        /// Include ready-to-edit example blocks for specific features
        /// (comma-separated): externals-mock, scenario, flink, auth, graphql,
        /// grpc, metrics, sse, websocket, schema.
        #[arg(long, value_delimiter = ',')]
        with: Vec<String>,
    },
    /// Parse + validate the spec without starting any containers.
    Validate,
    /// Print the `noworries.yml` field reference (markdown) or JSON Schema.
    #[command(alias = "schema")]
    Spec {
        /// Output format: `md` (human reference) or `json` (JSON Schema for
        /// editor completion / programmatic queries).
        #[arg(long, value_parser = ["md", "json"], default_value = "md")]
        format: String,
    },
    /// List the files the AI changed (for scoping checks). `--all` = everything.
    Changed {
        /// Cover all tracked files (the "force"/regression scope).
        #[arg(long, alias = "force")]
        all: bool,
    },
    /// Install the /noworries skill for Claude Code.
    InstallCommand {
        /// Install into ./.claude/skills (this project) instead of ~/.claude/skills (all projects).
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
            app::kill_tree(pid);
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

/// Extract `${VAR}` names referenced in a text blob.
fn extract_var_refs(text: &str) -> std::collections::BTreeSet<String> {
    let mut set = std::collections::BTreeSet::new();
    let mut rest = text;
    while let Some(i) = rest.find("${") {
        let after = &rest[i + 2..];
        if let Some(j) = after.find('}') {
            let name = after[..j].trim();
            if !name.is_empty() {
                set.insert(name.to_string());
            }
            rest = &after[j + 1..];
        } else {
            break;
        }
    }
    set
}

fn append_env_file(dir: &str, key: &str, value: &str) {
    let path = Path::new(dir).join(ENV_FILE);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{key}={value}");
    }
}

/// Interactively prompt for any `${VAR}` referenced in `noworries.yml` that
/// isn't resolvable yet, saving answers to `.noworries.env`. No-op when stdin
/// isn't a TTY (e.g. CI) — there, an unset var just warns at use time.
fn prompt_missing_env(dir: &str, spec_path: &Path) {
    if !std::io::stdin().is_terminal() {
        return;
    }
    let Ok(spec_text) = std::fs::read_to_string(spec_path) else {
        return;
    };
    let refs = extract_var_refs(&spec_text);
    if refs.is_empty() {
        return;
    }
    let file_env = {
        let p = Path::new(dir).join(ENV_FILE);
        std::fs::read_to_string(&p)
            .map(|c| runner::parse_env_file(&c))
            .unwrap_or_default()
    };
    let missing: Vec<String> = refs
        .into_iter()
        .filter(|v| !file_env.contains_key(v.as_str()) && std::env::var(v).is_err())
        .collect();
    if missing.is_empty() {
        return;
    }
    report::info("");
    report::info(&format!(
        "noworries.yml references {} variable(s) not set yet: {}",
        missing.len(),
        missing.join(", ")
    ));
    report::info(&format!("Enter values (saved to {ENV_FILE}; leave blank to skip):"));
    for var in missing {
        print!("  {var}=");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            continue;
        }
        let val = line.trim();
        if !val.is_empty() {
            append_env_file(dir, &var, val);
        }
    }
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

# Infrastructure this feature needs. Supported:
#   postgres · mysql · mongodb · redis · kafka · elastic(search)
# Declare as `kind` or `kind:tag`.
services:
  - postgres:16-alpine
  # - redis
  # - kafka

# How to launch the app under test. Optional — auto-detected from the framework
# (Spring Boot, Go, FastAPI, Node.js) when omitted. `framework:` forces one.
# app:
#   start: "./mvnw spring-boot:run"
#   health: "/actuator/health"        # or "none" for TCP-only readiness
#   ready_timeout: 180
#   env: { LOG_LEVEL: "info" }         # ${VAR} values resolve from .noworries.env

# Migrations / fixtures run after infra is up, before the app starts:
# setup:
#   - "./mvnw -q flyway:migrate"

# Upstream services the app calls out to (noworries injects the URL + creds;
# add `mock:` to stand up an in-process fake and assert calls via external_calls):
# externals:
#   - name: payments
#     url: "${PAYMENTS_URL}"
#     auth: { basic: { username: "${PAY_USER}", password: "${PAY_PASS}", header_env: PAYMENTS_AUTHORIZATION } }

# Auth for noworries' own requests to the app (login / bearer / basic / api_key):
# auth:
#   bearer: { token: "${API_TOKEN}" }

checks:
  - name: "app health endpoint responds"
    tags: [smoke]
    request: { method: GET, path: /actuator/health }
    expect:  { status: 200 }

  # A check can combine many assertion types (all must pass):
  # - name: "create order: 201, persists, indexed, fast"
  #   request: { method: POST, path: /orders, body: { sku: "ABC", qty: 2 } }
  #   expect:  { status: 201, body_contains: { sku: "ABC" }, max_ms: 500 }
  #   db:      { query: "SELECT status FROM orders WHERE sku='ABC'", expect_row: { status: "PENDING" } }
  #   logs:    { contains: ["OrderCreated"], absent: ["ERROR"] }
"#;

fn cmd_init(dir: &str, with: &[String]) -> Result<i32> {
    let path = Path::new(dir).join(SPEC_FILENAME);
    if path.exists() {
        report::warn(&format!(
            "{SPEC_FILENAME} already exists at {} — leaving it untouched.",
            path.display()
        ));
        return Ok(0);
    }
    let mut content = String::from(STARTER_SPEC);
    for feature in with {
        match template_for(feature) {
            Some(t) => content.push_str(t),
            None => report::warn(&format!(
                "unknown --with \"{feature}\" (known: {})",
                WITH_FEATURES.join(", ")
            )),
        }
    }
    std::fs::write(&path, content)?;
    report::ok(&format!("wrote a starter {SPEC_FILENAME} to {}", path.display()));
    if !with.is_empty() {
        report::info(&format!("included example blocks for: {}", with.join(", ")));
    }
    report::info("Edit it to declare your services and checks, then run `noworries`.");
    Ok(0)
}

const WITH_FEATURES: &[&str] = &[
    "externals-mock",
    "scenario",
    "flink",
    "auth",
    "graphql",
    "grpc",
    "metrics",
    "sse",
    "websocket",
    "schema",
];

/// A commented, ready-to-edit example block appended by `init --with <feature>`.
fn template_for(feature: &str) -> Option<&'static str> {
    Some(match feature {
        "externals-mock" => {
            r#"
# --- externals-mock: stand up an in-process fake of an upstream and assert the
# --- app called it. The mock's URL is injected as the external's URL.
# externals:
#   - name: payments
#     url_env: PAYMENTS_URL           # the mock URL is injected here (+ NOWORRIES_EXTERNAL_PAYMENTS_URL)
#     mock:
#       stubs:                        # matched top-to-bottom, first match wins
#         - when: { method: GET, path: /charge/1 }         # exact path; query string ignored
#           respond: { status: 200, body: { id: "1", status: "PAID" }, headers: { X-Trace: "t" } }
#         - when: { path: /webhook }  # method omitted → matches any method
#           respond: { status: 202 }
#       # any unmatched request is still recorded and answered 200 with an empty body
# checks:
#   - name: "creating an order charges the customer"
#     request: { method: POST, path: /orders, body: { sku: "A", amount: 100 } }
#     expect:  { status: 201 }
#     external_calls:
#       - external: payments
#         method: POST                # optional; omit to match any method
#         path: /charge               # exact path match
#         body_contains: { amount: 100 }   # deep-subset match on the recorded JSON body
#         times: 1                    # exact count; omit for "at least one"
"#
        }
        "scenario" => {
            r#"
# --- scenario: edge-case load/timing trigger (burst | concurrent | duplicates | out_of_order)
# checks:
#   - name: "burst of 500 orders: none dropped"
#     scenario:
#       kind: burst
#       count: 500
#       concurrency: 8
#       expect_throughput_per_sec: 1000   # optional gate
#       kafka: { topic: orders-in, key: "ord-${seq}", message: { id: "${uuid}", amount: "${seq}" } }
#     db: { query: "SELECT count(*) n FROM orders", expect_row: { n: 500 } }
"#
        }
        "flink" => {
            r#"
# --- flink: ephemeral session cluster + submit jobs, verify the pipeline end to end
# flink:
#   image: flink:1.19
#   slots: 2
#   jobs:
#     - build: "mvn -q -DskipTests package"    # optional
#       jar: target/pipeline-0.1.jar
#       entry_class: com.acme.Pipeline
# # the job reaches services by in-network name: kafka:9092, postgres:5432, elastic:9200
"#
        }
        "auth" => {
            r#"
# --- auth: applied to every check request (login | bearer | basic | api_key | oidc)
# auth:
#   login: { request: { method: POST, path: /auth/login, body: { username: "${U}", password: "${P}" } }, token_from: "$.accessToken" }
#   # bearer: { token: "${API_TOKEN}" }
#   # basic:  { username: "${U}", password: "${P}" }
#   # api_key: { header: "X-API-Key", value: "${API_KEY}" }
#   # oidc:   { issuer: "${ISSUER}", client_id: app, client_secret: "${OIDC_SECRET}", scope: "orders:read" }
#
# --- users + as: named identities, for role-based access checks
# users:
#   admin:  { oidc: { issuer: "${ISSUER}", client_id: admin-cli, client_secret: "${ADMIN_SECRET}" } }
#   reader: { bearer: { token: "${READER_TOKEN}" } }
# checks:
#   - name: "an admin can delete an order"
#     as: admin
#     request: { method: DELETE, path: /orders/1 }
#     expect:  { status: 204 }
#   - name: "a reader cannot"
#     as: reader
#     request: { method: DELETE, path: /orders/2 }
#     expect:  { status: 403 }
"#
        }
        "graphql" => {
            r#"
# --- graphql: POST a query/mutation, assert on data/errors
# checks:
#   - name: "order query"
#     graphql:
#       path: /graphql
#       query: "query($id: ID!) { order(id: $id) { status } }"
#       variables: { id: "ABC" }
#       expect_data: { order: { status: "PENDING" } }
#       expect_no_errors: true
"#
        }
        "grpc" => {
            r#"
# --- grpc: calls a method via grpcurl (must be on PATH; needs reflection or protos)
# checks:
#   - name: "GetOrder returns the order"
#     grpc:
#       target: "127.0.0.1:${NOWORRIES_APP_PORT}"   # or ${GRPC_PORT} if you serve HTTP + gRPC
#       method: "orders.OrderService/GetOrder"
#       data: { id: "ABC" }
#       expect_contains: { status: "PENDING" }
"#
        }
        "metrics" => {
            r#"
# --- metrics: scrape a Prometheus endpoint and compare a series value
# checks:
#   - name: "the request was counted"
#     request: { method: POST, path: /orders, body: { sku: "A" } }
#     expect:  { status: 201 }
#     metrics:
#       path: /actuator/prometheus
#       metric: http_server_requests_seconds_count
#       labels: { status: "201" }
#       expect: ">= 1"
"#
        }
        "sse" => {
            r#"
# --- sse: read an event stream until a matching event arrives
# checks:
#   - name: "an OrderCreated event is streamed"
#     sse:
#       path: /events
#       contains: { type: "OrderCreated" }
#       timeout_ms: 5000
"#
        }
        "websocket" => {
            r#"
# --- websocket: connect, optionally send, await a matching message
# checks:
#   - name: "subscription pushes the update"
#     websocket:
#       url: "/ws"                                  # relative → ws://<app>; or ws://127.0.0.1:${NOWORRIES_APP_PORT}/ws
#       send: { subscribe: "orders" }
#       expect_message: { type: "OrderCreated" }
#       timeout_ms: 5000
"#
        }
        "schema" => {
            r#"
# --- schema: assert a table's columns/types (Postgres or MySQL)
# checks:
#   - name: "orders table migrated correctly"
#     schema:
#       table: orders
#       has_columns: [id, sku, status]
#       columns: { qty: int, created_at: timestamp }
"#
        }
        _ => return None,
    })
}

/// Bundled spec reference (travels with the binary, so it can't drift from it).
const SPEC_REFERENCE_CONFIG: &str = include_str!("../docs/configuration.md");
const SPEC_REFERENCE_CHECKS: &str = include_str!("../docs/checks.md");

fn cmd_spec(format: &str) -> Result<i32> {
    if format == "json" {
        println!("{}", noworries::spec::json_schema());
    } else {
        println!("{SPEC_REFERENCE_CONFIG}");
        println!("\n---\n");
        println!("{SPEC_REFERENCE_CHECKS}");
    }
    Ok(0)
}

fn cmd_validate(cli: &Cli) -> Result<i32> {
    let path = spec_path(cli);
    match NoworriesSpec::load(&path) {
        Ok(spec) => {
            report::ok(&format!(
                "{} is valid — {} service(s), {} check(s){}{}",
                path.display(),
                spec.services.len(),
                spec.checks.len(),
                if spec.flink.is_some() { ", flink pipeline" } else { "" },
                if spec.externals.is_empty() {
                    String::new()
                } else {
                    format!(", {} external(s)", spec.externals.len())
                }
            ));
            Ok(0)
        }
        Err(e) => {
            report::error_line(&e.to_string());
            Ok(1)
        }
    }
}

fn cmd_install_command(project: bool, dir: &str) -> Result<i32> {
    // Install as an Agent Skill (directory + SKILL.md). Custom commands and
    // skills were merged in Claude Code: this gives `/noworries` and, because it
    // ships a `description`, lets Claude load it automatically when a change
    // needs verifying. Existing `.claude/commands/noworries.md` installs keep
    // working, but the skill form is the current one.
    let base = if project {
        Path::new(dir).join(".claude").join("skills").join("noworries")
    } else {
        let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME is not set"))?;
        Path::new(&home).join(".claude").join("skills").join("noworries")
    };
    for (rel, contents) in SKILL_FILES {
        let path = base.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
    }
    report::ok(&format!(
        "installed the /noworries skill to {} ({} files)",
        base.display(),
        SKILL_FILES.len()
    ));
    if project {
        report::info("Available in this project. Run /noworries in Claude Code (or let Claude invoke it).");
    } else {
        report::info("Available in every Claude Code project. Run /noworries (or let Claude invoke it).");
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

/// Resolve the spec file: `--file <path>` wins, else `<dir>/noworries.yml`.
fn spec_path(cli: &Cli) -> std::path::PathBuf {
    match &cli.file {
        Some(f) => Path::new(f).to_path_buf(),
        None => Path::new(&cli.dir).join(SPEC_FILENAME),
    }
}

fn cmd_run(cli: &Cli) -> Result<i32> {
    let dir = cli.dir.clone();
    let spec_path = spec_path(cli);
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

    // Ask for any ${VAR} the spec references but that isn't set yet (interactive
    // only), and save answers to .noworries.env so they persist and stay out of git.
    prompt_missing_env(&dir, &spec_path);

    lifecycle::preflight()?;

    let run_id = short_run_id();
    let compose = compose::generate(&spec.services, spec.flink.as_ref(), &run_id)?;

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

/// Run the declared setup/migration hooks with the app's env wiring (DB URLs,
/// external creds, etc.). Executed after infra is healthy and before the app
/// starts; a non-zero exit aborts the run.
#[allow(clippy::too_many_arguments)]
fn run_setup_hooks(
    cmds: &[String],
    dir: &str,
    app_spec: Option<&noworries::spec::AppSpec>,
    fw: Option<&dyn framework::Framework>,
    endpoints: &[lifecycle::ServiceEndpoint],
    external_env: &BTreeMap<String, String>,
    vars: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    // Same env the app will get (service wiring + app.env + externals). The port
    // env is irrelevant to a migration, so a placeholder is fine.
    let env = app::build_env(app_spec, fw, endpoints, external_env, vars, 0);
    for cmd in cmds {
        report::info(&format!("  $ {cmd}"));
        let status = app::shell_command(cmd)
            .current_dir(dir)
            .envs(&env)
            .status()
            .map_err(|e| anyhow::anyhow!("could not run `{cmd}`: {e}"))?;
        if !status.success() {
            anyhow::bail!("`{cmd}` exited with {status}");
        }
    }
    Ok(())
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

    // Stand up the Flink pipeline (build + submit jobs) if this run tests one.
    // Jobs must be RUNNING before checks trigger the pipeline.
    if let Some(fs) = &spec.flink {
        // Flink's KafkaSource won't auto-create source topics — pre-create the
        // topics the checks reference (plus any in `flink.topics`) so the job's
        // `describeTopics` finds them instead of hard-failing at startup.
        if spec.services.iter().any(|s| s.kind.as_str() == "kafka") {
            let topics = flink::referenced_topics(fs, selected);
            if !topics.is_empty() {
                let ensured = flink::ensure_kafka_topics(
                    &handles.compose_file,
                    &handles.project_name,
                    &topics,
                );
                if !ensured.is_empty() {
                    report::ok(&format!("ensured Kafka topic(s): {}", ensured.join(", ")));
                }
            }
        }
        match handles.aux_ports.get(flink::JOBMANAGER) {
            Some(&rest_port) => {
                report::info("");
                report::info(&format!(
                    "Submitting {} Flink job(s) (JobManager REST http://127.0.0.1:{rest_port})…",
                    fs.jobs.len()
                ));
                match flink::submit_all(Path::new(dir), rest_port, fs) {
                    Ok(jobs) => {
                        for j in &jobs {
                            report::ok(&format!("job running: {} ({})", j.name, j.job_id));
                        }
                    }
                    Err(e) => {
                        report::error_line(&format!("Flink job submission failed: {e}"));
                        return 1;
                    }
                }
            }
            None => {
                report::error_line("internal error: Flink JobManager REST port was not resolved");
                return 1;
            }
        }
    }

    let needs_app = selected.iter().any(|c| c.request.is_some())
        || spec.app.as_ref().and_then(|a| a.start.as_ref()).is_some()
        || spec.auth.as_ref().map(|a| a.login.is_some()).unwrap_or(false);

    // Variables for ${VAR} interpolation in requests/auth. Precedence (low->high
    // at lookup time): app.env < .noworries.env file < process env.
    let mut env = spec.app.as_ref().map(|a| a.env.clone()).unwrap_or_default();
    for (k, v) in load_env_file(dir) {
        env.insert(k, v);
    }

    // Start in-process mocks for any external that declares one, so the app can
    // call them and we can record + assert on those calls. Their URLs override
    // the external's declared `url`.
    let mut mocks: Vec<mock::MockHandle> = Vec::new();
    let mut mock_urls: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for ext in &spec.externals {
        if let Some(m) = &ext.mock {
            match mock::start_mock(&ext.name, m) {
                Ok(h) => {
                    report::ok(&format!("mock \"{}\" listening at {}", ext.name, h.url));
                    mock_urls.insert(ext.name.clone(), h.url.clone());
                    mocks.push(h);
                }
                Err(e) => {
                    report::error_line(&format!("failed to start mock \"{}\": {e}", ext.name));
                    return 1;
                }
            }
        }
    }

    // External/upstream dependencies (sandbox URLs + credentials) the app calls
    // out to — injected into the app's environment (not stood up by noworries).
    let external_env = externals::resolve_external_env(&spec.externals, &env, &mock_urls);
    if !external_env.is_empty() {
        let names: Vec<&str> = spec.externals.iter().map(|e| e.name.as_str()).collect();
        report::info(&format!(
            "Externals wired ({} var(s) for: {})",
            external_env.len(),
            names.join(", ")
        ));
    }

    // Setup / migration hooks: run declared shell commands (with the same env
    // wiring the app gets) after infra is healthy and before the app starts, so
    // DB migrations / fixtures are in place. A failing hook aborts the run.
    if !spec.setup.is_empty() {
        report::info("");
        report::info(&format!("Running {} setup hook(s)…", spec.setup.len()));
        if let Err(e) = run_setup_hooks(&spec.setup, dir, spec.app.as_ref(), framework, &handles.endpoints, &external_env, &env) {
            report::error_line(&format!("setup hook failed: {e}"));
            for m in &mut mocks {
                m.stop();
            }
            return 1;
        }
    }

    let app_port = if needs_app {
        report::info("");
        report::info("Starting app…");
        match app::start_app(spec.app.as_ref(), framework, Path::new(dir), &handles.endpoints, &external_env, &env, out_dir) {
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

    // Runtime wiring the *checks* can interpolate. HTTP checks and relative
    // paths resolve against the app's port automatically, but a target given in
    // full — `grpc.target`, an absolute `websocket.url`, an external trace
    // backend URL — had no way to name the app's ephemeral port, so specs had
    // to pin a fixed port through `app.env`. These make it addressable:
    // `${NOWORRIES_APP_PORT}`, `${NOWORRIES_APP_URL}`, the framework's own port
    // var (`${PORT}` / `${SERVER_PORT}`), and `${NOWORRIES_<SERVICE>_PORT}` for
    // each container. Existing entries win, so `app.env` / `.noworries.env`
    // still override.
    let (owned, fallback) = app::check_runtime_vars(
        app_port,
        &handles.endpoints,
        &app::port_env_name(spec.app.as_ref(), framework),
    );
    for (k, v) in fallback {
        env.entry(k).or_insert(v);
    }
    env.extend(owned);

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

    // Named identities (`users:`) are resolved the same way as the default one,
    // once, before the checks run — so a role's login/OIDC round-trip happens
    // once rather than per check. A failure here is fatal for the same reason
    // the default identity's is: every check that runs `as:` that user would
    // otherwise silently fall back to the wrong credentials.
    let mut identities = BTreeMap::new();
    for (name, spec) in &spec.users {
        match runner::resolve_auth(Some(spec), app_port, &env) {
            Ok(resolved) => {
                identities.insert(name.clone(), resolved);
            }
            Err(e) => {
                report::error_line(&format!("auth setup failed for user \"{name}\": {e}"));
                return 1;
            }
        }
    }
    if !identities.is_empty() {
        report::ok(&format!(
            "resolved {} identit{} ({})",
            identities.len(),
            if identities.len() == 1 { "y" } else { "ies" },
            identities.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }

    report::info("");
    report::info(&format!("Running {} check(s)…", selected.len()));
    let external_mocks = mocks
        .iter()
        .map(|m| (m.name.clone(), m.recorder()))
        .collect();
    let ctx = runner::RunnerContext {
        app_port,
        endpoints: handles.endpoints.clone(),
        env,
        auth_headers: auth.headers,
        auth_query: auth.query,
        identities,
        app_log_path: started_app.as_ref().map(|a| Path::new(&a.log_path).to_path_buf()),
        external_mocks,
        dir: Path::new(dir).to_path_buf(),
        update_snapshots: cli.update_snapshots,
        http_capture: std::sync::Mutex::new(None),
        compose_project: Some(handles.project_name.clone()),
        compose_file: Some(handles.compose_file.clone()),
    };
    let results = runner::run_checks(selected, &ctx);
    let summary = report::print_results(results);

    // Mocks have served their purpose; stop them before teardown.
    for m in &mut mocks {
        m.stop();
    }

    let results_path = out_dir.join("results.json");
    if let Err(e) = report::write_results_json(&results_path, &summary) {
        report::warn(&format!("could not write results.json: {e}"));
    } else {
        report::info(&format!("(machine-readable results: {}/results.json)", OUT_DIR));
    }
    if let Some(path) = &cli.junit {
        match std::fs::write(path, reports::junit_xml(&summary)) {
            Ok(()) => report::info(&format!("(JUnit report: {path})")),
            Err(e) => report::warn(&format!("could not write JUnit report: {e}")),
        }
    }
    if let Some(path) = &cli.html {
        match std::fs::write(path, reports::html_report(&summary)) {
            Ok(()) => report::info(&format!("(HTML report: {path})")),
            Err(e) => report::warn(&format!("could not write HTML report: {e}")),
        }
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
        Some(Command::Init { with }) => cmd_init(&cli.dir, with),
        Some(Command::Validate) => cmd_validate(&cli),
        Some(Command::Spec { format }) => cmd_spec(format),
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
