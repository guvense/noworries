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
| `mariadb` service never becomes healthy (`running/unhealthy`) | needs noworries ≥ 0.12 — MariaDB 11 ships no `mysqladmin`, older builds probe with it and never pass |
| `elastic assertion but no Elasticsearch/OpenSearch service` | declare `elasticsearch` or `opensearch` in `services:` (an `opensearch` container satisfies `elastic:` checks from 0.12) |
| `could not run grpcurl` | `brew install grpcurl` (or add it to `PATH`) — the `grpc:` check shells out to it |
| gRPC check can't reach the app | use `${NOWORRIES_APP_PORT}` in `grpc.target` (0.14+). An app serving HTTP *and* gRPC needs its own port: set `app.env: { GRPC_PORT: "50551" }` and use `${GRPC_PORT}` |
| App crashed connecting to MySQL/Cassandra/RabbitMQ at startup | container-healthy ≠ protocol-ready — add a connect-retry loop in the app |
| ClickHouse check finds nothing though the app wrote rows | the app wrote to `default`: the HTTP interface ignores the user's default DB. Use `${CLICKHOUSE_DSN}` (or `?database=noworries` / `noworries.<table>`) |
| ClickHouse HTTP 403 | bare `CLICKHOUSE_URL` carries no credentials — use `${CLICKHOUSE_DSN}` or send basic auth `noworries`/`noworries` |
| MSSQL login failed | SA password is `Noworries!Pass1` (`${MSSQL_PASSWORD}`); no database is created, so connect to `master` |
| .NET app listens on 5000, health probe times out | `Properties/launchSettings.json` overrides the port in Development — keep `dotnet run --no-launch-profile` (0.14+ default) or set `ASPNETCORE_ENVIRONMENT=Production` |
