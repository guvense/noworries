# Supported frameworks, services & checks

Everything noworries can detect, stand up, and verify — at a glance. If your
stack is here, a `noworries.yml` needs almost no configuration; noworries
auto-detects the app framework and wires each service's connection env for you.

- **Frameworks** are auto-detected from the project (override with `app.framework`).
- **Services** are declared in `services:` as `kind` or `kind:tag`.
- **Checks** are the assertion types you combine inside a check.

Full config reference: [configuration.md](configuration.md) · check details:
[checks.md](checks.md).

---

## Application frameworks

noworries starts your **real** app and probes it from the outside. Detection
runs in this priority order (first match wins), so a specific marker beats a
broad one:

| Framework | Detected by | Default start command | Readiness | Port env |
| --- | --- | --- | --- | --- |
| **Spring Boot** (Java/Kotlin) | `pom.xml`, `build.gradle(.kts)`, `mvnw`, `gradlew` | `./mvnw spring-boot:run` (or `mvn` / `./gradlew bootRun` / `gradle bootRun`) | `GET /actuator/health` | `SERVER_PORT` |
| **ASP.NET Core** (.NET) | a `*.csproj` / `*.fsproj` / `*.sln` in the dir | `dotnet run` | TCP (port open) | `ASPNETCORE_HTTP_PORTS` |
| **Go** | `go.mod` | `go run .` | TCP (port open) | `PORT` |
| **Ruby on Rails** | `bin/rails` or `config/application.rb` | `bin/rails server` (or `bundle exec rails server`) | TCP (port open) | `PORT` |
| **Laravel** (PHP) | `artisan` | `php artisan serve` | TCP (port open) | `PORT` |
| **Django** (Python) | `manage.py` | `python manage.py runserver --noreload` | TCP (port open) | `PORT` |
| **FastAPI** (Python) | `main.py` / `app.py`, or a manifest referencing FastAPI/Uvicorn | `uvicorn main:app` (or `app:app`) | TCP (port open) | `PORT` |
| **Node.js** | `package.json` | `npm start` (or `node server.js` / `index.js` / `app.js`) | TCP (port open) | `PORT` |

Notes:

- **Readiness = TCP** means noworries waits for the HTTP port to accept
  connections (most frameworks have no universal health endpoint). Set
  `app.health: "/healthz"` to probe a real endpoint instead. Spring Boot
  defaults to `/actuator/health`.
- **Anything else?** noworries is framework-agnostic — set `app.start`,
  `app.health`, and `app.port_env` explicitly and it will start and probe any
  HTTP app, detected or not.
- **Env wiring:** each framework receives its native config keys —
  Spring Boot `SPRING_DATASOURCE_*` / `SPRING_DATA_*` / `SPRING_RABBITMQ_*` /
  `SPRING_CASSANDRA_*`, Laravel `DB_CONNECTION` / `DB_*`, Rails/Node/Go/FastAPI
  the conventional `DATABASE_URL` / `REDIS_URL` / `KAFKA_BROKERS` / … — plus
  framework-agnostic `NOWORRIES_*` fallbacks, always present.

---

## Services (ephemeral infrastructure)

Declared in `services:`. Each boots in a throwaway Docker container with a
healthcheck; connection details are injected into the app's env. Bare `kind`
uses the default image; `kind:tag` pins a tag.

| Declare as | Default image | Notes |
| --- | --- | --- |
| `postgres` | `postgres:16-alpine` | |
| `timescaledb` / `timescale` | `timescale/timescaledb:2.17.2-pg16` | Postgres-wire — reuses the Postgres provider/checks |
| `cockroachdb` / `cockroach` | `cockroachdb/cockroach:v24.2.0` | Postgres-wire; single-node insecure, `root`/`defaultdb` |
| `mysql` | `mysql:8.4` | |
| `mariadb` | `mariadb:11` | MySQL-wire — reuses the MySQL provider/checks |
| `mssql` / `sqlserver` | `mcr.microsoft.com/mssql/server:2022-latest` | `schema` check via in-container `sqlcmd` |
| `mongodb` / `mongo` | `mongo:7` | |
| `redis` | `redis:7-alpine` | |
| `kafka` | `apache/kafka:3.7.0` | advertises `localhost:9092` |
| `rabbitmq` / `rabbit` / `amqp` | `rabbitmq:3.13-management` | management API (15672) for the `rabbitmq` check |
| `elastic` / `elasticsearch` | `docker.elastic.co/elasticsearch/elasticsearch:8.13.4` | ES 7 & 8 |
| `opensearch` | `opensearchproject/opensearch:2.17.1` | Elasticsearch-compatible |
| `clickhouse` | `clickhouse/clickhouse-server:24.8` | HTTP interface (8123) for the `clickhouse` check |
| `cassandra` | `cassandra:5.0` | CQL via in-container `cqlsh` |
| `scylla` / `scylladb` | `scylladb/scylla:6.1` | CQL-wire — reuses the Cassandra provider/checks |

Plus **Apache Flink** (`flink:1.19` by default) — declared in its own `flink:`
block (an ephemeral jobmanager + taskmanager session cluster) instead of
`services:`, when the thing under test is a Flink job. See
[configuration.md](configuration.md).

**Wire-compatible aliases** (`timescaledb`, `mariadb`, `scylladb`) reuse a
peer's provider and checks — only the pulled image differs. `cockroachdb` and
`opensearch` speak a peer's protocol but need their own container config, so
they are distinct kinds.

---

## Check / assertion types

Combine any of these inside a single check; the check passes only if **every**
assertion passes. Full semantics in [checks.md](checks.md).

| Area | Types |
| --- | --- |
| **HTTP** | `request` + `expect` — status, `body_contains`, `max_ms` latency budget |
| **SQL** | `db` (Postgres query → `expect_row`), `mysql` (seed → query), `schema` (column/type diff — Postgres, MySQL, or SQL Server) |
| **NoSQL / cache** | `mongodb` (seed → find), `redis` (key/value) |
| **Messaging** | `kafka` (produce / `expect_message`), `rabbitmq` (queue depth via management API) |
| **Search** | `elastic` (template + insert/update/delete + query; ES 7 & 8, OpenSearch) |
| **Analytics** | `clickhouse` (HTTP SQL query → `expect_value` / `expect_row` / `expect_rows`) |
| **Wide-column** | `cassandra` (CQL query → `expect_value` / `expect_row` / `expect_rows`; incl. Scylla) |
| **Realtime** | `sse` (Server-Sent Events), `websocket` |
| **APIs** | `graphql`, `grpc` (via `grpcurl`) |
| **Observability** | `metrics` (Prometheus), `traces` (OpenTelemetry via Jaeger/Tempo), `logs` (contains / absent) |
| **Contracts** | `snapshot` (golden-file diff), `external_calls` (assert outbound calls to a built-in mock) |
| **Security** | `security` — abuse-case checks: `require_auth`, `reject_bad_input` (no 5xx on hostile input), `no_error_leak`, `require_headers` |
| **Edge cases** | `scenario` — `burst`, `concurrent` (races), `duplicates` (idempotency), `out_of_order`, with concurrency / rate limits and a throughput assertion |

Assertions that observe an eventually-consistent effect (DB, cache, search,
ClickHouse, RabbitMQ, Cassandra, metrics, traces) **retry within a budget**, so
async writes have time to land.

---

## Test scenarios & modes

Beyond a single request→assert, noworries covers the harder-to-verify behaviours
of real systems:

- **Multi-service, one check.** A single check can assert across layers at once —
  HTTP status + latency budget (`max_ms`), the DB row that landed, the cache key
  that was set, the message on the downstream topic, the outbound call to a mock,
  and the app log — so "it worked" means the whole slice worked, not just the
  status code.
- **Race conditions & concurrency.** `scenario: { kind: concurrent }` fires many
  parallel producers at the same small set of keys to create *real* contention,
  then your `observe` assertions prove the invariant held (no lost update, no
  double-processing). `duplicates` re-sends the same logical message to check
  **idempotency**; `out_of_order` shuffles delivery to check ordering assumptions.
- **Load / throughput.** `scenario: { kind: burst, count, concurrency, rate_per_sec }`
  drives volume through the pipeline; `expect_throughput_per_sec` asserts a floor,
  so a regression that halves throughput fails the run. (Load sink is Kafka today.)
- **Async pipelines.** For out-of-band processing (a Kafka consumer, a **Flink**
  job), noworries retries eventual-consistency observers within a budget and lets
  you stage the observation — `kafka.expect_message` on the downstream topic
  (with `timeout_ms`) before the DB/search assert. See the Flink section in
  [configuration.md](configuration.md).
- **Security / abuse cases (defensive).** `security` probes an endpoint of *your
  own* app with hostile input and asserts it stays safe: auth is enforced
  (`require_auth` → 401/403 when stripped), malformed/injection-style input never
  causes a 5xx (`reject_bad_input`), responses don't leak stack traces or DB
  errors (`no_error_leak`), and security headers are present (`require_headers`).
  It only ever talks to the ephemeral app under test and asserts *safe behaviour*
  — it is a hardening regression check, not an exploit tool. See
  [checks.md](checks.md#security-abuse-case-checks).

---

## External requirements

- **Docker** (with the `docker compose` plugin) — for the ephemeral services.
- **`grpcurl`** on `PATH` — only if you use the `grpc` check.
- No language toolchain is required by noworries itself; it starts your app with
  whatever your project already uses (`mvnw`, `dotnet`, `go`, `npm`, …).
