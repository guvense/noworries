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

The runner then waits for the app's health endpoint and executes the selected
checks — see [checks](checks.md).

## 4. Tear down

- The app process group is stopped (SIGTERM → SIGKILL).
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
