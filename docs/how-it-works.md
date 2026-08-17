# How it works

A run has four stages: **generate → up → verify → tear down**. Everything is
ephemeral and isolated.

## 1. Detect & generate

- Parse `noworries.yml`.
- Resolve the framework (Spring Boot auto-detected from `mvnw`/`gradlew`/
  `pom.xml`/`build.gradle`, or forced via `app.framework`).
- Ask each declared service's provider for its Compose fragment and generate
  `.noworries/compose.test.yml` — an isolated bridge network per run, healthchecks,
  and port mappings.
- Without `--yes`, print the detected services and wait for confirmation. Nothing
  is started until you approve.

## 2. Bring infrastructure up

- `docker compose up -d`.
- Poll container health. A service that declares a healthcheck must report
  **`healthy`** — a merely "running" container is not considered ready, so the app
  is never pointed at a database that isn't accepting connections yet.
- Resolve host ports:
  - **Postgres / Redis** get a random host port assigned by Docker, resolved via
    `docker compose port`.
  - **Kafka** gets a **fixed** host port chosen up front, because a Kafka client
    must be handed the address the broker advertises. Kafka runs single-node
    KRaft (no ZooKeeper) with its external listener advertised at `localhost:<that
    port>`.

## 3. Start the app & run checks

### Environment wiring

The app is started as a subprocess (its own process group) with env vars mapping
the resolved container ports. Each framework adapter translates the endpoints
into the variables its ecosystem expects.

For **Spring Boot** (a subset):

| Service  | Variables set |
| -------- | ------------- |
| Postgres | `SPRING_DATASOURCE_URL`, `SPRING_DATASOURCE_USERNAME`, `SPRING_DATASOURCE_PASSWORD` |
| MySQL    | `SPRING_DATASOURCE_URL` (jdbc:mysql), `SPRING_DATASOURCE_USERNAME`, `SPRING_DATASOURCE_PASSWORD` |
| MongoDB  | `SPRING_DATA_MONGODB_URI` |
| Kafka    | `SPRING_KAFKA_BOOTSTRAP_SERVERS` |
| Redis    | `SPRING_DATA_REDIS_HOST`, `SPRING_DATA_REDIS_PORT` |
| Elastic  | `SPRING_ELASTICSEARCH_URIS` |

For **Go / FastAPI / Node.js**, noworries exports the conventional connection
strings these ecosystems read (apps pick whichever their client expects):

| Service  | Variables set |
| -------- | ------------- |
| Postgres | `DATABASE_URL` (postgres://), `PGHOST`, `PGPORT`, `PGUSER`, `PGPASSWORD`, `PGDATABASE` |
| MySQL    | `DATABASE_URL` (mysql://), `MYSQL_HOST`, `MYSQL_PORT`, `MYSQL_USER`, `MYSQL_PASSWORD`, `MYSQL_DATABASE` |
| MongoDB  | `MONGODB_URI`, `MONGO_URL` |
| Redis    | `REDIS_URL`, `REDIS_HOST`, `REDIS_PORT` |
| Kafka    | `KAFKA_BROKERS`, `KAFKA_BOOTSTRAP_SERVERS` |
| Elastic  | `ELASTICSEARCH_URL`, `ELASTIC_URL` |

Regardless of framework:

| Service  | Variables set |
| -------- | ------------- |
| (all)    | `NOWORRIES_<SERVICE>_HOST`, `NOWORRIES_<SERVICE>_PORT` (framework-agnostic) |
| (app)    | the port env (`SERVER_PORT` for Spring, `PORT` for Go/FastAPI/Node, or `app.port_env`) = a free port chosen for the app |

Baked-in Postgres/MySQL credentials are `noworries` / `noworries` / `noworries`
(user / password / db). If both Postgres and MySQL are declared, `DATABASE_URL`
resolves to the last one wired — pick per-part vars (`PGHOST` vs `MYSQL_HOST`) to
disambiguate.

`DATABASE_URL` is a **URL** (`mysql://user:pass@host:port/db`). Postgres clients
(`lib/pq`, `pgx`, SQLAlchemy, JDBC via `SPRING_DATASOURCE_URL`) take it as-is,
but Go's `go-sql-driver/mysql` wants a DSN (`user:pass@tcp(host:port)/db`) and
rejects the URL form — build it from `MYSQL_HOST`/`MYSQL_PORT`/`MYSQL_USER`/
`MYSQL_PASSWORD`/`MYSQL_DATABASE` rather than parsing `DATABASE_URL`.

Services without a framework convention (SQL Server, ClickHouse, RabbitMQ,
Cassandra, OpenSearch) export the conventional vars listed in
[supported.md](supported.md) — `MSSQL_*`, `CLICKHOUSE_DSN`/`CLICKHOUSE_URL`,
`RABBITMQ_URL`, `CASSANDRA_CONTACT_POINTS`, `OPENSEARCH_URL`. Spring apps get
those **on top of** their `SPRING_*` properties, and an app whose framework
wasn't detected gets them too (before 0.14 both cases fell back to the bare
`NOWORRIES_*` host/port pairs, leaving the app with an address but no
credentials).

The **checks** get the same names for interpolation, plus
`NOWORRIES_APP_HOST`/`_PORT`/`_URL` for the app itself — so a target that can't
be relative (`grpc.target`, an absolute `ws://` URL) can say
`${NOWORRIES_APP_PORT}` instead of pinning a fixed port through `app.env`.

A healthy container is not always a ready protocol: RabbitMQ, MySQL/MariaDB and
Cassandra accept TCP before they complete an application-level handshake, so the
app's first `connect()` should sit in a short retry loop.

**External dependencies.** Any [`externals`](configuration.md#externals-optional)
the app calls out to (a partner sandbox, a separate auth server) are also injected
into the app's environment here — the sandbox URL plus credential env vars
(`NOWORRIES_EXTERNAL_<NAME>_*` and any app-specific `*_env` names you set), with
`${VAR}` secrets resolved from `.noworries.env`. noworries doesn't stand these
services up; it only hands the app the URL + credentials so the app's own code can
reach them.

The runner then waits for the app's health endpoint and executes the selected
checks — see [checks](checks.md).

## 4. Tear down

- The app's whole process tree is stopped (Unix: SIGTERM → SIGKILL on the
  process group; Windows: `taskkill /T /F`).
- `docker compose down -v` removes containers, the network, and volumes.
- Teardown is guaranteed: it runs on success, on failure, on Ctrl-C/SIGTERM, and
  when the hard `--timeout` fires. `--keep-alive` skips container teardown (but
  still stops the app) for debugging.

## The AI loop (`/noworries`)

The CLI is the deterministic half (run + verify + report). The smart half lives
in the `/noworries` Claude Code command:

1. `noworries changed` (or `changed --all` for `force`) tells Claude which files
   the change touched.
2. Claude reads the diff, understands the behaviour, and writes/updates the
   `checks` in `noworries.yml`.
3. Claude runs `noworries --yes`, reads `.noworries/results.json`.
4. If NOT READY, Claude fixes the code (using the expected-vs-actual and
   `app.log`) and runs again — until READY, or it stops to ask you.
