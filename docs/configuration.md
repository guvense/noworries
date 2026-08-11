# Configuration — `noworries.yml`

`noworries.yml` lives at your project root. It declares **what infrastructure a
feature needs** and **what "correct" means** as a list of checks. It's meant to
be edited by both humans and AI.

```yaml
version: 1

services:
  - postgres:16-alpine
  - kafka
  - redis:7-alpine

app:
  start: "./mvnw spring-boot:run"
  health: "/actuator/health"
  ready_timeout: 180

checks:
  - name: "create order returns 201 and persists"
    tags: [orders]
    request: { method: POST, path: /orders, body: { sku: "ABC123", qty: 2 } }
    expect:  { status: 201 }
    db:
      query: "SELECT status FROM orders WHERE sku = 'ABC123'"
      expect_row: { status: "PENDING" }
```

## `version`

Required. Currently must be `1`.

## `services`

Required, non-empty list. Each entry is `kind` or `kind:tag`. Supported kinds:

| Entry               | Image used                |
| ------------------- | ------------------------- |
| `postgres`          | `postgres:16-alpine`      |
| `postgres:16`       | `postgres:16`             |
| `kafka`             | `apache/kafka:3.7.0`      |
| `kafka:3.7`         | `apache/kafka:3.7`        |
| `redis`             | `redis:7-alpine`          |
| `redis:7-alpine`    | `redis:7-alpine`          |

Note that `kafka:<tag>` expands to the **`apache/kafka`** repository (Kafka's
official image), not `kafka`.

## `app` (optional)

How to launch the app under test. Omit it entirely and `noworries` auto-detects
the framework (currently Spring Boot via `mvnw`/`gradlew`/`pom.xml`/
`build.gradle`).

| Field           | Default                              | Meaning |
| --------------- | ------------------------------------ | ------- |
| `start`         | auto-detected from the framework     | Shell command to start the app. |
| `health`        | framework default (`/actuator/health`) | Path polled until it returns 2xx before checks run. Set to `none` to skip HTTP and just wait for the port to accept TCP. |
| `framework`     | auto-detect                          | Force a specific framework adapter by name (e.g. `spring-boot`). |
| `port_env`      | `SERVER_PORT`                        | Env var the app reads for its HTTP port. |
| `ready_timeout` | `120`                                | Seconds to wait for the app to become ready. |
| `env`           | none                                 | Extra environment variables passed to the app process. |

The app is started with environment variables wired to the resolved container
ports — see [how it works](how-it-works.md#environment-wiring).

## `checks`

A list of named checks. Each check may combine HTTP, DB, Kafka, and Redis
assertions; a check passes only if **all** its assertions pass. Full details and
examples in [checks](checks.md). Shape:

| Field     | Meaning |
| --------- | ------- |
| `name`    | Required, unique, human-readable. |
| `tags`    | Optional list; lets a run target a subset (`--tags`). |
| `request` | HTTP request to send. |
| `expect`  | HTTP expectations (`status`, `body_contains`). |
| `db`      | Postgres assertion (`query`, `expect_row`, `expect_row_count`). |
| `kafka`   | Kafka `produce` and/or `expect_message`. |
| `redis`   | Redis `key` with `expect_exists` / `expect_value` / `expect_value_contains`. |

### Tagging & scope

Tag checks by feature so runs stay fast while iterating:

```bash
noworries --tags orders        # only checks tagged "orders"
noworries                      # all checks (also what /noworries force uses)
```
