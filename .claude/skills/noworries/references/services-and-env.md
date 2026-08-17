# noworries — services, frameworks, injected environment

_Reference for the `noworries` skill. Read this when choosing `services:` or when the app can't reach a container._

## Supported services

`postgres` · `timescaledb` · `cockroachdb` · `mysql` · `mariadb` · `mssql`
(`sqlserver`) · `mongodb` (`mongo`) · `redis` · `kafka` · `rabbitmq` (`amqp`) ·
`elastic` (`elasticsearch`) · `opensearch` · `clickhouse` · `cassandra` ·
`scylladb`. Declare as `kind` or `kind:tag`:

```yaml
services: [ postgres:16-alpine, mysql, mongodb, redis, kafka, elastic ]
```

Container credentials are `noworries`/`noworries`/`noworries` where applicable
(postgres, mysql/mariadb, clickhouse; SQL Server SA password `Noworries!Pass1`).
Mongo/redis/kafka/elastic/cassandra run without auth; rabbitmq is
`noworries`/`noworries`. Wire-compatible aliases reuse a peer's provider/checks:
`timescaledb`+`mariadb` → Postgres/MySQL, `scylladb` → Cassandra (only the image
differs). `cockroachdb`/`opensearch` are Postgres-/Elasticsearch-compatible but
distinct kinds. See docs/supported.md for the full table.

**Container-healthy is not protocol-ready.** noworries waits for each
container's healthcheck, but several servers accept TCP before they will finish
an application-level handshake: RabbitMQ (first `connect()` can still get
ECONNRESET), MySQL/MariaDB (the daemon is still initializing on first boot),
Cassandra/Scylla (~30s before CQL answers). **Give the app a short connect-retry
loop** — a few attempts a couple of seconds apart. This is ordinary client
practice, not a noworries quirk, and it is the single most common cause of an
app that "crashed at startup" in an otherwise green run.

## Frameworks

Auto-detected from the project root (override with `app.framework`): **Spring
Boot** (`pom.xml`/`gradle`/`mvnw`), **ASP.NET Core** (`*.csproj`/`*.sln`), **Go**
(`go.mod`), **Rails** (`bin/rails`/`config/application.rb`), **Laravel**
(`artisan`), **Django** (`manage.py`), **FastAPI** (`main.py`/`app.py` or a
fastapi/uvicorn manifest), **Node.js** (`package.json`). Detection priority is
Spring → .NET → Go → Rails → Laravel → Django → FastAPI → Node, so a JVM app that
also ships a `package.json` still detects as Spring, and a Django project with a
stray `main.py` still detects as Django. Spring probes `/actuator/health`; the
others wait for the TCP port (`health: none`) unless you set `app.health`.

> **Detection is from the project root only** (the `--dir`, default cwd). In a
> multi-module repo or one with sibling apps, run from (or point `--dir` at) the
> module you're verifying, and use `app.start`/`app.framework` explicitly if the
> markers of two frameworks sit in the same directory. Test a specific scope with
> `noworries --file <path>` when a repo holds more than one `noworries.yml`.

### Per-framework traps

- **Go:** the default start is `go run .`, which recompiles on every run. For
  repeat runs and CI, build once and point at the binary:
  `app: { start: "./server" }`. Also, `DATABASE_URL` is injected in **URL form**
  (`mysql://user:pass@host:port/db`) — `go-sql-driver/mysql` does *not* accept
  it and wants a DSN (`user:pass@tcp(host:port)/db`). Build the DSN from the
  per-part vars (`MYSQL_HOST`, `MYSQL_PORT`, `MYSQL_USER`, `MYSQL_PASSWORD`,
  `MYSQL_DATABASE`) instead of parsing `DATABASE_URL`. `lib/pq` and `pgx` take
  the Postgres `DATABASE_URL` as-is.
- **Python (FastAPI/Django):** detection finds the project, but the detected
  command runs whatever is on `PATH` — inside a virtualenv that is usually *not*
  your `uvicorn`. Point at it explicitly:
  `app: { start: ".venv/bin/uvicorn main:app --port ${PORT}" }`.
- **Django:** `manage.py runserver` is WSGI-only — a Channels/ASGI app needs
  `daphne`/`uvicorn`. Migrations are not run for you either; use a wrapper
  (`app.start: "./start.sh"` doing `migrate` then serving) or top-level `setup:`,
  and make sure migration files are committed (`makemigrations` first).
- **Node:** a Kafka client that batches (kafka-go's default `BatchSize=100`,
  `BatchTimeout=1s`) delays a single message by ~1s — raise
  `kafka.expect_message.timeout_ms` or flush explicitly.

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

**Go / FastAPI / Node.js / Django / Rails / .NET** (conventional connection
strings — read whichever your client expects):

| Service   | Variables |
| --------- | --------- |
| postgres  | `DATABASE_URL` (postgres://), `PGHOST`/`PGPORT`/`PGUSER`/`PGPASSWORD`/`PGDATABASE` |
| mysql     | `DATABASE_URL` (mysql://), `MYSQL_HOST`/`MYSQL_PORT`/`MYSQL_USER`/`MYSQL_PASSWORD`/`MYSQL_DATABASE` |
| mongodb   | `MONGODB_URI`, `MONGO_URL` |
| redis     | `REDIS_URL`, `REDIS_HOST`, `REDIS_PORT` |
| kafka     | `KAFKA_BROKERS`, `KAFKA_BOOTSTRAP_SERVERS` |
| elastic   | `ELASTICSEARCH_URL`, `ELASTIC_URL` |

**Laravel** gets the conventional vars above **plus** its native discrete keys:
`DB_CONNECTION` (`pgsql`/`mysql`), `DB_HOST`/`DB_PORT`/`DB_DATABASE`/
`DB_USERNAME`/`DB_PASSWORD`, and `REDIS_HOST`/`REDIS_PORT`.

**All frameworks:**

| Service   | Variables |
| --------- | --------- |
| (all)     | `NOWORRIES_<SERVICE>_HOST`, `NOWORRIES_<SERVICE>_PORT` (framework-agnostic) |
| (app)     | port env: `SERVER_PORT` (Spring) / `ASPNETCORE_HTTP_PORTS` (.NET) / `PORT` (the rest), or `app.port_env` |

If both postgres and mysql are declared, `DATABASE_URL` is the last one wired —
use the per-part vars (`PGHOST` vs `MYSQL_HOST`) to disambiguate.

