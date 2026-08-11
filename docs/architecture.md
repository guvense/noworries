# Architecture

`noworries` is a small Rust CLI built around two extension points, so adding a
new framework or service never touches the core.

```
src/
  main.rs        CLI (clap) + run orchestration + teardown safety net
  spec.rs        noworries.yml types + parse/validate
  framework.rs   Framework trait + Spring Boot adapter      ← add frameworks here
  services.rs    ServiceProvider trait + Postgres/Kafka/Redis ← add services here
  compose.rs     compose.test.yml generation
  lifecycle.rs   up / health / port-resolve / teardown (docker compose)
  app.rs         start the app subprocess, wire env, wait for health
  runner.rs      HTTP + DB + Kafka + Redis check execution
  docker.rs      docker CLI helpers
  git.rs         changed-files detection for /noworries
  report.rs      output + results.json
```

## The two traits

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

**Adding Go** (illustrative): implement `Framework` for a `Go` struct that
`detect`s `go.mod`, returns `go run .` as the start command, and maps endpoints
to `DATABASE_URL` / `KAFKA_BROKERS` / `REDIS_URL`. Then add one line to
`framework::registry()`. Nothing else changes.

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

**Adding a service** is a new impl plus one arm in `services::provider_for()`.
Services that must know their host port up front (like Kafka's advertised
listener) return `true` from `needs_fixed_host_port()`, and compose generation
reserves a free port for them instead of letting Docker randomize it.

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

- `cargo test --lib` — unit tests (subset matching, image expansion, …).
- `tests/lifecycle_it.rs` — real-Docker container lifecycle (uses a local image).
- `tests/runner_it.rs` — app start + HTTP/DB runner against a real Postgres.
- `tests/redis_it.rs` — HTTP + DB + Redis against real Postgres + Redis.

Integration tests self-skip unless the relevant `NOWORRIES_IT_*` env vars are
set (they need real services / images available).
