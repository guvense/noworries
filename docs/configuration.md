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
| `mariadb`           | `mariadb:11`              |
| `mariadb:11.4`      | `mariadb:11.4`            |
| `mongodb` / `mongo` | `mongo:7`                 |
| `timescaledb` / `timescale` | `timescale/timescaledb:2.17.2-pg16` |
| `cockroachdb` / `cockroach` | `cockroachdb/cockroach:v24.2.0` |
| `mssql` / `sqlserver` | `mcr.microsoft.com/mssql/server:2022-latest` |
| `rabbitmq` / `rabbit` / `amqp` | `rabbitmq:3.13-management` |
| `opensearch`        | `opensearchproject/opensearch:2.17.1` |
| `clickhouse`        | `clickhouse/clickhouse-server:24.8` |
| `cassandra`         | `cassandra:5.0`           |
| `scylla` / `scylladb` | `scylladb/scylla:6.1`   |
| `smtp` / `mailpit` / `mailhog` | `axllent/mailpit:latest` |
| `minio` / `s3`      | `minio/minio:latest`      |

`smtp` stands up a mail sink (Mailpit): the app sends over SMTP and the
[`email`](checks.md#email-email) check reads the mailbox back over its JSON API,
so nothing leaves the machine. `minio` stands up S3-compatible storage for the
[`s3`](checks.md#object-storage-s3) check — it starts with no buckets, so create
one in `setup:` or let the app create it on first write.

Note that `kafka:<tag>` expands to the **`apache/kafka`** repository (Kafka's
official image) and `elastic`/`elasticsearch` expand to the official
**`docker.elastic.co/elasticsearch/elasticsearch`** image — not the bare names.

Several kinds are **wire-compatible aliases** that reuse a peer's provider and
checks, differing only in the pulled image: `mariadb` maps onto MySQL,
`timescaledb` onto Postgres, and `scylladb` onto Cassandra. `cockroachdb` also
speaks the Postgres wire protocol but needs its own container config
(`start-single-node --insecure`, port 26257, `root`/`defaultdb`), so it is a
distinct kind rather than an alias.

Each service boots with a Docker healthcheck and exports connection env to the
app under test (e.g. `RABBITMQ_URL`/`AMQP_URL`, `CLICKHOUSE_URL`,
`OPENSEARCH_URL`, `CASSANDRA_CONTACT_POINTS`, `MSSQL_*`). Live check runners
that connect *from* noworries (e.g. `schema` against SQL Server, an AMQP/HTTP
assertion for RabbitMQ, an HTTP query check for ClickHouse) are a follow-up
layer — the provider layer above makes each service declarable and bootable.

## `app` (optional)

How to launch the app under test. Omit it entirely and `noworries` auto-detects
the framework from the files in your project root:

| Framework      | Adapter name  | Detected by | Default start | Health | Port env |
| -------------- | ------------- | ----------- | ------------- | ------ | -------- |
| Spring Boot    | `spring-boot` | `pom.xml` / `build.gradle(.kts)` / `mvnw` / `gradlew` | `./mvnw spring-boot:run` (or `gradle`/`mvn` equivalent) | `/actuator/health` | `SERVER_PORT` |
| ASP.NET Core   | `dotnet`      | a `*.csproj` / `*.fsproj` / `*.sln` in the root | `dotnet run` | TCP (`none`) | `ASPNETCORE_HTTP_PORTS` |
| Go             | `go`          | `go.mod` | `go run .` | TCP (`none`) | `PORT` |
| Ruby on Rails  | `rails`       | `bin/rails` or `config/application.rb` | `bin/rails server -b 0.0.0.0 -p ${PORT}` | TCP (`none`) | `PORT` |
| Laravel (PHP)  | `laravel`     | `artisan` | `php artisan serve --host=0.0.0.0 --port=${PORT}` | TCP (`none`) | `PORT` |
| Django (Python) | `django`     | `manage.py` | `python manage.py runserver 0.0.0.0:${PORT} --noreload` | TCP (`none`) | `PORT` |
| FastAPI (Python) | `fastapi`   | `main.py` / `app.py`, or a `pyproject.toml`/`requirements.txt` mentioning fastapi/uvicorn | `uvicorn main:app --host 0.0.0.0 --port ${PORT}` | TCP (`none`) | `PORT` |
| Node.js        | `node`        | `package.json` | `npm start` (or `node server.js`/`index.js`/`app.js`) | TCP (`none`) | `PORT` |

Detection runs in that order, so a Spring Boot project that also ships a
`package.json` (e.g. a JS frontend) still resolves to `spring-boot`, and a
Django project that happens to have a `main.py` still resolves to `django`
(its `manage.py` is checked first). Set `app.framework` to force one explicitly.

Django, Rails, .NET and Go read the standard connection-string env vars
(`DATABASE_URL`, `REDIS_URL`, `KAFKA_BROKERS`, ...). Laravel additionally gets
its native discrete vars (`DB_CONNECTION`/`DB_HOST`/`DB_PORT`/`DB_DATABASE`/
`DB_USERNAME`/`DB_PASSWORD`, `REDIS_HOST`/`REDIS_PORT`). The
framework-agnostic `NOWORRIES_<KIND>_HOST`/`_PORT` fallbacks are always present.

| Field           | Default                              | Meaning |
| --------------- | ------------------------------------ | ------- |
| `start`         | auto-detected from the framework     | Shell command to start the app. |
| `health`        | framework default (`/actuator/health` for Spring; `none` for the rest) | Path polled until it returns 2xx before checks run. Set to `none` to skip HTTP and just wait for the port to accept TCP. |
| `framework`     | auto-detect                          | Force a specific framework adapter by name (`spring-boot`, `dotnet`, `go`, `rails`, `laravel`, `django`, `fastapi`, `node`). |
| `port_env`      | framework default (`SERVER_PORT` for Spring; `ASPNETCORE_HTTP_PORTS` for .NET; `PORT` for the rest) | Env var the app reads for its HTTP port. |
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
| `topics`        | `[]`         | Extra Kafka topics to pre-create before jobs start. Topics named by a check's `kafka.produce` / `kafka.expect_message` are added automatically; use this only for a source/sink topic no check references. |
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

### Flink gotchas (learned the hard way)

- **Java version.** The default `flink:1.19` bundles **Java 11**. A job compiled
  for **Java 17** fails to load — set `image: flink:1.19-java17` (or a matching
  tag) explicitly.
- **Kafka source topics must exist first.** Flink's `KafkaSource` uses
  `AdminClient.describeTopics`, which does **not** trigger broker auto-create
  (even with `KAFKA_AUTO_CREATE_TOPICS_ENABLE=true`) — the job would hard-fail on
  a missing source topic. **noworries handles this for you:** before submitting
  the jobs it pre-creates every topic referenced by a check's `kafka.produce` /
  `kafka.expect_message`, plus any you list in `flink.topics`. So you no longer
  need an `ensureTopics()` step in the job — just make sure the source topic is
  named by a check or in `flink.topics`. (One partition / one replica; declare
  the topic in `flink.topics` only if no check names it.)
- **`flink-connector-elasticsearch7` ≠ ES 8.** The ES 7 connector can't parse an
  ES 8 bulk response (`IOException: Unable to parse response body`). If you use
  that connector, pin the service to ES 7 — `services: [elasticsearch:7.17.22]`
  — instead of the default ES 8.x.
- **Elasticsearch refresh policy.** The connector exposes no bulk-level refresh
  policy, and an **item-level** `setRefreshPolicy(...)` is rejected
  (`RefreshPolicy is not supported on an item request`). Control visibility via
  the index template instead: `settings: { refresh_interval: "100ms" }` in your
  `elastic.template`. (noworries also issues a `_refresh` before each search
  assertion, so `expect_hits` no longer races the default 1s interval.)
- **Observe async pipelines in stages.** A Flink pipeline is asynchronous:
  gate on the *downstream* signal before asserting the sink. Put
  `kafka.expect_message` on the output topic (with `timeout_ms`) **first**, then
  the `elastic`/`db` assertion. noworries retries the observers within the
  check's budget, but the explicit downstream wait fails fast with a clear
  message when the job never produced.

## `setup` (optional)

Shell commands run **after infra is healthy and before the app starts**, with the
same environment the app gets (DB URLs, `externals`, `app.env`). Use it for
migrations and fixtures — Flyway, Liquibase, Prisma, Alembic, raw `psql`, seed
scripts. A non-zero exit aborts the run.

```yaml
setup:
  - "./mvnw -q flyway:migrate"
  - "psql \"$DATABASE_URL\" -f fixtures/seed.sql"
```

Runs on the platform shell (`sh -c` on Unix, `cmd /C` on Windows).

## `externals` (optional)

For an **upstream/third-party service the app calls out to** but that noworries
does **not** stand up — a partner sandbox API, a separate auth server, a payment
gateway. noworries injects its URL and credentials into the app's environment so
the app can reach it during the run. This is different from `services` (which
noworries containerizes) and from `auth` (which authenticates noworries → app):
`externals` is app → upstream.

```yaml
externals:
  - name: payments               # drives the conventional env prefix
    url: "https://sandbox.pay.example.com"
    url_env: PAYMENTS_BASE_URL    # optional: also expose the URL under your app's name
    env:                          # optional: extra literal env (values interpolate ${VAR})
      PAYMENTS_TIMEOUT_MS: "5000"
    auth:                         # optional: basic | bearer | api_key
      basic:
        username: "${PAY_USER}"           # secret -> .noworries.env, prompted if missing
        password: "${PAY_PASS}"
        username_env: PAYMENTS_USERNAME    # optional app-specific aliases
        password_env: PAYMENTS_PASSWORD
        header_env: PAYMENTS_AUTHORIZATION # gets "Basic base64(user:pass)"
      # bearer: { token: "${PAY_TOKEN}", scheme: Bearer, token_env: PAYMENTS_TOKEN, header_env: PAYMENTS_AUTHORIZATION }
      # api_key: { value: "${PAY_KEY}", header: X-Api-Key, value_env: PAYMENTS_API_KEY }
```

Every external always sets **conventional** vars, and — where you provide the
app-specific names — those too (so no app config change is needed):

| Declared | Conventional vars set | Also set when you name them |
| -------- | --------------------- | --------------------------- |
| `url`    | `NOWORRIES_EXTERNAL_<NAME>_URL` | `url_env` |
| `basic`  | `…_USER`, `…_PASSWORD`, `…_AUTHORIZATION` (= `Basic base64(u:p)`) | `username_env`, `password_env`, `header_env` |
| `bearer` | `…_TOKEN`, `…_AUTHORIZATION` (= `<scheme> <token>`) | `token_env`, `header_env` |
| `api_key`| `…_API_KEY`, `…_API_KEY_HEADER` (header name) | `value_env` |
| `env`    | (your literal keys, verbatim) | — |

`<NAME>` is the external's `name` uppercased with non-alphanumerics turned into
`_` (`payments-v2` → `PAYMENTS_V2`). All values interpolate `${VAR}` from
`.noworries.env` / the process env, so credentials stay out of the repo and are
prompted for when missing (see below). External vars sit **above** the framework
service wiring but **below** `app.env`, so an explicit `app.env` entry still wins.

### Mocking an external (`mock`)

Instead of pointing at a real sandbox, noworries can stand up an **in-process
mock** of the external: it serves canned responses and **records every request**
the app makes, so a check can assert the app actually called it (see
`external_calls` in [checks](checks.md#external-calls)). The mock's URL overrides
the external's `url` — the app calls the mock with no code change.

```yaml
externals:
  - name: payments
    url_env: PAYMENTS_BASE_URL      # the mock's URL is injected here (and conventionally)
    mock:
      stubs:
        - when: { method: POST, path: /charge, body_contains: { amount: 200 } }   # match by body too
          respond: { status: 402, body: { error: "too big" } }
        - when: { method: POST, path: /charge }
          respond: { status: 201, body: { id: "ch_1", status: "ok" } }
        - when: { path: /slow }
          respond: { status: 200, delay_ms: 3000 }   # artificial latency (test client timeout/circuit-breaker)
        - when: { path: /health }   # method omitted = any
          respond: { status: 200 }
```

Stubs match top-to-bottom, first match wins. `when`: `path` (exact, query ignored),
`method` (optional = any), `body_contains` (optional deep-subset match on the
request JSON body — so the same path can answer differently per payload; put
specific body stubs before a catch-all). An unmatched request is still recorded
and answered `200` empty. `respond`: `status` (default 200), `body` (JSON),
`headers`, `delay_ms` (latency before replying).

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

  # 5) OpenID Connect / OAuth2 — noworries fetches the token itself:
  # oidc:
  #   issuer: "https://id.example.com/realms/app"   # discovery finds token_endpoint
  #   client_id: orders-app
  #   client_secret: "${OIDC_SECRET}"
  #   scope: "orders:read orders:write"
```

| Sub-block | Effect |
| --------- | ------ |
| `login`   | Runs the login request, extracts `token_from` (JSON path), and adds `<scheme> <token>` to `header`. The login `path` hits the app under test when relative (`/auth/login`), or a **separate auth server** when it's an absolute URL (`https://auth.example.com/oauth/token`, or `${AUTH_URL}/token`). |
| `bearer`  | Adds `<scheme> <token>` to `header` (defaults: `Bearer`, `Authorization`). |
| `basic`   | Adds `Authorization: Basic base64(user:pass)`. |
| `api_key` | Adds the key as a header and/or a query parameter (defaults to header `X-API-Key`). |
| `oidc`    | Fetches a token from an OAuth2/OIDC provider and adds it as a bearer. See below. |

### `auth.oidc` — Keycloak / Auth0 / Cognito / Entra

Instead of hand-rolling the token request as an `auth.login`, declare the
provider and let noworries do the round-trip. With `issuer`, the token endpoint
is read from `<issuer>/.well-known/openid-configuration`; with `token_url` the
discovery step is skipped.

```yaml
auth:
  oidc:
    issuer: "https://id.example.com/realms/app"
    # token_url: "https://id.example.com/realms/app/protocol/openid-connect/token"
    client_id: orders-app
    client_secret: "${OIDC_SECRET}"   # omit for a public client
    grant: client_credentials         # default; or `password`
    # username: "${TEST_USER}"        # grant: password only
    # password: "${TEST_PASS}"
    scope: "orders:read orders:write"
    audience: "https://api.example.com"   # Auth0 needs this to issue a JWT for your API
    client_auth: post                 # default; `basic` puts the secret in an
                                      # Authorization header (Cognito requires it)
    params: { resource: "api://orders" }  # anything else the provider wants
    token_from: "$.access_token"      # default
    header: Authorization             # default
    scheme: Bearer                    # default ("" = raw token)
```

A failing token request aborts the run and passes the provider's own reason
through (`invalid_client`, `invalid_scope`, `unauthorized_client`) — that error
body is the only useful diagnostic when a client is misconfigured.

## `users` (optional)

Named identities, each the same shape as `auth:`. A check selects one with
`as:`; a check without `as:` uses the top-level `auth:`. This is how you test
role-based access: same request, different role, different expected status.

```yaml
auth:
  login: { request: { method: POST, path: /auth/login, body: { u: "${U}", p: "${P}" } }, token_from: "$.token" }
users:
  admin:
    oidc: { issuer: "${ISSUER}", client_id: admin-cli, client_secret: "${ADMIN_SECRET}" }
  reader:
    login: { request: { method: POST, path: /auth/login, body: { u: reader, p: "${READER_PASS}" } }, token_from: "$.token" }
checks:
  - name: "an admin can delete an order"
    as: admin
    request: { method: DELETE, path: /orders/1 }
    expect:  { status: 204 }

  - name: "a reader cannot"
    as: reader
    request: { method: DELETE, path: /orders/2 }
    expect:  { status: 403 }
```

Every identity is resolved **once**, before the checks run, so a login or token
round-trip happens per identity rather than per check. An `as:` naming an
undeclared user is rejected when the spec is parsed — otherwise the check would
silently run as the default identity and an RBAC assertion could pass for the
wrong reason. The identity applies to the whole check, not just its HTTP
request: `graphql`, `sse`, `websocket`, `grpc`, `metrics`, `traces` and
`security` probes all use it too.

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
