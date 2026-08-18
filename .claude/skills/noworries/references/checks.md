# noworries — check reference

_Reference for the `noworries` skill. Read this when writing or fixing `checks:` in `noworries.yml`. `noworries spec` is authoritative when it disagrees with this file._

## Check reference

A check may combine assertion types; it passes only if **all** pass. Order
within a check: **seed** (mysql/mongodb `seed`) → **trigger** (`request`,
`kafka.produce`, `scenario`) → **observe** (all queries/verifications). So "seed
data → hit the API → check what changed" works.

```yaml
version: 1
services: [postgres, redis, kafka, elastic, mysql, mongodb]
app:                          # optional; auto-detected from mvnw/gradlew
  start: "./mvnw spring-boot:run"
  health: "/actuator/health"
  ready_timeout: 180
auth:                         # optional; applied to every request. ${VAR} from .noworries.env
  login: { request: { method: POST, path: /auth/login, body: { username: "${U}", password: "${P}" } }, token_from: "$.accessToken" }
  # or: bearer: { token: "${API_TOKEN}" } | basic: { username: "${U}", password: "${P}" } | api_key: { header: "X-API-Key", value: "${K}" }
checks:
  # One check can span services:
  - name: "create order: 201, persists, caches, emits, indexes"
    tags: [orders]
    request: { method: POST, path: /orders, body: { sku: "ABC", qty: 2 } }
    expect:  { status: 201, body_contains: { sku: "ABC" } }
    db:      { query: "SELECT status FROM orders WHERE sku='ABC'", expect_row: { status: "PENDING" }, expect_row_count: 1 }
    redis:   { key: "cache:order:ABC", expect_exists: true, expect_value_contains: { status: "PENDING" } }
    kafka:   { expect_message: { topic: "order-events", contains: { type: "OrderCreated" }, timeout_ms: 5000 } }
    elastic: { index: "orders", doc_id: "ABC", expect_source_contains: { status: "PENDING" }, query: { term: { sku: "ABC" } }, expect_hits: 1 }

  # Kafka consumer feature (produce to trigger, verify the effect):
  - name: "OrderCreated is consumed"
    kafka: { produce: { topic: "orders", key: "X1", message: { type: "OrderCreated", sku: "X1" } } }
    db:    { query: "SELECT count(*) n FROM orders WHERE sku='X1'", expect_row: { n: 1 } }

  # MySQL seed -> request -> verify:
  - name: "reprice updates the row"
    mysql:   { seed: ["INSERT INTO orders(sku,price) VALUES('P',100)"], query: "SELECT price FROM orders WHERE sku='P'", expect_row: { price: 120 } }
    request: { method: POST, path: /orders/P/reprice, body: { price: 120 } }
    expect:  { status: 200 }

  # MongoDB seed (insert/update/delete) -> verify (find/count):
  - name: "order document is updated"
    mongodb:
      database: "app"
      collection: "orders"
      seed: [ { insert: { document: { sku: "M1", status: "NEW" } } } ]
      find: { sku: "M1" }
      expect_doc_contains: { status: "PROCESSED" }
      expect_count: 1
    request: { method: POST, path: /orders/M1/process }
    expect:  { status: 200 }

  # Elasticsearch template + ops:
  - name: "reindex writes the mapping"
    elastic:
      index: "orders"
      template: { name: "orders-tmpl", body: { index_patterns: ["orders*"], template: { mappings: { properties: { sku: { type: keyword } } } } } }
      operations: [ { insert: { id: "E1", document: { sku: "E1", status: "PENDING" } } } ]
      doc_id: "E1"
      expect_source_contains: { status: "PENDING" }
```

Field summary: **http** `request{method,path,headers,body}` + `expect{status,body_contains,max_ms}` (max_ms = latency budget) ·
**db/mysql** `query`, `expect_row` (deep subset on first row), `expect_row_count`; `mysql.seed` = SQL run before the request. `db:` queries **any Postgres-wire service** — `postgres`, `timescaledb` or `cockroachdb` (the last as `root`/`defaultdb`) ·
**mongodb** `database`, `collection`, `seed[{insert|update{filter,set}|delete}]`, `find`, `expect_doc_contains`, `expect_count` ·
**redis** `key`, `expect_exists`, `expect_value`, `expect_value_contains` ·
**kafka** `produce{topic,key,message}`, `expect_message{topic,contains,timeout_ms}` ·
**scenario** `kind` (burst|concurrent|duplicates|out_of_order), `kafka{topic,key,message}`, `count`, `concurrency`, `keys`, `rate_per_sec`, `expect_throughput_per_sec` (edge-case load trigger; verify with observe counts) ·
**external_calls** `[{external,method,path,body_contains,times}]` (assert the app called a mocked external — needs `externals[].mock`) ·
**logs** `contains[]`, `absent[]` (grep `.noworries/app.log`) ·
**elastic** `index`, `template{name,body,legacy}`, `operations[{insert|update|delete}]`, `doc_id`, `expect_exists`, `expect_source_contains`, `query`, `expect_hits` ·
**rabbitmq** `queue`, `vhost`, `expect_exists`, `min_messages`, `expect_messages` (queue depth via management API) ·
**clickhouse** `query`, `expect_value`, `expect_row`, `expect_rows` (HTTP SQL; the check queries the `noworries` database — make sure the app wrote there, see `services-and-env.md`. No `FORMAT` clause needed) ·
**cassandra** `query`, `expect_value`, `expect_row`, `expect_rows` (CQL via cqlsh; also Scylla). **No `keyspace` field** — qualify tables in the query: `SELECT ... FROM app.orders` ·
**email** `to` (matches To/Cc/Bcc), `from`, `subject_contains`, `body_contains` (text or HTML part), `expect_count`, `expect_none`, `timeout_ms` — reads the ephemeral `smtp` sink's mailbox; retry-aware, since mail is usually queued ·
**s3** `bucket` + `key` (one object) or `prefix` (a listing), `expect_exists`, `expect_count`, `content_type`, `min_size`, `body_contains`, plus `endpoint`/`region`/`access_key`/`secret_key` to point at a real bucket instead of the `minio` service ·
**security** `path`, `method`, `body`, `require_auth`, `reject_bad_input`, `no_error_leak`, `require_headers[]` (defensive abuse-case probes of the app under test) ·
**graphql** `path`, `query`, `variables`, `expect_data`, `expect_no_errors` ·
**metrics** `path`, `metric`, `labels`, `expect` (Prometheus). `expect` is a comparison string: `">= 1"`, `"> 0"`, `"<= 10"`, `"< 5"`, `"== 5"` (also `"= 5"`); a bare number means `== n` ·
**snapshot** `file`, `ignore[]` (golden diff of the check's response). **First run writes the golden and passes only with `--update-snapshots`; later runs diff against it.** `ignore` items are a top-level key (`createdAt`) or a dotted path (`$.a.b` / `a.b`), blanked before comparing ·
**schema** `table`, `has_columns[]`, `columns{col:type}` — types are whatever that backend's `information_schema.data_type` returns: Postgres `character varying` / `integer` / `timestamp without time zone`; MySQL `varchar` / `int` / `datetime`; SQL Server `nvarchar` / `int` / `datetime2` (never `varchar(64)` — no length) ·
**sse** `path`, `contains` (**a JSON object**, deep-subset against each event — `contains: { type: "OrderCreated" }`, not a bare string), `timeout_ms` · **websocket** `url` (relative path → `ws://<app>`), `send`, `expect_message` (JSON subset), `timeout_ms` ·
**grpc** `target`, `method`, `data`, `expect_contains`, `protos[]`, `import_paths[]` (needs `grpcurl` on PATH; paths resolve relative to the `noworries.yml`) ·
**traces** `query_url`, `service`, `operation`, `tags`, `min_count` (query Jaeger/Tempo for OTel spans).

Beyond checks: top-level **`setup`** = shell commands (migrations/fixtures, e.g.
`./mvnw flyway:migrate`) run with the app's env after infra is up, before the app
starts. **`externals[].mock`** stands up an in-process fake (stubs + records
calls) so you can assert outbound behaviour with `external_calls` instead of
hitting a real sandbox. Reports: `--junit <path>` / `--html <path>` for CI.

### Newer check types — examples

(Run `noworries spec` for the authoritative shapes; these show the common cases.)

```yaml
checks:
  # graphql — POST a query/mutation, assert on data (path default /graphql)
  - name: "order query"
    graphql: { path: /graphql, query: "query { order(id:1){status} }", expect_data: { order: { status: "PENDING" } } }

  # metrics — scrape Prometheus, match a series by labels, compare its value
  - name: "request counted"
    request: { method: POST, path: /orders, body: { sku: "A" } }
    expect:  { status: 201 }
    metrics: { path: /actuator/prometheus, metric: http_server_requests_seconds_count, labels: { status: "201" }, expect: ">= 1" }

  # sse — read an event stream until a matching event
  - name: "OrderCreated streamed"
    sse: { path: /events, contains: { type: "OrderCreated" }, timeout_ms: 5000 }

  # websocket — connect, optionally send, await a matching message (ws:// or relative path)
  - name: "subscription pushes update"
    websocket: { url: "/ws", send: { subscribe: "orders" }, expect_message: { type: "OrderCreated" }, timeout_ms: 5000 }

  # grpc — via grpcurl on PATH (reflection or protos:[]/import_paths:[])
  - name: "GetOrder returns the order"
    grpc: { target: "127.0.0.1:${NOWORRIES_APP_PORT}", method: "orders.OrderService/GetOrder", data: { id: "ABC" }, expect_contains: { status: "PENDING" } }

  # traces — app exports to Jaeger/Tempo; query its HTTP API for matching spans
  - name: "request produced a trace"
    request: { method: POST, path: /orders, body: { sku: "A" } }
    expect:  { status: 201 }
    traces: { query_url: "http://127.0.0.1:16686/api/traces", service: "orders-service", operation: "POST /orders", tags: { "http.status_code": "201" }, min_count: 1 }

  # rabbitmq — inspect a queue via the management API (default rabbitmq:3.13-management)
  - name: "publishing an order enqueues a job"
    request:  { method: POST, path: /orders, body: { sku: "A" } }
    expect:   { status: 202 }
    rabbitmq: { queue: "order-jobs", min_messages: 1 }   # or expect_messages / expect_exists / vhost

  # clickhouse — run SQL over the HTTP interface
  - name: "event recorded in analytics store"
    request:    { method: POST, path: /events, body: { user_id: 42 } }
    expect:     { status: 202 }
    clickhouse: { query: "SELECT count() FROM events WHERE user_id = 42", expect_value: 1 }

  # cassandra — run CQL via cqlsh (also Scylla)
  - name: "order written to the wide-column store"
    cassandra: { query: "SELECT count(*) FROM app.orders WHERE id = 42", expect_value: 1 }

  # email — the app sent mail (needs an `smtp` service; Mailpit under the hood)
  - name: "ordering emails the customer"
    request: { method: POST, path: /orders, body: { sku: "NET-1", email: "user@example.com" } }
    expect:  { status: 201 }
    email:
      to: user@example.com
      subject_contains: "Order confirmed"
      body_contains: "NET-1"
      expect_count: 1

  # s3 — the upload actually landed (needs a `minio` service)
  - name: "the invoice is stored"
    request: { method: POST, path: /invoices, body: { order: "NET-1" } }
    expect:  { status: 201 }
    s3:
      bucket: invoices
      key: "2026/NET-1.pdf"
      content_type: application/pdf
      min_size: 100

  # security — defensive abuse-case probes of the app under test (localhost only)
  - name: "orders endpoint is hardened"
    security:
      path: /orders
      method: POST
      body: { sku: "ABC", qty: 1 }
      require_auth: true        # auth stripped -> must be 401/403
      reject_bad_input: true    # hostile/malformed input must not 5xx
      no_error_leak: true       # no stack traces / DB errors in responses
      require_headers: [X-Content-Type-Options]
```

## Naming traps

- **Row counts differ by backend.** `db:`/`mysql:` use `expect_row_count`;
  `clickhouse:`/`cassandra:` use `expect_rows`. Run `noworries validate` if
  unsure — it names the accepted fields.
- **`cassandra:` has no `keyspace`.** Qualify the table in the query
  (`FROM app.orders`).
- **`sse.contains` is an object**, not a string.

## The app's port is nameable

Relative paths (`sse.path`, `websocket.url`, `graphql.path`, `metrics.path`)
resolve against the app automatically. For a target you must give in full —
`grpc.target`, an absolute `ws://` URL — interpolate the port instead of pinning
one:

| Variable | Value |
| -------- | ----- |
| `${NOWORRIES_APP_PORT}` | the port noworries assigned the app |
| `${NOWORRIES_APP_HOST}` / `${NOWORRIES_APP_URL}` | `127.0.0.1` / `http://127.0.0.1:<port>` |
| `${PORT}` / `${SERVER_PORT}` | the same port under the framework's own name (`app.port_env` wins) |
| `${NOWORRIES_<SERVICE>_HOST}` / `_PORT` | a container's mapped host/port (`NOWORRIES_KAFKA_PORT`, …) |

```yaml
grpc: { target: "127.0.0.1:${NOWORRIES_APP_PORT}", method: "orders.OrderService/GetOrder" }
```

A gRPC-only service listens on the port noworries handed it, so
`${NOWORRIES_APP_PORT}` is the target. An app that serves **both** HTTP and gRPC
needs a second port that noworries doesn't assign — set it yourself
(`app: { env: { GRPC_PORT: "50551" } }`) and use `${GRPC_PORT}`.

Two more gRPC traps: server **reflection only works with statically compiled
proto stubs** — services registered from descriptors built at run time leave
grpcurl reporting "server does not expose service", so pass
`protos: [orders.proto]` (resolved relative to the `noworries.yml`); and
`grpcurl` must be on `PATH` (`brew install grpcurl`).

## Identities: `auth`, `users` and `as`

`auth:` authenticates **noworries → the app** for every check. Pick the block
that matches the app: `login` (run a login request, extract a token), `bearer`,
`basic`, `api_key`, or `oidc`.

`oidc:` is the one to reach for behind Keycloak / Auth0 / Cognito / Entra —
noworries fetches the token itself instead of you hand-rolling `login`:

```yaml
auth:
  oidc:
    issuer: "https://id.example.com/realms/app"   # discovery; or token_url: ...
    client_id: orders-app
    client_secret: "${OIDC_SECRET}"               # omit for a public client
    scope: "orders:read orders:write"
    # audience: "https://api.example.com"   # Auth0 needs it for a JWT
    # grant: password + username/password  # resource-owner flow
    # client_auth: basic                   # Cognito wants the secret in a header
```

For **RBAC**, declare the roles once and pick one per check — this is the whole
point of `users:` + `as:`, and the only way to assert "this role may, that role
may not":

```yaml
users:
  admin:  { oidc: { issuer: "${ISSUER}", client_id: admin-cli, client_secret: "${ADMIN_SECRET}" } }
  reader: { bearer: { token: "${READER_TOKEN}" } }
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

Each identity is resolved once, before the checks run. A check without `as:`
uses the top-level `auth:`. An `as:` naming an undeclared user fails at parse
time — deliberately, since falling back to the default identity would make an
RBAC check pass for the wrong reason. The identity covers the whole check, not
only `request:` — `graphql`, `sse`, `websocket`, `grpc`, `metrics`, `traces` and
the `security` probes all send it.

## Secrets and auth in a spec

Reference `${VAR}` in `auth`/`externals`/headers/body; put the values in a
**`.noworries.env`** (gitignored automatically). Derive auth from
`application.properties`/code where possible; **ask the user** for the login URL
/ credentials / API key you can't derive. If auth lives on a **separate server**
(not the app), use an absolute URL for `auth.login.request.path` (e.g.
`${AUTH_URL}/oauth/token`) — a relative path hits the app under test. For an
upstream the app *calls*, use `externals` (see `externals.md`).

_Run-level behaviour (shared state between checks, image pulls, teardown) is in
`SKILL.md`._
