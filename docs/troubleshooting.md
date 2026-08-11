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

### `/noworries` command not found in Claude Code

Run `noworries install-command` (global) or `noworries install-command
--project`. Confirm the file exists at `~/.claude/commands/noworries.md`, and
that `noworries` is on your `PATH`.

### `noworries: command not found`

The binary isn't on your `PATH`. If installed via `cargo`/`curl|sh`, ensure the
install dir is on `PATH`:

```bash
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
```
