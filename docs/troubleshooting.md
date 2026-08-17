# Troubleshooting

### "the Docker daemon is not reachable"

Docker isn't running. Start Docker Desktop (macOS) or `dockerd` (Linux) and try
again. `noworries` runs a preflight and stops early with this message rather
than half-starting.

### Image pull fails / `403 Forbidden`

Your Docker can't reach the registry for `postgres` / `apache/kafka` / `redis`.
Check connectivity or your registry mirror. On a normal machine with Docker
Desktop this just works.

### "could not determine how to start the app"

No framework was auto-detected and `app.start` isn't set. Add it to
`noworries.yml`:

```yaml
app:
  start: "./mvnw spring-boot:run"
```

### App "did not become ready within Ns"

- Check `.noworries/app.log` for a stack trace.
- The first Maven/Gradle build can be slow — raise the budget with
  `app.ready_timeout` (seconds) and/or the whole-run `--timeout`.
- If your app has no `/actuator/health`, set `app.health` to a path that returns
  2xx, or `health: none` to just wait for the port to open.

### A DB/Redis check fails even though the write happens

If the write is asynchronous (a Kafka consumer, a background handler), give it
room: `noworries` already retries DB/Redis assertions for a few seconds when the
check has an HTTP request or a Kafka produce. If your effect is slower than that,
split it into an explicit produce + assertion, or widen the trigger.

### Kafka check can't connect

Kafka is advertised at `localhost:<fixed host port>`. Make sure your app reads
`SPRING_KAFKA_BOOTSTRAP_SERVERS` (Spring Boot gets it automatically) or the
`NOWORRIES_KAFKA_HOST`/`NOWORRIES_KAFKA_PORT` vars. Kafka takes longer than
Postgres/Redis to become healthy; if it times out, raise `--timeout`.

### Containers left running after a crash

Teardown is guaranteed on normal exit, failure, and Ctrl-C. If a hard kill
orphaned something, remove it manually:

```bash
docker compose -f .noworries/compose.test.yml -p <project-name> down -v
```

The project name is printed at the start of each run (`noworries-...`).

### `/noworries` skill not found in Claude Code

Run `noworries install-command` (global) or `noworries install-command
--project`. Confirm `~/.claude/skills/noworries/SKILL.md` exists (with a `references/`
folder next to it), and
that `noworries` is on your `PATH`. (Older installs wrote
`~/.claude/commands/noworries.md`; that still works, but you can delete it once
the skill is installed.)

### `noworries: command not found`

The binary isn't on your `PATH`. If installed via `cargo`/`curl|sh`, ensure the
install dir is on `PATH`:

```bash
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
```

### A service never becomes healthy

`mariadb` needs its own readiness probe: MariaDB 11 removed the `mysql*`
compatibility symlinks, so a `mysqladmin ping` healthcheck can never pass and
the container sits in `running/unhealthy` until the run times out. Fixed in
0.12 (`mariadb-admin ping`, falling back to `mysqladmin` for MariaDB 10.x). On
an older build, pin `mariadb:10.11` or declare `mysql` instead.

### `elastic assertion but no Elasticsearch/OpenSearch service is running`

Declare `elasticsearch` or `opensearch` under `services:`. From 0.12 an
`opensearch` container satisfies `elastic:` checks — it answers the same
`_doc`/`_search`/`_refresh` API. Before that the app could write through the
injected `OPENSEARCH_URL` while the check refused to read it back.

### The app crashes on its first connection, but the container is healthy

Container-healthy is not protocol-ready. RabbitMQ accepts TCP before the AMQP
listener will complete a handshake, MySQL/MariaDB are still initializing on
first boot, and Cassandra/Scylla take ~30s before CQL answers. Wrap the app's
initial connect in a short retry loop (a few attempts, a couple of seconds
apart) — standard client practice, and the usual cause of a startup crash in an
otherwise green run.

### A gRPC check can't reach the app

There is no `${NOWORRIES_GRPC_PORT}`: relative paths (`sse`, `websocket`,
`graphql`, `metrics`) resolve against the app's assigned HTTP port, but
`grpc.target` is used verbatim. Serve gRPC on a port you set yourself
(`app.env: { GRPC_PORT: "50551" }`) and hardcode the target. If grpcurl reports
`server does not expose service`, reflection isn't usable — that happens when
services are registered from dynamically built descriptors — so pass
`protos: [orders.proto]` (paths resolve relative to the `noworries.yml`).

