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
| `elastic:8.13.4`    | `docker.elastic.co/elasticsearch/elasticsearch:8.13.4` |
| `elasticsearch:7.17.22` | `docker.elastic.co/elasticsearch/elasticsearch:7.17.22` |

Note that `kafka:<tag>` expands to the **`apache/kafka`** repository (Kafka's
official image) and `elastic`/`elasticsearch` expand to the official
**`docker.elastic.co/elasticsearch/elasticsearch`** image — not the bare names.

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

## `auth` (optional)

Authentication applied to **every** check's HTTP request. Extensible — set the
sub-block(s) you need; a check's own `request.headers` override anything auth
sets. All string values support `${VAR}` interpolation (from `app.env` or the
process env), so tokens/passwords stay out of the repo.

```yaml
auth:
  # 1) Log in, extract a token, send it as a header (most common):
  login:
    request: { method: POST, path: /auth/login, body: { username: "${TEST_USER}", password: "${TEST_PASS}" } }
    expect: { status: 200 }
    token_from: "$.accessToken"     # JSON path in the login response
    header: Authorization            # optional (default Authorization)
    scheme: Bearer                   # optional (default Bearer; "" = raw token)

  # 2) A static bearer token:
  # bearer: { token: "${API_TOKEN}" }

  # 3) HTTP Basic auth:
  # basic: { username: "${USER}", password: "${PASS}" }

  # 4) API key as a header and/or query param:
  # api_key: { header: "X-API-Key", value: "${API_KEY}" }
  # api_key: { query: "api_key", value: "${API_KEY}" }
```

| Sub-block | Effect |
| --------- | ------ |
| `login`   | Runs the login request against the app, extracts `token_from` (JSON path), and adds `<scheme> <token>` to `header`. |
| `bearer`  | Adds `<scheme> <token>` to `header` (defaults: `Bearer`, `Authorization`). |
| `basic`   | Adds `Authorization: Basic base64(user:pass)`. |
| `api_key` | Adds the key as a header and/or a query parameter (defaults to header `X-API-Key`). |

`${VAR}` interpolation also works inside a check's `request` headers, path, and
body — e.g. `Authorization: "Bearer ${API_TOKEN}"` on a single check.

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
| `elastic` | Elasticsearch: `index`, optional `template`, `operations` (insert/update/delete), and `doc_id`/`expect_source_contains` or `query`/`expect_hits`. |

### Tagging & scope

Tag checks by feature so runs stay fast while iterating:

```bash
noworries --tags orders        # only checks tagged "orders"
noworries                      # all checks (also what /noworries force uses)
```
