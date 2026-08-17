# Checks

A check describes an observable behaviour and the assertions that prove it. A
check can mix assertion types; it passes only if **every** assertion passes.
Assertions run in this order so that actions happen before observations:

1. HTTP request (`request` + `expect`, incl. `max_ms` latency)
2. Kafka `produce`
3. `scenario` — edge-case load/timing flood (burst / concurrent / duplicates / out_of_order)
4. DB assertion (`db`) — retries for eventual consistency (window scales with a scenario's size)
5. Redis assertion (`redis`) — retries briefly
6. Kafka `expect_message`
7. `external_calls` — assert the app called a mocked external (retry-aware)
8. `logs` — assert the app log contains / omits patterns (retry-aware)
9. Protocol, datastore, security & observability types — `graphql`, `metrics`, `snapshot`, `schema`, `clickhouse`, `rabbitmq`, `cassandra`, `security`, `sse`, `websocket`, `grpc`, `traces` (see below)

## HTTP

```yaml
- name: "create order returns 201"
  request:
    method: POST
    path: /orders
    headers: { X-Trace: "abc" }     # optional
    body: { sku: "ABC123", qty: 2 } # JSON; sent with content-type: application/json
  expect:
    status: 201
    body_contains: { sku: "ABC123", status: "PENDING" }   # deep subset match
```

- `expect.status` — exact HTTP status.
- `expect.body_contains` — a **deep partial match**: every field you list must
  appear in the response (extra fields are ignored). Works on objects and array
  prefixes.
- `expect.max_ms` — latency budget: fail if the response took longer than this
  many milliseconds (`expect: { status: 200, max_ms: 300 }`).

## Database (Postgres)

```yaml
  db:
    query: "SELECT status FROM orders WHERE sku = 'ABC123'"
    expect_row: { status: "PENDING" }   # first row must contain these fields
    # or:
    expect_row_count: 1
```

- `expect_row` — deep subset match against the **first** row.
- `expect_row_count` — exact number of rows returned.
- Scalars compare loosely (a YAML number matches a text column of the same
  value), so you don't have to fight type coercion.
- DB assertions **retry for eventual consistency** when the check also has an
  async trigger (an HTTP request, a Kafka produce, or a `scenario` flood), so a
  consumer/handler has time to write. The retry window is ~6s normally and grows
  with a scenario's message count (capped at 120s) so big floods have time to
  drain.

## Kafka

Two independent capabilities. Use `produce` to *trigger* a consumer the feature
added, and verify the effect with a DB/Redis assertion in the same check. Use
`expect_message` to verify a *producer* the feature added.

```yaml
- name: "OrderCreated event is consumed and persisted"
  kafka:
    produce:
      topic: "orders"
      key: "ABC123"                       # optional
      message: { type: "OrderCreated", sku: "ABC123", qty: 2 }
  db:
    query: "SELECT count(*) AS n FROM orders WHERE sku = 'ABC123'"
    expect_row: { n: 1 }

- name: "processing emits an OrderProcessed event"
  kafka:
    expect_message:
      topic: "order-events"
      contains: { type: "OrderProcessed", sku: "ABC123" }  # deep subset
      timeout_ms: 5000                                     # optional (default 5000)
```

- `produce.message` is serialized to JSON (a plain string is sent as-is).
- `expect_message.contains` is matched (deep subset) against each message on the
  topic until one matches or `timeout_ms` elapses.

## Edge-case scenarios

A single trigger→observe check proves the **happy path**. A data pipeline (Flink,
a Kafka consumer, any read-then-write flow) can pass that and still be wrong under
load: it may drop messages in a burst, corrupt state under concurrent writes to
the same key, double-process duplicates, or mishandle out-of-order arrival. A
check's `scenario:` block replaces the single `kafka.produce` with a **generated
flood** designed to expose exactly those bugs. Verification stays ordinary: the
check's `db` / `mysql` / `mongodb` / `elastic` assertions (usually an exact
**count**) prove the invariant held under load.

```yaml
- name: "burst of 500 orders: nothing dropped"
  scenario:
    kind: burst            # burst | concurrent | duplicates | out_of_order
    count: 500             # logical messages (per-kind default if omitted)
    concurrency: 8         # parallel producer threads (the source of races)
    keys: 500              # distinct keys the load spreads over
    rate_per_sec: 2000     # optional per-producer cap (0/absent = as fast as possible)
    kafka:
      topic: orders-in
      key: "ord-${seq}"                       # template
      message: { id: "${uuid}", amount: "${seq}" }
  db:
    query: "SELECT count(*) AS n FROM orders"
    expect_row: { n: 500 }                    # exactly-once, no loss
```

### The four kinds

| `kind`         | Behaviour | Bug it exposes | Typical verification |
| -------------- | --------- | -------------- | -------------------- |
| `burst`        | Floods `count` messages (optionally rate-limited) over `keys` keys; one producer by default. | Dropped messages, can't keep up. | count `= count`. |
| `concurrent`   | `count` writes over a few `keys` across `concurrency` parallel producers → real races; `${i}` is the per-key version. | Lost updates, duplicate rows, stale final value. | one row per key (`= keys`) and/or final value `= max(${i})`. |
| `duplicates`   | Sends every message **twice** (same key + payload, back-to-back). | Non-idempotent / double-processing. | count `= count` (not `2·count`). |
| `out_of_order` | For each key, emits events in **reverse** arrival order; the true order rides in `${i}`. | Naive last-arrival-wins, bad windowing. | final value `= max(${i})` per key. |

### Knobs

| Field         | Default (per kind) | Meaning |
| ------------- | ------------------ | ------- |
| `kind`        | —                  | Which strategy: `burst`, `concurrent`, `duplicates`, `out_of_order`. |
| `count`       | burst 100; others 50 | Number of logical messages. |
| `concurrency` | burst/dupes/oo 1; concurrent 4 (min 2) | Parallel producer threads. Higher = more contention. |
| `keys`        | burst = `count`; concurrent/oo 1 | Distinct keys the load spreads over. Fewer keys + more concurrency = more contention on one key. |
| `rate_per_sec`| unbounded          | Optional cap on messages/second **per producer**. |
| `expect_throughput_per_sec` | none | Assert the achieved produce throughput was at least this many msgs/second (fails otherwise). |
| `kafka.topic` | —                  | Target topic (required). |
| `kafka.key`   | `k<n>` per strategy | Key template. |
| `kafka.message` | —                | Message template (required), serialized to JSON. |

The scenario assertion always reports the achieved rate (e.g. `~4200 msg/s`);
`expect_throughput_per_sec` turns that into a pass/fail gate.

### Template placeholders

Available in `kafka.key` and any string leaf of `kafka.message`:

- `${seq}` — global 0-based index across the whole flood.
- `${i}` — per-key sequence (a version number for that key).
- `${key}` — the assigned key value.
- `${uuid}` — a plan-unique token (handy for a distinct id field).

### Verifying each kind

```yaml
# concurrent: many writers hammer ONE key — the app must end consistent.
- name: "concurrent updates to a hot key stay consistent"
  scenario:
    kind: concurrent
    count: 50
    keys: 1
    concurrency: 6
    kafka: { topic: orders-in, key: "hot", message: { id: "hot", version: "${i}" } }
  db:
    query: "SELECT count(*) AS n FROM orders WHERE id = 'hot'"
    expect_row: { n: 1 }              # one row, not 50 (no duplicate inserts under race)

# duplicates: idempotency — half the messages are exact repeats.
- name: "duplicate delivery is idempotent"
  scenario:
    kind: duplicates
    count: 100
    kafka: { topic: orders-in, key: "dup-${seq}", message: { id: "dup-${seq}" } }
  db:
    query: "SELECT count(*) AS n FROM orders WHERE id LIKE 'dup-%'"
    expect_row: { n: 100 }            # 100, not 200

# out_of_order: newest event arrives first; app must end on the newest by ${i}.
- name: "out-of-order events converge on the latest"
  scenario:
    kind: out_of_order
    count: 20
    keys: 1
    kafka: { topic: prices-in, key: "P1", message: { id: "P1", version: "${i}" } }
  db:
    query: "SELECT version FROM prices WHERE id = 'P1'"
    expect_row: { version: 19 }       # highest ${i}, despite reverse arrival
```

### How it runs

- Scenarios run in the **trigger** phase (after `request` / `kafka.produce`,
  before the observe assertions), so "flood → observe the effect" works.
- Concurrency is real: the runner spawns `concurrency` threads, each with its own
  Kafka producer, and dispatches messages round-robin — so writes genuinely race.
- Because a flood takes time to drain, the observe assertions' retry window
  **grows with `count`** (≈ 6s + count/50, capped at 120s). For very large bursts,
  also raise the overall `--timeout`.
- A scenario declared without a Kafka service, or with an unknown `kind`, fails
  the check with a clear message (it never silently passes).

Scenarios currently target **Kafka**. The layer is an extensible strategy (the
`EdgeCase` trait in `src/edgecases/`), so new kinds and sinks can be added without
changing the check schema — see [architecture](architecture.md#edgecase-srcedgecases).

## Redis

For features that cache to Redis: do the action, then assert the key.

```yaml
- name: "order is cached after creation"
  request: { method: POST, path: /orders, body: { sku: "C1", qty: 1 } }
  expect:  { status: 201 }
  redis:
    key: "cache:order:C1"
    expect_exists: true
    expect_value_contains: { status: "PENDING" }   # value parsed as JSON, deep subset
    # or exact:
    # expect_value: "PENDING"
```

- `expect_exists` — whether the key should exist (defaults to `true` if you give
  no other expectation).
- `expect_value` — exact value match (loose scalar comparison).
- `expect_value_contains` — parse the value as JSON and deep-subset match.
- Redis assertions also retry briefly for async cache writes.

## Elasticsearch

Supports applying an index template, running insert/update/delete operations
(noworries-run, for setup or triggering), and verifying a document or a search
— so it covers both "noworries writes and checks" and "the app writes, noworries
checks".

```yaml
- name: "order is indexed and searchable"
  # the app indexes the order during this request:
  request: { method: POST, path: /orders, body: { sku: "ABC123", qty: 2 } }
  expect:  { status: 201 }
  elastic:
    index: "orders"
    template:                       # applied before the app starts (ES 7 & 8)
      name: "orders-template"
      # legacy: true                # use the old _template API (ES < 7.8)
      body:
        index_patterns: ["orders*"]
        template:
          mappings:
            properties:
              sku:    { type: keyword }
              status: { type: keyword }
    operations:                     # optional noworries-run ops (setup/trigger)
      - insert: { id: "SEED1", document: { sku: "SEED1", status: "PENDING" } }
      - update: { id: "SEED1", document: { status: "SHIPPED" } }
      - delete: { id: "OLD9" }
    # verify a document:
    doc_id: "ABC123"
    expect_exists: true
    expect_source_contains: { status: "PENDING" }
    # and/or verify via a search (Elasticsearch query DSL):
    query: { match: { sku: "ABC123" } }
    expect_hits: 1
```

- `template` — an index template generated by Claude from the code (or pasted
  from production) and applied by the CLI **before the app starts**, so
  app-created indices get the right mapping. `legacy: true` targets the old
  `_template` API (ES < 7.8); otherwise `_index_template` (ES 7.8+ and 8). ES 7
  and 8 mapping formats differ — the template is written for the declared major.
- `operations` — insert (`PUT /<index>/_doc/<id>` or auto-id `POST`), update
  (`POST /<index>/_update/<id>` with a partial `doc`), delete
  (`DELETE /<index>/_doc/<id>`). All use `refresh=true` so they're immediately
  visible; delete treats a 404 as already-gone.
- `doc_id` + `expect_exists` / `expect_source_contains` — GET the document and
  deep-subset match its `_source`.
- `query` + `expect_hits` — POST a search with your query DSL and assert the hit
  count. noworries issues a `_refresh` on the index **before each search**, so a
  write made by the app or a downstream sink (e.g. a Flink job) is visible
  immediately instead of racing the default 1s refresh interval.
- A `template` on its own is a valid check: noworries asserts the template
  installed (a template-only check no longer fails as "no assertions").
- Verification retries within the check's budget for near-real-time / async
  writes — including when the `elastic` block is the check's only assertion, so
  you don't need an unrelated trigger just to get the retry window.

For an **asynchronous pipeline** (the app/Flink writes to ES out-of-band), gate
on the downstream signal first: `kafka.expect_message` on the output topic (with
`timeout_ms`), then the `elastic` assertion. See the Flink gotchas in
[configuration.md](configuration.md#flink-gotchas-learned-the-hard-way).

The Elasticsearch container runs single-node with security disabled (plain HTTP),
which works for both ES 7 and 8. Declare it as `elastic:8.13.4` /
`elasticsearch:7.17.22` (expands to `docker.elastic.co/elasticsearch/elasticsearch`).

## MySQL

The "seed data → hit the API → check what changed" flow: `seed` statements run
**before** the request, then `query` verifies the result afterwards.

```yaml
- name: "reprice endpoint updates the order"
  request: { method: POST, path: /orders/ABC/reprice, body: { price: 120 } }
  expect:  { status: 200 }
  mysql:
    seed:
      - "INSERT INTO orders (sku, price, status) VALUES ('ABC', 100, 'NEW')"
    query: "SELECT price, status FROM orders WHERE sku = 'ABC'"
    expect_row: { price: 120, status: "REPRICED" }
    expect_row_count: 1
```

`seed` runs before the request regardless of where the `mysql` block appears in
the check.

- `seed` — a list of raw SQL statements (INSERT/UPDATE/DELETE/DDL) executed in
  order **before** the request, so a read-then-update feature has data to act on.
- `query` + `expect_row` (deep subset match on the first row) /
  `expect_row_count` — verification, retried briefly when the check also has a
  request (for async writes).

(The container uses `user/password/db = noworries`. Declare it as `mysql` /
`mysql:8.0` → `mysql:8.4` by default.)

## ClickHouse

Run a `SELECT` over ClickHouse's HTTP interface (port 8123) and check the
result. Uses the built-in HTTP client — no native driver — so it works out of
the box once you declare a `clickhouse` service.

```yaml
- name: "event is recorded in the analytics store"
  request: { method: POST, path: /events, body: { user_id: 42, kind: click } }
  expect:  { status: 202 }
  clickhouse:
    query: "SELECT count() FROM events WHERE user_id = 42"
    expect_value: 1
```

- `query` — any SQL; the output format is set automatically (`FORMAT JSON`).
- `expect_value` — the single scalar result (first column of the first row).
  Comparison is loose across the string/int boundary, because ClickHouse renders
  64-bit integers as JSON strings under `FORMAT JSON` (so `count()` → `"1"`).
- `expect_row` — deep subset match on the first row (`column → value`).
- `expect_rows` — assert the query returned exactly N rows.

Verification is retried briefly when the check also issues a request, so an
asynchronous insert has time to land. (The container uses
`user/password/db = noworries`. Declare it as `clickhouse` /
`clickhouse:<tag>` → `clickhouse/clickhouse-server:24.8` by default.)

## Cassandra / ScyllaDB

Run a CQL query via `cqlsh` inside the container (no native driver — same
in-container approach as SQL Server's `schema`). `SELECT` queries are rewritten
to `SELECT JSON` internally so each row is parsed as JSON.

```yaml
- name: "order is written to the wide-column store"
  request: { method: POST, path: /orders, body: { id: 42 } }
  expect:  { status: 201 }
  cassandra:
    query: "SELECT count(*) FROM app.orders WHERE id = 42"
    expect_value: 1
```

- `query` — any CQL `SELECT`.
- `expect_value` — the single scalar result (single-column query).
- `expect_row` — deep subset match on the first row (`column → value`).
- `expect_rows` — assert exactly N rows. (Note the name: `db:`/`mysql:` call the
  same thing `expect_row_count`.)

There is **no `keyspace` field** — qualify the table in the query
(`FROM app.orders`).

Works identically against ScyllaDB (declare `scylla` / `scylladb`), which ships
the same `cqlsh`. Verification retries briefly for asynchronous writes.

## RabbitMQ

Inspect a queue via RabbitMQ's management HTTP API (port 15672). Uses the
built-in HTTP client — no AMQP driver — so declaring a `rabbitmq` service (the
default image is `rabbitmq:3.13-management`, which enables the API) is all it
takes. noworries resolves the management port's host mapping automatically.

```yaml
- name: "publishing an order enqueues a job"
  request: { method: POST, path: /orders, body: { sku: ABC } }
  expect:  { status: 202 }
  rabbitmq:
    queue: "order-jobs"
    min_messages: 1
```

- `queue` — the queue name to inspect.
- `vhost` — virtual host (default `/`).
- `expect_exists` — assert the queue exists (default `true`); set `false` to
  assert absence.
- `min_messages` / `expect_messages` — assert the queue holds at least / exactly
  N messages (ready + unacknowledged).

Verification is retried briefly when the check also issues a request, so an
asynchronous publish has time to land. (The container uses
`user/password = noworries`. A non-management image has no API — use the default
`rabbitmq:3.13-management` or another `*-management` tag.)

## Security (abuse-case checks)

Probe an endpoint of the app under test with hostile input and assert it behaves
safely. This is a **defensive** hardening check: it only ever talks to the
ephemeral app noworries started, and every assertion is that the app *reacts
safely* (rejects, or handles without crashing/leaking) — it does not exploit
anything.

```yaml
- name: "orders endpoint is hardened"
  security:
    path: /orders                 # optional (default: the check's request.path)
    method: POST                  # optional (default: the check's request.method / GET)
    body: { sku: "ABC", qty: 1 }  # optional baseline the input probes mutate
    require_auth: true            # request with auth stripped must be 401/403
    reject_bad_input: true        # hostile/malformed input must not 5xx; malformed body → 4xx
    no_error_leak: true           # responses must not leak stack traces / DB errors
    require_headers: [X-Content-Type-Options, X-Frame-Options]
```

- `require_auth` — resends the request with the run's `auth` stripped; the
  endpoint must reject it (401/403). Catches an endpoint that forgot its guard.
- `reject_bad_input` — sends a malformed body, an oversized field, and classic
  abuse strings (SQL-ish, script-ish, path-traversal, template-injection) placed
  in the body. Every probe must come back **without a 5xx** (a server crash is a
  real defect), and the malformed body must be a **4xx**. A safely-ignored probe
  that returns 2xx is fine — the assertion is *no crash*, not *rejected*.
- `no_error_leak` — scans the probe responses for leaked internals (stack
  traces, `SQLSTATE`, `Traceback`, `panic:`, framework signatures, …) and fails
  if any appear.
- `require_headers` — asserts the listed response headers are present
  (case-insensitive), e.g. `X-Content-Type-Options`, `Strict-Transport-Security`.

Set at least one probe. The check needs a running app (it has no effect on a
Flink-only run). Pair it with the normal `request`/`expect` in the same or a
neighbouring check to cover both the happy path and the abuse path.

## External calls (mocks)

When an external declares a [`mock`](configuration.md#mocking-an-external-mock),
noworries records every request the app makes to it. `external_calls` asserts the
app actually reached out — a request-level assertion on the app's *outbound*
behaviour, retried for calls the app makes asynchronously.

```yaml
- name: "creating an order charges the customer"
  request: { method: POST, path: /orders, body: { sku: "A", amount: 100 } }
  expect:  { status: 201 }
  external_calls:
    - external: payments          # matches externals[].name
      method: POST                # optional; omit to match any method
      path: /charge               # exact path
      body_contains: { amount: 100 }   # deep-subset match on the recorded JSON body
      times: 1                    # exact count; omit for "at least one"
      timeout_ms: 8000            # optional; how long to wait for an async call (default ~6s window)
```

## Logs

Assert the app's captured log (`.noworries/app.log`) contains — or is free of —
substrings. Great for "the handler logged this" and "no stack trace / no ERROR".
Retried within the same eventual-consistency window as the DB observers, so a
line the app writes just after responding still counts.

```yaml
- name: "processing logs success and no errors"
  kafka: { produce: { topic: orders, key: "X1", message: { sku: "X1" } } }
  logs:
    contains: ["OrderProcessed X1"]     # all must appear
    absent:   ["ERROR", "Exception"]    # none may appear
```

## Protocol & observability assertions

These extend a check beyond HTTP/DB. Each is an independent assertion type
(pluggable via `src/checks/`), so a check can combine them with the others.

Assertions that **act** on the app — a GraphQL mutation, a gRPC call, a
WebSocket `send` — run in the check's trigger slot, before the observers
(`db`, `mysql`, `mongodb`, `redis`, `elastic`, `kafka.expect_message`, `logs`).
So one check can call a mutation and assert its effect in Postgres or Kafka;
the observers keep their retry budget for the eventual-consistency window.
(Before 0.12 they ran last, and such a check had to be split in two.) An acting
assertion with nothing else to observe — a lone `graphql:` query — is treated
as the observation and keeps the full retry budget.

### GraphQL (`graphql`)

POST a query/mutation and assert on `data` / `errors`.

```yaml
- name: "order query returns status"
  graphql:
    path: /graphql                 # default /graphql; relative → app, or absolute URL
    query: "query($id: ID!) { order(id: $id) { status } }"
    variables: { id: "ABC" }       # optional; values interpolate ${VAR}
    expect_data: { order: { status: "PENDING" } }   # deep-subset on data
    expect_no_errors: true         # default: fail if errors[] is non-empty
```

### Prometheus metrics (`metrics`)

Scrape a metrics endpoint, match one series by name + labels, compare its value.

```yaml
- name: "the request was counted"
  request: { method: POST, path: /orders, body: { sku: "A" } }
  expect:  { status: 201 }
  metrics:
    path: /actuator/prometheus     # default /metrics
    metric: http_server_requests_seconds_count
    labels: { status: "201", uri: "/orders" }   # subset match
    expect: ">= 1"                 # >=, <=, >, <, ==, = ; omit to just require presence
```

### Snapshot / golden (`snapshot`)

Capture the check's HTTP `request` response body and diff it against a saved
golden file. Run once with `--update-snapshots` to create/refresh the golden.

```yaml
- name: "order response shape is stable"
  request: { method: GET, path: /orders/ABC }
  snapshot:
    file: snapshots/order.json     # relative to the project dir
    ignore: [ "$.id", "$.createdAt" ]   # blank volatile fields before comparing
```

- **First run:** if the golden file doesn't exist yet, the check **FAILs** with
  "golden … missing" — run once with `--update-snapshots` to write it, then
  normal runs diff the response against it. (There's no separate scaffold step;
  `--update-snapshots` creates the file, and also refreshes it after an
  intentional response change.)
- **`ignore` semantics:** each item is a **top-level key** (`createdAt`) or a
  **dotted path** (`$.a.b`, `a.b`, or `$.items.0.id`); the field at that path is
  blanked (set to null) in both sides before comparing, so volatile values (ids,
  timestamps) don't cause spurious diffs. A non-numeric segment against an array
  applies to **every element** — `items.createdAt` blanks `createdAt` in each
  item of the `items` list. A missing path is ignored.

### DB schema (`schema`, Postgres / MySQL / SQL Server)

Assert a table's columns (and optionally types) via `information_schema` — uses
Postgres if declared, otherwise MySQL, otherwise SQL Server. On MySQL, `schema`
is the database name; on SQL Server it is likewise treated as the database (a
`USE` clause). Postgres/MySQL connect from the host; SQL Server is queried with
`sqlcmd` inside the container (no host driver needed). Type names are matched
loosely, including SQL Server aliases (`bit`→bool, `nvarchar`→text,
`datetime2`→timestamp, `uniqueidentifier`→uuid).

```yaml
- name: "orders table migrated correctly"
  schema:
    table: orders
    schema: public                 # optional (default public)
    has_columns: [id, sku, status] # must exist
    columns: { qty: int, price: numeric, created_at: timestamp }  # type (loose) match
```

### Server-Sent Events (`sse`)

Open a stream and wait for an event whose JSON `data` matches.

```yaml
- name: "an OrderCreated event is streamed"
  request: { method: POST, path: /orders, body: { sku: "A" } }
  expect:  { status: 201 }
  sse:
    path: /events
    contains: { type: "OrderCreated" }   # deep-subset on an event's data
    timeout_ms: 5000
```

### WebSocket (`websocket`)

Connect, optionally send a message, and await a matching message.

```yaml
- name: "subscription pushes the update"
  websocket:
    url: "ws://127.0.0.1:${SERVER_PORT}/ws"   # or a relative path → ws://app
    send: { subscribe: "orders" }             # optional; JSON or string
    expect_message: { type: "OrderCreated" }  # deep-subset on a received message
    timeout_ms: 5000
```

### gRPC (`grpc`)

Calls a method via **`grpcurl`** (must be on PATH; needs server reflection, or
give `protos`/`import_paths`). Keeps noworries free of a heavy gRPC/proto stack.

```yaml
- name: "GetOrder returns the order"
  grpc:
    target: "127.0.0.1:50551"                    # host:port — see the note below
    method: "orders.OrderService/GetOrder"
    data: { id: "ABC" }                          # request JSON
    expect_contains: { status: "PENDING" }       # deep-subset on the response
    plaintext: true                              # default true (no TLS)
    # protos: [orders.proto]                     # if not using reflection
    # import_paths: [proto]
```

Three things to know:

- **The port is yours to fix.** Relative paths (`sse`, `websocket`, `graphql`,
  `metrics`) resolve against the app's assigned HTTP port, but `grpc.target` is
  used verbatim and there is no `${NOWORRIES_GRPC_PORT}` to interpolate. Set the
  port yourself — `app: { env: { GRPC_PORT: "50551" } }` — and hardcode it.
- **Reflection needs statically compiled stubs.** Registering services from
  descriptors you build at run time leaves grpcurl reporting `server does not
  expose service`; pass `protos: [orders.proto]` instead.
- **`protos`/`import_paths` resolve relative to the `noworries.yml`.**

### OpenTelemetry traces (`traces`)

The app exports spans to a trace backend (Jaeger/Tempo); noworries queries the
backend's HTTP API and asserts matching traces exist.

```yaml
- name: "the request produced a trace"
  request: { method: POST, path: /orders, body: { sku: "A" } }
  expect:  { status: 201 }
  traces:
    query_url: "http://127.0.0.1:16686/api/traces"   # Jaeger query API
    service: "orders-service"
    operation: "POST /orders"      # optional
    tags: { "http.status_code": "201" }   # optional (subset)
    min_count: 1                   # default 1
```

## Reading results

Every run prints a `=== noworries results ===` block and writes
`.noworries/results.json`:

```json
{
  "ready": false,
  "passed": 1,
  "failed": 1,
  "total": 2,
  "results": [
    {
      "name": "create order returns 201 and persists",
      "passed": false,
      "assertions": [
        { "kind": "http", "passed": false, "message": "...", "expected": 201, "actual": 500 }
      ]
    }
  ]
}
```

Exit code: `0` when `ready` is true, `1` otherwise. This is what `/noworries`
parses to decide whether to fix and re-run.
