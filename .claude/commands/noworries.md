---
description: Verify the change just made — auto-generate noworries.yml, stand up infra, run checks, fix and re-run until READY
argument-hint: "[force | init]"
allowed-tools: Bash(noworries:*), Bash(cargo:*), Bash(git:*), Bash(docker:*), Read, Edit, Write
---

# /noworries

> **Requires `noworries` >= 0.4.0** (`noworries --version`). Older builds lack
> the extensible check types (graphql/metrics/snapshot/schema/sse/websocket/grpc/
> traces), `externals`/`mock`, `setup` hooks, and interpolated `app.env`.

After code has been written, verify it actually works: stand up its
infrastructure in ephemeral Docker containers, start the app against them, run
the checks in `noworries.yml`, and read the results. If everything passes, tell
the user it's **READY**. If not, fix the code and run again — loop until green.

`noworries` writes `.noworries/results.json` and prints a
`=== noworries results ===` block ending in `Result: READY` / `NOT READY`.
Exit code: `0` READY, `1` NOT READY/error, `2` not confirmed.

## Modes

- `/noworries` — scope to what changed since HEAD (`noworries changed`).
- `/noworries force` — regression: everything (`noworries changed --all`), full suite.
- `/noworries init` — scaffold a starter `noworries.yml` (`noworries init`), then
  show it to the user and tailor its `checks` to the code.

## The loop

1. **See what changed** (`noworries changed` / `git diff`). Identify observable
   behaviour: HTTP endpoints/status/body, DB writes, Kafka consumers/producers,
   Redis caching, Elasticsearch indexing, MySQL/Mongo reads-then-writes. For
   **data pipelines / streaming / concurrent writes**, also plan edge-case checks
   (burst, races, duplicates, out-of-order) — see [Edge-case scenarios](#edge-case-scenarios-dont-just-test-the-happy-path).
2. **Write/update `noworries.yml`** (reference below). For a **new** feature or
   new checks, show the proposed checks to the user before the first run. For a
   pure re-run or a tag/scope change, just run — don't gate on the user.
3. **Run** `noworries --yes [--tags <tags>]` (confirm services on a project's
   very first run). `force` → no tag filter.
4. **Read** `.noworries/results.json` + the printed block. Each failed assertion
   has `expected` vs `actual`; check `.noworries/app.log` for app errors.
5. **Decide.** READY → tell the user, summarise what was verified. NOT READY →
   fix the **code** (not the check, unless the check was wrong) and go to 3.
   After ~2 failed attempts on the same check, stop and ask the user.

## Supported services

`postgres` · `mysql` · `mongodb` (`mongo`) · `redis` · `kafka` ·
`elastic` (`elasticsearch`). Declare as `kind` or `kind:tag`:

```yaml
services: [ postgres:16-alpine, mysql, mongodb, redis, kafka, elastic ]
```

Container credentials are `noworries`/`noworries`/`noworries` where applicable
(postgres, mysql). Mongo/redis/kafka/elastic run without auth.

## Frameworks

Auto-detected from the project root (override with `app.framework`): **Spring
Boot** (`pom.xml`/`gradle`/`mvnw`), **Go** (`go.mod`), **FastAPI** (`main.py`/
`app.py` or a fastapi/uvicorn manifest), **Node.js** (`package.json`). Detection
priority is Spring → Go → FastAPI → Node, so a JVM app that also ships a
`package.json` still detects as Spring. Spring probes `/actuator/health`; the
others wait for the TCP port (`health: none`) unless you set `app.health`.

## Auto-injected environment

The app is started with these set from the resolved random container ports — you
usually **don't need to touch config files**.

**Spring Boot:**

| Service   | Variables |
| --------- | --------- |
| postgres  | `SPRING_DATASOURCE_URL` (jdbc:postgresql), `SPRING_DATASOURCE_USERNAME`, `SPRING_DATASOURCE_PASSWORD` |
| mysql     | `SPRING_DATASOURCE_URL` (jdbc:mysql), `SPRING_DATASOURCE_USERNAME`, `SPRING_DATASOURCE_PASSWORD` |
| mongodb   | `SPRING_DATA_MONGODB_URI` |
| redis     | `SPRING_DATA_REDIS_HOST`, `SPRING_DATA_REDIS_PORT` |
| kafka     | `SPRING_KAFKA_BOOTSTRAP_SERVERS` |
| elastic   | `SPRING_ELASTICSEARCH_URIS` |

**Go / FastAPI / Node.js** (conventional connection strings — read whichever your
client expects):

| Service   | Variables |
| --------- | --------- |
| postgres  | `DATABASE_URL` (postgres://), `PGHOST`/`PGPORT`/`PGUSER`/`PGPASSWORD`/`PGDATABASE` |
| mysql     | `DATABASE_URL` (mysql://), `MYSQL_HOST`/`MYSQL_PORT`/`MYSQL_USER`/`MYSQL_PASSWORD`/`MYSQL_DATABASE` |
| mongodb   | `MONGODB_URI`, `MONGO_URL` |
| redis     | `REDIS_URL`, `REDIS_HOST`, `REDIS_PORT` |
| kafka     | `KAFKA_BROKERS`, `KAFKA_BOOTSTRAP_SERVERS` |
| elastic   | `ELASTICSEARCH_URL`, `ELASTIC_URL` |

**All frameworks:**

| Service   | Variables |
| --------- | --------- |
| (all)     | `NOWORRIES_<SERVICE>_HOST`, `NOWORRIES_<SERVICE>_PORT` (framework-agnostic) |
| (app)     | port env: `SERVER_PORT` (Spring) / `PORT` (Go/FastAPI/Node), or `app.port_env` |

If both postgres and mysql are declared, `DATABASE_URL` is the last one wired —
use the per-part vars (`PGHOST` vs `MYSQL_HOST`) to disambiguate.

## Flink pipelines

If the change is an Apache Flink **job** (not an HTTP app), use a `flink:` block
**instead of `app:`**. noworries stands up an ephemeral Flink session cluster
(jobmanager + taskmanager) on the same network as the declared services, builds
and submits the job(s) over the REST API, waits for `RUNNING`, then runs checks.

```yaml
version: 1
services: [kafka, postgres, elastic]
flink:
  image: flink:1.19        # optional
  slots: 2                 # optional task slots
  jobs:
    - build: "mvn -q -DskipTests package"   # optional; runs first
      jar: target/pipeline-0.1.jar          # required
      entry_class: com.acme.Pipeline        # optional
      args: ["--source", "events-in"]       # optional
checks:
  - name: "kafka -> postgres -> topic -> ES flows end to end"
    kafka:   { produce: { topic: "events-in", key: "E1", message: { id: "E1" } } }
    db:      { query: "SELECT status FROM processed WHERE id='E1'", expect_row: { status: "OK" } }
  - name: "enriched event indexed"
    kafka:   { expect_message: { topic: "events-enriched", contains: { id: "E1" }, timeout_ms: 15000 } }
    elastic: { index: "events", doc_id: "E1", expect_source_contains: { id: "E1" } }
```

**Critical — in-network addresses.** The job runs *inside* the cluster, so it
reaches services by compose name + container port, NOT the host ports: `kafka:9092`,
`postgres:5432`, `elastic:9200`, `redis:6379`, `mysql:3306`, `mongodb:27017`.
These are also injected as `NOWORRIES_<SERVICE>_HOST`/`_PORT`. Configure the job
to use those. First run pulls the ~600MB Flink image — use `--timeout 600`.

## External / upstream services (app calls out to something noworries can't run)

If the change makes the app call an **upstream service you don't containerize** —
a partner/sandbox API, a separate auth server, a payment gateway — declare it
under `externals:`. noworries injects its URL + credentials into the app's
environment (it does **not** stand the service up). This is app → upstream;
`services` is what noworries runs, `auth` is noworries → app.

**How to fill it (do this from the code, then ask):**

1. **Detect** the dependency: look in `application.properties`/`.yml`, config
   classes, or client code for a base URL (e.g. `payments.base-url`,
   `PARTNER_API_URL`) and how it authenticates (basic, bearer, api-key header).
2. **Derive** what you can. Set `env`/`url_env` to the exact env var / property
   the app reads. If auth details are in config/code, wire them.
3. **Never hardcode secrets or guess a URL.** For the sandbox URL and any
   credential you can't derive, reference `${VAR}` and **ask the user** for the
   value (sandbox base URL, username/password, token, or API key). Values go in
   the gitignored `.noworries.env`; an interactive run also prompts for missing
   `${VAR}`.

```yaml
externals:
  - name: payments
    url: "${PAYMENTS_URL}"              # ask the user for the sandbox URL
    url_env: PAYMENTS_BASE_URL          # the property/env your app actually reads
    auth:
      basic: { username: "${PAY_USER}", password: "${PAY_PASS}", header_env: PAYMENTS_AUTHORIZATION }
      # or bearer: { token: "${PAY_TOKEN}", header_env: PAYMENTS_AUTHORIZATION }
      # or api_key: { value: "${PAY_KEY}", header: "X-Api-Key", value_env: PAYMENTS_API_KEY }
```

Every external also sets conventional vars the app can read with no mapping:
`NOWORRIES_EXTERNAL_<NAME>_URL`, `…_USER`/`…_PASSWORD`/`…_AUTHORIZATION` (basic,
ready `Basic base64` header), `…_TOKEN`/`…_AUTHORIZATION` (bearer),
`…_API_KEY`/`…_API_KEY_HEADER` (api-key). `<NAME>` is uppercased with
non-alphanumerics → `_`. If the app has no matching env override yet, prefer
adding one in code that reads the conventional var, or set the app's real var via
`url_env`/`*_env`.

## Edge-case scenarios (don't just test the happy path)

For **data pipelines** (Flink, Kafka consumers, any read-then-write flow), a
single trigger→observe check is not enough — the code can pass it and still be
wrong under load. When the change touches such a flow, **add edge-case checks**
alongside the happy-path one. A check's `scenario:` block replaces the single
`kafka.produce` with a generated flood; verify it with an ordinary observe
assertion (usually an exact **count**). Pick the ones that match the risk:

| `kind`         | What it does | What it catches | Verify with |
| -------------- | ------------ | --------------- | ----------- |
| `burst`        | floods N messages (optionally rate-limited) over many keys | dropped messages / can't keep up | `expect_row_count: N` (no loss) |
| `concurrent`   | N writes to few keys across parallel producers → real races | lost updates, duplicate rows, stale final value | `expect_row_count: <keys>` + final value = highest `${i}` |
| `duplicates`   | sends every message **twice** | non-idempotent / double-processing | `expect_row_count: N` (not `2N`) |
| `out_of_order` | emits each key's events in **reverse** order (true order in `${i}`) | naive last-arrival-wins, bad windowing | final value = highest `${i}` |

```yaml
checks:
  # happy path first:
  - name: "single order persists"
    kafka: { produce: { topic: orders-in, key: "A", message: { id: "A", amount: 5 } } }
    db:    { query: "SELECT count(*) n FROM orders WHERE id='A'", expect_row: { n: 1 } }

  # then the edge cases:
  - name: "burst of 500 orders: none dropped"
    scenario:
      kind: burst
      count: 500
      concurrency: 8
      kafka: { topic: orders-in, key: "ord-${seq}", message: { id: "${uuid}", amount: "${seq}" } }
    db: { query: "SELECT count(*) n FROM orders", expect_row_count: 1, expect_row: { n: 500 } }

  - name: "duplicate delivery is idempotent"
    scenario:
      kind: duplicates
      count: 100
      kafka: { topic: orders-in, key: "dup-${seq}", message: { id: "dup-${seq}" } }
    db: { query: "SELECT count(*) n FROM orders WHERE id LIKE 'dup-%'", expect_row: { n: 100 } }

  - name: "concurrent updates to one key stay consistent"
    scenario:
      kind: concurrent
      count: 50
      keys: 1
      concurrency: 6
      kafka: { topic: orders-in, key: "hot", message: { id: "hot", version: "${i}" } }
    db: { query: "SELECT count(*) n FROM orders WHERE id='hot'", expect_row: { n: 1 } }
```

Template placeholders in `scenario.kafka.key`/`message`: `${seq}` (global index),
`${i}` (per-key sequence / version), `${key}` (assigned key), `${uuid}` (unique
id). Knobs: `count`, `concurrency`, `keys`, `rate_per_sec`. Scenarios flood the
pipeline, so give the observe assertions time — raise `--timeout` for big bursts.

## Check reference

A check may combine assertion types; it passes only if **all** pass. Order
within a check: **seed** (mysql/mongodb `seed`) → **trigger** (`request`,
`kafka.produce`, `scenario`) → **observe** (all queries/verifications). So "seed
data → hit the API → check what changed" works.

```yaml
version: 1
services: [postgres, redis, kafka, elastic, mysql, mongodb]
app:                          # optional; auto-detected from mvnw/gradlew
  start: "./mvnw spring-boot:run"
  health: "/actuator/health"
  ready_timeout: 180
auth:                         # optional; applied to every request. ${VAR} from .noworries.env
  login: { request: { method: POST, path: /auth/login, body: { username: "${U}", password: "${P}" } }, token_from: "$.accessToken" }
  # or: bearer: { token: "${API_TOKEN}" } | basic: { username: "${U}", password: "${P}" } | api_key: { header: "X-API-Key", value: "${K}" }
checks:
  # One check can span services:
  - name: "create order: 201, persists, caches, emits, indexes"
    tags: [orders]
    request: { method: POST, path: /orders, body: { sku: "ABC", qty: 2 } }
    expect:  { status: 201, body_contains: { sku: "ABC" } }
    db:      { query: "SELECT status FROM orders WHERE sku='ABC'", expect_row: { status: "PENDING" }, expect_row_count: 1 }
    redis:   { key: "cache:order:ABC", expect_exists: true, expect_value_contains: { status: "PENDING" } }
    kafka:   { expect_message: { topic: "order-events", contains: { type: "OrderCreated" }, timeout_ms: 5000 } }
    elastic: { index: "orders", doc_id: "ABC", expect_source_contains: { status: "PENDING" }, query: { term: { sku: "ABC" } }, expect_hits: 1 }

  # Kafka consumer feature (produce to trigger, verify the effect):
  - name: "OrderCreated is consumed"
    kafka: { produce: { topic: "orders", key: "X1", message: { type: "OrderCreated", sku: "X1" } } }
    db:    { query: "SELECT count(*) n FROM orders WHERE sku='X1'", expect_row: { n: 1 } }

  # MySQL seed -> request -> verify:
  - name: "reprice updates the row"
    mysql:   { seed: ["INSERT INTO orders(sku,price) VALUES('P',100)"], query: "SELECT price FROM orders WHERE sku='P'", expect_row: { price: 120 } }
    request: { method: POST, path: /orders/P/reprice, body: { price: 120 } }
    expect:  { status: 200 }

  # MongoDB seed (insert/update/delete) -> verify (find/count):
  - name: "order document is updated"
    mongodb:
      database: "app"
      collection: "orders"
      seed: [ { insert: { document: { sku: "M1", status: "NEW" } } } ]
      find: { sku: "M1" }
      expect_doc_contains: { status: "PROCESSED" }
      expect_count: 1
    request: { method: POST, path: /orders/M1/process }
    expect:  { status: 200 }

  # Elasticsearch template + ops:
  - name: "reindex writes the mapping"
    elastic:
      index: "orders"
      template: { name: "orders-tmpl", body: { index_patterns: ["orders*"], template: { mappings: { properties: { sku: { type: keyword } } } } } }
      operations: [ { insert: { id: "E1", document: { sku: "E1", status: "PENDING" } } } ]
      doc_id: "E1"
      expect_source_contains: { status: "PENDING" }
```

Field summary: **http** `request{method,path,headers,body}` + `expect{status,body_contains,max_ms}` (max_ms = latency budget) ·
**db/mysql** `query`, `expect_row` (deep subset on first row), `expect_row_count`; `mysql.seed` = SQL run before the request ·
**mongodb** `database`, `collection`, `seed[{insert|update{filter,set}|delete}]`, `find`, `expect_doc_contains`, `expect_count` ·
**redis** `key`, `expect_exists`, `expect_value`, `expect_value_contains` ·
**kafka** `produce{topic,key,message}`, `expect_message{topic,contains,timeout_ms}` ·
**scenario** `kind` (burst|concurrent|duplicates|out_of_order), `kafka{topic,key,message}`, `count`, `concurrency`, `keys`, `rate_per_sec`, `expect_throughput_per_sec` (edge-case load trigger; verify with observe counts) ·
**external_calls** `[{external,method,path,body_contains,times}]` (assert the app called a mocked external — needs `externals[].mock`) ·
**logs** `contains[]`, `absent[]` (grep `.noworries/app.log`) ·
**elastic** `index`, `template{name,body,legacy}`, `operations[{insert|update|delete}]`, `doc_id`, `expect_exists`, `expect_source_contains`, `query`, `expect_hits` ·
**graphql** `path`, `query`, `variables`, `expect_data`, `expect_no_errors` ·
**metrics** `path`, `metric`, `labels`, `expect` (Prometheus; `">= 1"` etc.) ·
**snapshot** `file`, `ignore[]` (golden diff of the check's response; `--update-snapshots` to write) ·
**schema** `table`, `has_columns[]`, `columns{col:type}` (Postgres information_schema) ·
**sse** `path`, `contains`, `timeout_ms` · **websocket** `url`, `send`, `expect_message`, `timeout_ms` ·
**grpc** `target`, `method`, `data`, `expect_contains` (needs `grpcurl` on PATH) ·
**traces** `query_url`, `service`, `operation`, `tags`, `min_count` (query Jaeger/Tempo for OTel spans).

Beyond checks: top-level **`setup`** = shell commands (migrations/fixtures, e.g.
`./mvnw flyway:migrate`) run with the app's env after infra is up, before the app
starts. **`externals[].mock`** stands up an in-process fake (stubs + records
calls) so you can assert outbound behaviour with `external_calls` instead of
hitting a real sandbox. Reports: `--junit <path>` / `--html <path>` for CI.

## Important behaviours

- **Checks run in order and share state.** The infra is one shared instance per
  run — a check that writes `ABC` is visible to later checks. If a later check
  asserts "exactly 1 row", an earlier check that inserted the same key will
  break it. Make check data disjoint, or reset in a `seed`.
- **Secrets:** reference `${VAR}` in `auth`/`externals`/headers/body; put values
  in a **`.noworries.env`** file (gitignored automatically). Derive auth from
  `application.properties`/code where possible; **ask the user** for the login
  URL / credentials / API key / upstream sandbox URL you can't derive. If auth
  lives on a **separate server** (not the app), use an absolute URL for
  `auth.login.request.path` (e.g. `${AUTH_URL}/oauth/token`); a relative path
  hits the app under test. For an upstream the app *calls*, use `externals`.
- **First run is slow:** Elasticsearch (~600MB), Kafka, and Mongo images pull on
  first use. Use `--timeout 600` for the first run of a project, otherwise a slow
  pull shows up as `NOT READY` (a timeout, not an app bug).
- Everything is torn down (`docker compose down -v` + the app) after each run
  unless `--keep-alive`.

## Troubleshooting

| Symptom | Look at / do |
| ------- | ------------ |
| `NOT READY`, app-side assertion fails | `.noworries/results.json` (expected vs actual) → fix code |
| `app did not become ready` | `.noworries/app.log` (stack trace); raise `app.ready_timeout` |
| `containers did not become healthy` / timeout on first run | image still pulling — re-run with `--timeout 600` |
| `docker compose up failed` / `daemon not reachable` | start Docker; check the image tag exists |
| Kafka/ES check connection error | the service is slow to become healthy — raise `--timeout` |
| `${VAR} is not set` warning, auth 401 | add the value to `.noworries.env` or `export` it |
