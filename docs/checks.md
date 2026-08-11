# Checks

A check describes an observable behaviour and the assertions that prove it. A
check can mix assertion types; it passes only if **every** assertion passes.
Assertions run in this order so that actions happen before observations:

1. HTTP request (`request` + `expect`)
2. Kafka `produce`
3. DB assertion (`db`) — retries briefly for eventual consistency
4. Redis assertion (`redis`) — retries briefly
5. Kafka `expect_message`

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
- DB assertions **retry for a few seconds** when the check also has an async
  trigger (an HTTP request or a Kafka produce), so a consumer/handler has time
  to write.

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
