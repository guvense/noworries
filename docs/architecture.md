# Architecture

`noworries` is a small Rust CLI built around a few extension points, so adding a
new framework, service, or edge-case scenario never touches the core.

```
src/
  main.rs            CLI (clap) + run orchestration + teardown safety net
  spec.rs            noworries.yml types + parse/validate
  framework/
    mod.rs           Framework trait + registry + shared helpers (interface only)
    spring_boot.rs   Spring Boot adapter          ← add a framework: new file here
    go.rs / node.rs / python.rs   Go, Node.js, FastAPI adapters
  services/
    mod.rs           ServiceProvider trait + registry + shared types (interface only)
    postgres.rs      Postgres provider            ← add a service: new file here
    kafka.rs / redis.rs / elastic.rs / mysql.rs / mongodb.rs
  edgecases/
    mod.rs           EdgeCase trait + registry + templating (interface only)
    burst.rs / concurrent.rs / duplicates.rs / out_of_order.rs  ← add a scenario here
  flink.rs           Flink session-cluster compose + REST job submission
  compose.rs         compose.test.yml generation
  lifecycle.rs       up / health / port-resolve / teardown (docker compose)
  app.rs             start the app subprocess, wire env, wait for health
  runner.rs          HTTP + DB + Kafka + Redis + scenario check execution
  docker.rs          docker CLI helpers
  git.rs             changed-files detection for /noworries
  report.rs          output + results.json
```

The `framework/`, `services/`, and `edgecases/` modules keep the **interface**
(`mod.rs`) apart from the **implementations** (one file each), so the extension
points grow without the core changing — new adapters/strategies are added, never
edited into an existing file (open/closed).

## The extension traits

### `Framework` (`src/framework.rs`)

Everything framework-specific — how to detect the project, start it, where its
health endpoint is, and how to translate resolved service endpoints into env
vars:

```rust
pub trait Framework {
    fn name(&self) -> &'static str;
    fn detect(&self, dir: &Path) -> bool;
    fn default_start_command(&self, dir: &Path) -> Option<String>;
    fn default_health_path(&self) -> &'static str;
    fn env_wiring(&self, endpoints: &[ServiceEndpoint]) -> BTreeMap<String, String>;
}
```

**Adding Go** (illustrative): create `src/framework/go.rs`, implement `Framework`
for a `Go` struct that `detect`s `go.mod`, returns `go run .` as the start
command, and maps endpoints to `DATABASE_URL` / `KAFKA_BROKERS` / `REDIS_URL`.
Then declare `pub mod go;` and add one line to `framework::registry()` in
`framework/mod.rs`. Nothing else changes.

### `ServiceProvider` (`src/services.rs`)

Everything service-specific — the Compose fragment (image, healthcheck, ports)
and, for the runner, how to make/verify its assertions:

```rust
pub trait ServiceProvider {
    fn kind(&self) -> ServiceKind;
    fn needs_fixed_host_port(&self) -> bool { false }   // Kafka returns true
    fn compose_service(&self, decl: &ServiceDecl, assigned_host_port: Option<u16>)
        -> Result<ComposeService>;
}
```

**Adding a service** is a new file `src/services/<name>.rs` with the impl, plus
`pub mod <name>;` and one arm in `services::provider_for()` in `services/mod.rs`.
Services that must know their host port up front (like Kafka's advertised
listener) return `true` from `needs_fixed_host_port()`, and compose generation
reserves a free port for them instead of letting Docker randomize it.

### `EdgeCase` (`src/edgecases/`)

Everything scenario-specific — how a `kind` (burst / concurrent / duplicates /
out_of_order) expands into a concrete load plan the runner then produces to Kafka
(optionally across parallel producers, to create real races):

```rust
pub trait EdgeCase {
    fn kind(&self) -> &'static str;
    fn describe(&self) -> &'static str;
    fn plan(&self, s: &ScenarioSpec) -> Result<ScenarioPlan>;
}
```

**Adding a scenario** is a new file `src/edgecases/<kind>.rs` implementing
`EdgeCase`, plus `pub mod <kind>;` and one line in `edgecases::registry()`. The
strategy only builds the plan (pure, unit-testable); the runner owns execution
(threads, producers, rate limiting) and the check's observe assertions verify the
invariant. Kafka is the first sink; the plan/action shape leaves room for others.

### Flink (`src/flink.rs`)

Not a trait but a composed capability: it generates the session-cluster compose
fragment (jobmanager + taskmanager, wired to in-network service coordinates) and
drives the JobManager REST API to upload, run, and await each job. It slots into
the existing `up → run → down` lifecycle via the compose file's `aux` containers.

## Design choices

- **Plain `docker compose` CLI**, not embedded Testcontainers — so the harness
  can be invoked standalone from a slash command, not only from a test run.
- **Ephemeral & isolated** — a per-run project name and bridge network,
  `down -v` teardown guaranteed via a signal handler + a hard run timeout.
- **Structured output** — a human-readable block plus `results.json`, so an AI
  can parse the result and act on it deterministically.
- **The CLI is deterministic; the intelligence is in the prompt** — the tool
  reports *which files changed* and *what passed/failed*; the `/noworries`
  command lets Claude decide *what to verify* and *how to fix*.

## Testing

Client-library integration bugs (a wrong API call against Kafka, Elasticsearch,
Postgres, …) only surface at runtime against a **real** service — compile checks
and pure-logic unit tests can't catch them. So every external dependency has an
integration test that actually talks to it, and CI runs them against real
services on every push.

- `cargo test --lib` — unit tests (subset matching, interpolation, base64, JSON
  path, auth resolution, image expansion, …).
- `tests/lifecycle_it.rs` — real-Docker container lifecycle.
- `tests/runner_it.rs` — app start + HTTP/DB runner against a real Postgres.
- `tests/redis_it.rs` — HTTP + DB + Redis against real Postgres + Redis.
- `tests/auth_it.rs` — the login → token → protected-request flow over real HTTP.
- `tests/kafka_it.rs` — **produce + consume against a real broker** (this is the
  path where the consumer "offset storage" bug lived).
- `tests/elastic_it.rs` — template + insert/update/delete + doc/search against a
  real Elasticsearch.

Integration tests self-skip unless their `NOWORRIES_IT_*` env vars are set.
**CI** (`.github/workflows/ci.yml`) brings up real Postgres / Redis / Kafka /
Elasticsearch via `ci/docker-compose.yml`, sets those vars, and runs the suite
plus `clippy -D warnings` on every push and PR — so a regression in how the CLI
uses a client library fails the build.
