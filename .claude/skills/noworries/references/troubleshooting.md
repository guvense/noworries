# noworries — troubleshooting

_Reference for the `noworries` skill. Read this when a run fails for a reason that isn't an app-side assertion._

## Troubleshooting

| Symptom | Look at / do |
| ------- | ------------ |
| `NOT READY`, app-side assertion fails | `.noworries/results.json` (expected vs actual) → fix code |
| `app did not become ready` | `.noworries/app.log` (stack trace); raise `app.ready_timeout` |
| `containers did not become healthy` / timeout on first run | image still pulling — re-run with `--timeout 600` |
| `docker compose up failed` / `daemon not reachable` | start Docker; check the image tag exists |
| Kafka/ES check connection error | the service is slow to become healthy — raise `--timeout` |
| `${VAR} is not set` warning, auth 401 | add the value to `.noworries.env` or `export` it |
| App crashes on first RabbitMQ `connect()` (ECONNRESET) | broker just started — add a short connect-retry loop in the app's AMQP client (normal practice) |
| `npm install` fails `EACCES` on `~/.npm/_cacache` | stale root-owned cache — install with `npm install --cache /tmp/npm-$$` (or `sudo chown -R $(whoami) ~/.npm`) |
| `snapshot` FAILs "golden … missing" on first run | expected — run once with `--update-snapshots` to write the golden, then it diffs |
| `unknown field` on a check | run `noworries spec` for the exact field names (e.g. rabbitmq uses `expect_messages`, not `queue_length`) |
