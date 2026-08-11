# noworries (Rust)

An ephemeral infrastructure harness that lets an AI **verify the changes it just
made**. You ask Claude to build something; you don't know end-to-end whether it
did it right. You run `/noworries`; based on the feature Claude just added, it
stands up exactly the infra that feature needs (Postgres, Kafka, Redis) in
throwaway Docker containers, starts the app against them, exercises the new
behaviour (HTTP calls, produce a Kafka message, check a Redis key, assert the
DB), reports **READY / NOT READY**, and tears everything down. If it's NOT READY,
Claude reads the structured report, fixes the code, and runs again.

This is the Rust rewrite, designed to be **framework-agnostic** from the start:
Spring Boot today, Go (or anything else) later, by adding one adapter.

## Status — phased build

**Phase 1 (done):** framework-agnostic architecture (`Framework` +
`ServiceProvider` traits); `noworries.yml` parsing/validation (services, `app`,
and HTTP/DB/**Kafka**/**Redis** check shapes); Postgres compose generation;
container lifecycle (preflight, `up -d`, health polling, dynamic host-port
resolution, guaranteed `down -v` on success/failure/Ctrl-C); `noworries changed`
/ `changed --all` git-diff scope.

**Phase 2 (done):**

- App start: launches the app (auto-detected from the framework, or an
  `app.start` override) with env wired to the resolved container ports, waits
  for its health endpoint, and stops the whole process group on teardown.
- Check runner: HTTP assertions (status + `body_contains`) and Postgres DB
  assertions (`expect_row` / `expect_row_count`) with deep subset matching.
- Structured report: a `Result: READY / NOT READY` block, `.noworries/
  results.json`, and an exit code (`0` ready, `1` not) that Claude reads to
  fix-and-rerun.

**Phase 3 (done — Kafka):**

- Kafka `ServiceProvider`: single-node KRaft (no ZooKeeper) compose, with the
  external listener advertised at `localhost:<host port>`. Because a Kafka
  client needs the advertised address up front, Kafka's host port is fixed at
  generation time (a generalised "fixed host port" mechanism in the provider
  interface) instead of the random port Postgres uses.
- Kafka checks: `kafka.produce` sends a message to a topic (to trigger a
  consumer the feature added — the `db` assertion in the same check verifies
  the effect, and now **retries for a few seconds** to allow for async
  processing); `kafka.expect_message` consumes a topic and asserts a matching
  message arrived (to verify a producer the feature added).

> Kafka was verified for compose validity (`docker compose config`), build, and
> the message-matching logic, but **not** against a live broker in this repo's
> sandbox (public image registries are blocked there). Test the produce/consume
> path against a real Kafka on your machine.

**Phase 4 (done — Redis):**

- Redis `ServiceProvider`: `redis:7-alpine` compose with a `redis-cli ping`
  healthcheck (random host port).
- Redis checks: `redis.key` with `expect_exists`, `expect_value` (exact), and
  `expect_value_contains` (JSON subset) — for verifying a feature that caches to
  Redis ("write to cache, then check the cache is there"). Retries briefly for
  async cache writes.
- Verified end-to-end against a real native Redis + Postgres + stub app
  (READY when the cache is written, NOT READY when it isn't).

A single check can span services — HTTP status + DB row + Redis cache all in one
`Result: READY`.

**Phase 5 (done — the loop):** the `/noworries` slash command
(`.claude/commands/noworries.md`) that reads what changed (`noworries changed` /
`changed --all` for `force`), auto-generates/updates `noworries.yml` for the
feature, runs the harness, reads `.noworries/results.json`, and — if NOT READY —
fixes the code and re-runs until READY. This is the whole point: **the AI
verifies its own changes and self-corrects.**

All planned functionality for v1 is in place. Adding a new framework (Go) or
service is still just one trait impl + one registry line.

## Architecture (why it's flexible)

Two traits are the extension points:

- `Framework` (`src/framework.rs`) — `detect()`, `default_start_command()`,
  `default_health_path()`, `env_wiring()`. Spring Boot maps resolved container
  ports to `SPRING_DATASOURCE_URL`, `SPRING_KAFKA_BOOTSTRAP_SERVERS`, etc.
  **Adding Go** = a new `impl Framework` that detects `go.mod`, returns
  `go run .`, and wires `DATABASE_URL` / `KAFKA_BROKERS` — one line in
  `registry()`.
- `ServiceProvider` (`src/services.rs`) — each service describes its own compose
  fragment (image, healthcheck, port) and, in later phases, runs its own
  assertions. **Adding Kafka/Redis** = a new `impl ServiceProvider` + one line
  in `provider_for()`.

Everything framework/service-specific lives behind these traits; the CLI,
compose generation, and lifecycle never hard-code Spring Boot or Postgres.

## Build & run

```bash
cargo build --release          # binary at target/release/noworries
# or install onto PATH:
cargo install --path .         # then just `noworries`

noworries init                 # scaffold noworries.yml
noworries                      # detect services, confirm, bring infra up, run checks, tear down
noworries --yes                # skip the confirmation prompt
noworries --tags orders        # only checks tagged "orders"
noworries --keep-alive         # leave containers up for debugging
noworries changed              # files changed vs HEAD (for /noworries)
noworries changed --all        # all tracked files (the "force" scope)
```

Requires Docker running. (In this repo's build sandbox, public image registries
are blocked, so the Postgres/Redis happy-paths were verified with a native
Postgres/Redis + stub app; on a normal machine it pulls the real images.)

## Installing (for end users)

Every channel installs the `noworries` binary; the `/noworries` slash command is
embedded in the binary and installed with `noworries install-command` (globally
into `~/.claude/commands`, or `--project` for one repo).

- **curl | sh (no Rust needed):**
  ```bash
  curl -fsSL https://raw.githubusercontent.com/guvense/noworries/main/install.sh | sh
  ```
  Detects OS/arch, downloads the prebuilt binary from GitHub Releases, and runs
  `install-command`.

- **Homebrew (macOS):**
  ```bash
  brew install guvense/noworries/noworries
  noworries install-command
  ```
  (Formula in `dist/homebrew/noworries.rb`; publish it in a `homebrew-noworries`
  tap repo.)

- **npm:**
  ```bash
  npm install -g noworries      # postinstall downloads the prebuilt binary
  noworries install-command
  ```
  (Package in `dist/npm/`.)

- **cargo (Rust devs):**
  ```bash
  cargo install --git https://github.com/guvense/noworries
  noworries install-command
  ```

## Releasing (for maintainers)

Releases are automated on merge to `main`. The whole flow is driven by the
`version` in `Cargo.toml` (single source of truth):

1. In your PR, bump `version` in `Cargo.toml`.
2. Merge to `main`. `.github/workflows/release.yml` then, **only if that version
   has no `v<version>` tag yet**:
   - builds macOS arm64/x64 + Linux x64 binaries and creates the GitHub Release
     (`noworries-<target>.tar.gz` + `.sha256`),
   - publishes to **npm** (syncing `dist/npm/package.json` to the version),
   - updates the **Homebrew tap** (`guvense/homebrew-noworries`, `Formula/
     noworries.rb`) with the new version + checksums.

   Pushing commits to `main` without bumping the version does nothing (the tag
   already exists) — so ordinary merges don't cut releases.

One-time setup:

- Repo secrets: `NPM_TOKEN` (npm automation token) and `HOMEBREW_TAP_TOKEN` (a
  PAT with `repo` scope on the tap).
- Create the tap repo `guvense/homebrew-noworries` (empty is fine; the workflow
  writes `Formula/noworries.rb` into it).
- If the npm name `noworries` is taken, switch to a scoped name
  (`@guvense/noworries`) in `dist/npm/package.json`.

You can also trigger a release manually from the Actions tab
(`workflow_dispatch`).

## Using it as `/noworries` in Claude Code

Copy `.claude/commands/noworries.md` into your project's `.claude/commands/`
(and make sure `noworries` is on PATH, e.g. `cargo install --path .`). Then,
after Claude makes a change, run `/noworries` (or `/noworries force` for a full
regression pass). Claude reads what changed, writes/updates `noworries.yml` for
the feature, runs the harness, and — if it's NOT READY — fixes the code and
re-runs until it's green.

## `noworries.yml`

```yaml
version: 1
services:
  - postgres:16-alpine
app:                          # optional; auto-detected from the framework
  start: "./mvnw spring-boot:run"
  health: "/actuator/health"
checks:
  - name: "create order persists as PENDING"
    tags: [orders]
    request: { method: POST, path: /orders, body: { sku: "ABC123", qty: 2 } }
    expect:  { status: 201 }
    db:
      query: "SELECT status FROM orders WHERE sku = 'ABC123'"
      expect_row: { status: "PENDING" }

  # Kafka: produce a message to trigger a consumer, verify the DB side effect.
  # - name: "OrderCreated event is consumed and persisted"
  #   tags: [orders]
  #   kafka:
  #     produce: { topic: "orders", message: { type: "OrderCreated", sku: "ABC123", qty: 2 } }
  #   db:
  #     query: "SELECT status FROM orders WHERE sku = 'ABC123'"
  #     expect_row: { status: "PENDING" }
  #   # or verify a producer the feature added:
  #   # kafka:
  #   #   expect_message: { topic: "order-events", contains: { type: "OrderProcessed" } }

  # Redis: verify a feature caches to Redis.
  # - name: "order is cached after creation"
  #   request: { method: POST, path: /orders, body: { sku: "ABC123", qty: 2 } }
  #   expect: { status: 201 }
  #   redis:
  #     key: "cache:order:ABC123"
  #     expect_exists: true
  #     expect_value_contains: { status: "PENDING" }
```

## Layout

```
src/
  main.rs        CLI (clap) + run orchestration + teardown safety net
  spec.rs        noworries.yml types + parse/validate
  framework.rs   Framework trait + Spring Boot adapter  (add Go here)
  services.rs    ServiceProvider trait + Postgres        (add Kafka/Redis here)
  compose.rs     compose.test.yml generation
  lifecycle.rs   up / health / port-resolve / teardown
  docker.rs      docker CLI helpers
  git.rs         changed-files detection for /noworries
  report.rs      output
tests/
  lifecycle_it.rs   real-Docker lifecycle test
  runner_it.rs      app-start + HTTP/DB runner test (real Postgres)
  redis_it.rs       HTTP + DB + Redis runner test (real Postgres + Redis)
.claude/commands/
  noworries.md      the /noworries slash command (the loop)
```
