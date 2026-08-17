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

**RabbitMQ readiness:** the container is only reported healthy once the AMQP
listener accepts connections, but an app's *first* `connect()` can still race a
just-started broker — Node/Python/Go AMQP clients should wrap the initial
connect in a short retry loop (this is normal broker practice, not a noworries
quirk).

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

