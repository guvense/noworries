<div align="center">

# noworries

**Let an AI verify the changes it just made — before you find out the hard way.**

`noworries` stands up the real infrastructure a feature needs (Postgres, Kafka,
Redis) in throwaway Docker containers, starts your app against them, exercises
the new behaviour, and reports **READY** or **NOT READY**. Paired with the
`/noworries` command in Claude Code, the AI reads the result and fixes its own
code until it's green.

</div>

---

## The problem

You ask an AI agent to build a feature. It writes the code. It *looks* right.
But does it actually work end-to-end — does the endpoint return 201, does the
row land in the database, does the Kafka consumer fire, does the value get
cached in Redis? Usually you find out later, by hand, or in production.

`noworries` closes that gap. It's a small CLI that turns "looks right" into a
concrete **READY / NOT READY** you (and the AI) can act on.

## How it works

```
/noworries
   │
   ├─ 1. read what changed            (git diff of the AI's edits)
   ├─ 2. write/update noworries.yml   (services + checks for the feature)
   ├─ 3. spin up infra in Docker      (Postgres / Kafka / Redis, ephemeral)
   ├─ 4. start the app against it     (env wired to the containers)
   ├─ 5. run the checks               (HTTP + DB + Kafka + Redis assertions)
   ├─ 6. report READY / NOT READY     (+ machine-readable results.json)
   └─ 7. tear everything down
           │
           └─ NOT READY → the AI fixes the code and runs again, until green.
```

Everything is ephemeral: a fresh isolated Docker network per run, guaranteed
`docker compose down -v` teardown on success, failure, or Ctrl-C.

## Example

A single check can span services — HTTP status, a database row, and a Redis
cache entry, all at once:

```
=== noworries results ===
PASS  create order persists (pg) and caches (redis)
      ✓ [http]  POST /orders -> 201
      ✓ [db]    first row matches expected fields
      ✓ [redis] key "cache:order:ABC123" exists = true
      ✓ [redis] key "cache:order:ABC123" value contains expected fields

Summary: 1 passed, 0 failed (1 total)
Result: READY
=========================
```

When something regresses, you get the exact diff:

```
FAIL  create order returns 201 and persists as PENDING
      ✗ [http] POST /orders: expected status 201, got 500
          expected: 201
          actual:   500
      ✗ [db]   expected a matching row, but query returned no rows
Result: NOT READY
```

## Installation

Every method installs the `noworries` binary. The `/noworries` slash command is
embedded in the binary — install it once with `noworries install-command`
(globally into `~/.claude/commands`, or `--project` for a single repo).

<details open>
<summary><b>curl | sh</b> (no toolchain required)</summary>

```bash
curl -fsSL https://raw.githubusercontent.com/guvense/noworries/main/install.sh | sh
noworries install-command
```

Detects your OS/arch, downloads the prebuilt binary from GitHub Releases, and
verifies the checksum.
</details>

<details>
<summary><b>Homebrew</b> (macOS / Linux)</summary>

```bash
brew install guvense/noworries/noworries
noworries install-command
```
</details>

<details>
<summary><b>npm</b></summary>

```bash
npm install -g @guvenseckin4/noworries   # postinstall downloads the prebuilt binary
noworries install-command                # the command is still `noworries`
```
</details>

<details>
<summary><b>cargo</b> (build from source)</summary>

```bash
cargo install --git https://github.com/guvense/noworries
noworries install-command
```
</details>

### Requirements

- **Docker** running (Docker Desktop on macOS).
- Your app's own toolchain (e.g. **JDK + Maven** for Spring Boot).
- For `/noworries`: **Claude Code**.

## Quick start

```bash
cd your-spring-boot-project
noworries init          # scaffold a starter noworries.yml
# edit noworries.yml to declare services + checks, then:
noworries               # confirm services, bring infra up, run checks, tear down
```

Or, inside Claude Code, just make a change and run **`/noworries`** — the AI
writes the checks for you, runs them, and fixes the code if they fail. Use
**`/noworries force`** for a full regression pass over everything that changed.

## `noworries.yml` at a glance

```yaml
version: 1
services:
  - postgres:16-alpine        # also: kafka, redis, elastic
app:                          # optional; auto-detected from the framework
  start: "./mvnw spring-boot:run"
  health: "/actuator/health"
checks:
  - name: "create order returns 201 and persists"
    tags: [orders]
    request: { method: POST, path: /orders, body: { sku: "ABC123", qty: 2 } }
    expect:  { status: 201 }
    db:
      query: "SELECT status FROM orders WHERE sku = 'ABC123'"
      expect_row: { status: "PENDING" }
```

Full reference: **[docs/configuration.md](docs/configuration.md)** ·
check types: **[docs/checks.md](docs/checks.md)**.

## Supported services & frameworks

| Services       | Frameworks                        |
| -------------- | --------------------------------- |
| Postgres       | Spring Boot (auto-detected)       |
| MySQL          | *(architecture is framework-agnostic —* |
| MongoDB        | *adding Go etc. is one adapter)*  |
| Kafka          |                                   |
| Redis          |                                   |
| Elasticsearch  | *(ES 7 & 8)*                      |

Adding a new service or framework is a single trait implementation — see
**[docs/architecture.md](docs/architecture.md)**.

## Documentation

- **[Getting started](docs/getting-started.md)** — install, first run, the loop.
- **[Configuration](docs/configuration.md)** — the full `noworries.yml` schema.
- **[Checks](docs/checks.md)** — HTTP, DB, Kafka, and Redis assertions.
- **[CLI reference](docs/cli.md)** — commands and flags.
- **[How it works](docs/how-it-works.md)** — lifecycle, env wiring, teardown.
- **[Architecture](docs/architecture.md)** — traits, and adding services/frameworks.
- **[Troubleshooting](docs/troubleshooting.md)** — common issues.
- **[Releasing](docs/releasing.md)** — the automated release pipeline (maintainers).

## Status

`noworries` is early (v0.1). Postgres and Redis paths are verified end-to-end;
Kafka is implemented and validated for compose + message-matching (test the
live produce/consume path in your environment). Feedback and issues welcome.

## License

MIT
