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
| `mysql`             | `mysql:8.4`               |
| `mysql:8.0`         | `mysql:8.0`               |
| `mongodb` / `mongo` | `mongo:7`                 |

Note that `kafka:<tag>` expands to the **`apache/kafka`** repository (Kafka's
official image) and `elastic`/`elasticsearch` expand to the official
**`docker.elastic.co/elasticsearch/elasticsearch`** image — not the bare names.

## `app` (optional)

How to launch the app under test. Omit it entirely and `noworries` auto-detects
the framework from the files in your project root:

| Framework      | Adapter name  | Detected by | Default start | Health | Port env |
| -------------- | ------------- | ----------- | ------------- | ------ | -------- |
| Spring Boot    | `spring-boot` | `pom.xml` / `build.gradle(.kts)` / `mvnw` / `gradlew` | `./mvnw spring-boot:run` (or `gradle`/`mvn` equivalent) | `/actuator/health` | `SERVER_PORT` |
| Go             | `go`          | `go.mod` | `go run .` | TCP (`none`) | `PORT` |
| FastAPI (Python) | `fastapi`   | `main.py` / `app.py`, or a `pyproject.toml`/`requirements.txt` mentioning fastapi/uvicorn | `uvicorn main:app --host 0.0.0.0 --port ${PORT}` | TCP (`none`) | `PORT` |
| Node.js        | `node`        | `package.json` | `npm start` (or `node server.js`/`index.js`/`app.js`) | TCP (`none`) | `PORT` |

Detection runs in that order, so a Spring Boot project that also ships a
`package.json` (e.g. a JS frontend) still resolves to `spring-boot`. Set
`app.framework` to force one explicitly.

| Field           | Default                              | Meaning |
| --------------- | ------------------------------------ | ------- |
| `start`         | auto-detected from the framework     | Shell command to start the app. |
| `health`        | framework default (`/actuator/health` for Spring; `none` for Go/FastAPI/Node) | Path polled until it returns 2xx before checks run. Set to `none` to skip HTTP and just wait for the port to accept TCP. |
| `framework`     | auto-detect                          | Force a specific framework adapter by name (`spring-boot`, `go`, `fastapi`, `node`). |
| `port_env`      | framework default (`SERVER_PORT` for Spring; `PORT` for Go/FastAPI/Node) | Env var the app reads for its HTTP port. |
| `ready_timeout` | `120`                                | Seconds to wait for the app to become ready. |
| `env`           | none                                 | Extra environment variables passed to the app process. |

The app is started with environment variables wired to the resolved container
ports — see [how it works](how-it-works.md#environment-wiring).

## `flink` (optional)

Use `flink` **instead of `app`** when the thing under test is an Apache Flink
job rather than an HTTP server. noworries stands up an ephemeral Flink **session
cluster** (one JobManager + one TaskManager) on the same isolated network as your
declared `services`, builds and submits your job(s) over the JobManager REST API,
waits until each reaches `RUNNING`, then runs the `checks`. The whole cluster is
destroyed by teardown, so jobs need no explicit cancellation.

```yaml
version: 1
services: [kafka, postgres, elastic]

flink:
  image: flink:1.19        # optional (default flink:1.19)
  taskmanagers: 1          # optional replica count (default 1)
  slots: 2                 # optional task slots per TaskManager (default 2)
  submit_timeout: 120      # optional seconds to wait for REST + each job RUNNING
  jobs:
    - name: "enrich"                       # optional label
      build: "mvn -q -DskipTests package"  # optional; runs in the project dir first
      jar: target/pipeline-0.1.jar         # path to the job jar (required)
      entry_class: com.acme.Pipeline       # optional (--class)
      args: ["--source", "events-in"]      # optional program args
      parallelism: 2                        # optional

checks:
  # Trigger the pipeline, then observe each hop with the normal retry loop:
  - name: "event flows kafka -> postgres -> topic -> ES"
    kafka: { produce: { topic: "events-in", key: "E1", message: { id: "E1", amount: 10 } } }
    db:    { query: "SELECT status FROM processed WHERE id='E1'", expect_row: { status: "OK" } }
  - name: "enriched event indexed"
    kafka:   { expect_message: { topic: "events-enriched", contains: { id: "E1" }, timeout_ms: 15000 } }
    elastic: { index: "events", doc_id: "E1", expect_source_contains: { id: "E1" } }
```

| Field           | Default      | Meaning |
| --------------- | ------------ | ------- |
| `image`         | `flink:1.19` | Flink image for the cluster. |
| `taskmanagers`  | `1`          | Number of TaskManager replicas. |
| `slots`         | `2`          | Task slots per TaskManager (caps job parallelism). |
| `submit_timeout`| `120`        | Seconds to wait for the REST API and for each job to reach `RUNNING`. |
| `jobs`          | —            | Non-empty, ordered list of jobs to build + submit. |

Each job: `jar` (required, relative to the project dir), optional `build` (shell
command run before submission), `entry_class`, `args`, `parallelism`, `name`.

**Networking.** The job runs *inside* the cluster, so it must reach the declared
services by their **in-network** names and container ports, not the random host
ports the host-side app model uses:

| Service | In-network address | Also exported to the job as |
| ------- | ------------------ | --------------------------- |
| kafka   | `kafka:9092`       | `NOWORRIES_KAFKA_HOST`/`_PORT` |
| postgres| `postgres:5432`    | `NOWORRIES_POSTGRES_HOST`/`_PORT` |
| mysql   | `mysql:3306`       | `NOWORRIES_MYSQL_HOST`/`_PORT` |
| mongodb | `mongodb:27017`    | `NOWORRIES_MONGODB_HOST`/`_PORT` |
| redis   | `redis:6379`       | `NOWORRIES_REDIS_HOST`/`_PORT` |
| elastic | `elastic:9200`     | `NOWORRIES_ELASTIC_HOST`/`_PORT` |

Configure your job to read those hostnames (either hard-coded, or from the
injected `NOWORRIES_*` env vars). First run is slow — the `flink` image (~600MB)
plus your services pull; use `--timeout 600`.

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
| `login`   | Runs the login request, extracts `token_from` (JSON path), and adds `<scheme> <token>` to `header`. The login `path` hits the app under test when relative (`/auth/login`), or a **separate auth server** when it's an absolute URL (`https://auth.example.com/oauth/token`, or `${AUTH_URL}/token`). |
| `bearer`  | Adds `<scheme> <token>` to `header` (defaults: `Bearer`, `Authorization`). |
| `basic`   | Adds `Authorization: Basic base64(user:pass)`. |
| `api_key` | Adds the key as a header and/or a query parameter (defaults to header `X-API-Key`). |

`${VAR}` interpolation also works inside a check's `request` headers, path, and
body — e.g. `Authorization: "Bearer ${API_TOKEN}"` on a single check.

### Where `${VAR}` values come from

At run time noworries resolves `${VAR}` from, in increasing precedence:

1. `app.env` in `noworries.yml` (non-secret defaults),
2. a **`.noworries.env`** file in the project root (dotenv format, `KEY=VALUE`,
   `#` comments) — **gitignored by default**, so this is where secrets like
   tokens/passwords go,
3. the **process environment** (a shell `export` or a CI secret) — overrides the
   file, so CI can inject real credentials.

```dotenv
# .noworries.env  (never committed)
TEST_USER=alice
TEST_PASS=s3cret
API_TOKEN=eyJhbGciOi...
```

If you run `noworries` **interactively** and the spec references a `${VAR}` that
isn't set anywhere yet, it prompts you for the value and appends it to
`.noworries.env` (so it persists and stays out of git). In non-interactive runs
(CI, piped stdin) it doesn't prompt — an unset variable is left literal and a
warning is printed.

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
| `mysql`   | MySQL: `seed` SQL (run before the request), `query` + `expect_row`/`expect_row_count`. |
| `mongodb` | MongoDB: `database`, `collection`, `seed` (insert/update/delete), `find` + `expect_doc_contains`/`expect_count`. |
| `scenario`| Edge-case load/timing trigger (burst / concurrent / duplicates / out_of_order) — see below. |

### Edge-case scenarios

A single trigger→observe check proves the happy path. A **data pipeline** (Flink,
a Kafka consumer, any read-then-write flow) can pass that and still drop messages
under load, corrupt state under concurrent writes, double-process duplicates, or
mishandle out-of-order arrival. A check's `scenario:` block replaces the single
`kafka.produce` with a generated flood designed to expose exactly those bugs; the
check's ordinary observe assertions (usually an exact **count**) verify the
invariant.

```yaml
checks:
  - name: "burst of 500 orders: nothing dropped"
    scenario:
      kind: burst            # burst | concurrent | duplicates | out_of_order
      count: 500             # logical messages (default per-kind)
      concurrency: 8         # parallel producer threads (the source of races)
      keys: 500              # distinct keys the load spreads over
      rate_per_sec: 2000     # optional per-producer cap (0/absent = as fast as possible)
      kafka:
        topic: orders-in
        key: "ord-${seq}"                     # template
        message: { id: "${uuid}", amount: "${seq}" }
    db: { query: "SELECT count(*) n FROM orders", expect_row: { n: 500 } }
```

| `kind`         | Behaviour | Catches | Typical verification |
| -------------- | --------- | ------- | -------------------- |
| `burst`        | Floods `count` messages (optionally rate-limited) over `keys` keys. | Dropped messages, can't keep up. | `expect_row_count`/`n = count`. |
| `concurrent`   | `count` writes over a few `keys` across `concurrency` parallel producers → real races; `${i}` is the per-key version. | Lost updates, duplicate rows, stale final value. | one row per key + final value = highest `${i}`. |
| `duplicates`   | Sends every message **twice**. | Non-idempotent / double-processing. | `n = count` (not `2·count`). |
| `out_of_order` | Emits each key's events in **reverse** arrival order; true order rides in `${i}`. | Naive last-arrival-wins, bad windowing. | final value = highest `${i}`. |

Template placeholders (in `key` and `message`): `${seq}` (global 0-based index),
`${i}` (per-key sequence / version), `${key}` (assigned key), `${uuid}` (unique
id). Scenarios currently target Kafka — the layer is an extensible strategy
(`EdgeCase`) so more sinks and kinds can be added without changing the spec. Big
bursts take longer to drain, so raise `--timeout` accordingly.

### Tagging & scope

Tag checks by feature so runs stay fast while iterating:

```bash
noworries --tags orders        # only checks tagged "orders"
noworries                      # all checks (also what /noworries force uses)
```
