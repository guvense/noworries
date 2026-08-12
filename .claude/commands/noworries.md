---
description: Verify the change just made — auto-generate noworries.yml, stand up infra, run checks, fix and re-run until READY
argument-hint: "[force | init]"
allowed-tools: Bash(noworries:*), Bash(cargo:*), Bash(git:*), Bash(docker:*), Read, Edit, Write
---

# /noworries

> **Requires `noworries` >= 0.2.0** (`noworries --version`). Older builds lack
> some services/fields below and have the Kafka consumer offset-storage bug.

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
   Redis caching, Elasticsearch indexing, MySQL/Mongo reads-then-writes.
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

## Auto-injected environment (Spring Boot)

The app is started with these set from the resolved random container ports — you
usually **don't need to touch `application.yml`**:

| Service   | Variables |
| --------- | --------- |
| postgres  | `SPRING_DATASOURCE_URL` (jdbc:postgresql), `SPRING_DATASOURCE_USERNAME`, `SPRING_DATASOURCE_PASSWORD` |
| mysql     | `SPRING_DATASOURCE_URL` (jdbc:mysql), `SPRING_DATASOURCE_USERNAME`, `SPRING_DATASOURCE_PASSWORD` |
| mongodb   | `SPRING_DATA_MONGODB_URI` |
| redis     | `SPRING_DATA_REDIS_HOST`, `SPRING_DATA_REDIS_PORT` |
| kafka     | `SPRING_KAFKA_BOOTSTRAP_SERVERS` |
| elastic   | `SPRING_ELASTICSEARCH_URIS` |
| (all)     | `NOWORRIES_<SERVICE>_HOST`, `NOWORRIES_<SERVICE>_PORT` (framework-agnostic) |
| (app)     | `SERVER_PORT` (or `app.port_env`) |

## Check reference

A check may combine assertion types; it passes only if **all** pass. Order
within a check: **seed** (mysql/mongodb `seed`) → **trigger** (`request`,
`kafka.produce`) → **observe** (all queries/verifications). So "seed data → hit
the API → check what changed" works.

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

Field summary: **http** `request{method,path,headers,body}` + `expect{status,body_contains}` ·
**db/mysql** `query`, `expect_row` (deep subset on first row), `expect_row_count`; `mysql.seed` = SQL run before the request ·
**mongodb** `database`, `collection`, `seed[{insert|update{filter,set}|delete}]`, `find`, `expect_doc_contains`, `expect_count` ·
**redis** `key`, `expect_exists`, `expect_value`, `expect_value_contains` ·
**kafka** `produce{topic,key,message}`, `expect_message{topic,contains,timeout_ms}` ·
**elastic** `index`, `template{name,body,legacy}`, `operations[{insert|update|delete}]`, `doc_id`, `expect_exists`, `expect_source_contains`, `query`, `expect_hits`.

## Important behaviours

- **Checks run in order and share state.** The infra is one shared instance per
  run — a check that writes `ABC` is visible to later checks. If a later check
  asserts "exactly 1 row", an earlier check that inserted the same key will
  break it. Make check data disjoint, or reset in a `seed`.
- **Secrets:** reference `${VAR}` in `auth`/headers/body; put values in a
  **`.noworries.env`** file (gitignored automatically). Derive auth from
  `application.properties`/code where possible; **ask the user** for the login
  URL / credentials / API key you can't derive. If auth lives on a **separate
  server** (not the app), use an absolute URL for `auth.login.request.path`
  (e.g. `${AUTH_URL}/oauth/token`); a relative path hits the app under test.
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
