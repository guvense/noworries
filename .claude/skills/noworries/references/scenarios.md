# noworries — edge-case scenarios

_Reference for the `noworries` skill. Read this when the change touches a pipeline, consumer, or any read-then-write flow._

## Edge-case scenarios (don't just test the happy path)

For **data pipelines** (Flink, Kafka consumers, any read-then-write flow), a
single trigger→observe check is not enough — the code can pass it and still be
wrong under load. When the change touches such a flow, **add edge-case checks**
alongside the happy-path one. A check's `scenario:` block replaces the single
`kafka.produce` with a generated flood; verify it with an ordinary observe
assertion (usually an exact **count**). Pick the ones that match the risk:

| `kind`         | What it does | What it catches | Verify with |
| -------------- | ------------ | --------------- | ----------- |
| `burst`        | floods N messages (optionally rate-limited) over many keys | dropped messages / can't keep up | `expect_row_count: N` (no loss) |
| `concurrent`   | N writes to few keys across parallel producers → real races | lost updates, duplicate rows, stale final value | `expect_row_count: <keys>` + final value = highest `${i}` |
| `duplicates`   | sends every message **twice** | non-idempotent / double-processing | `expect_row_count: N` (not `2N`) |
| `out_of_order` | emits each key's events in **reverse** order (true order in `${i}`) | naive last-arrival-wins, bad windowing | final value = highest `${i}` |

```yaml
checks:
  # happy path first:
  - name: "single order persists"
    kafka: { produce: { topic: orders-in, key: "A", message: { id: "A", amount: 5 } } }
    db:    { query: "SELECT count(*) n FROM orders WHERE id='A'", expect_row: { n: 1 } }

  # then the edge cases:
  - name: "burst of 500 orders: none dropped"
    scenario:
      kind: burst
      count: 500
      concurrency: 8
      kafka: { topic: orders-in, key: "ord-${seq}", message: { id: "${uuid}", amount: "${seq}" } }
    db: { query: "SELECT count(*) n FROM orders", expect_row_count: 1, expect_row: { n: 500 } }

  - name: "duplicate delivery is idempotent"
    scenario:
      kind: duplicates
      count: 100
      kafka: { topic: orders-in, key: "dup-${seq}", message: { id: "dup-${seq}" } }
    db: { query: "SELECT count(*) n FROM orders WHERE id LIKE 'dup-%'", expect_row: { n: 100 } }

  - name: "concurrent updates to one key stay consistent"
    scenario:
      kind: concurrent
      count: 50
      keys: 1
      concurrency: 6
      kafka: { topic: orders-in, key: "hot", message: { id: "hot", version: "${i}" } }
    db: { query: "SELECT count(*) n FROM orders WHERE id='hot'", expect_row: { n: 1 } }
```

Template placeholders in `scenario.kafka.key`/`message`: `${seq}` (global index),
`${i}` (per-key sequence / version), `${key}` (assigned key), `${uuid}` (unique
id). Knobs: `count`, `concurrency`, `keys`, `rate_per_sec`. Scenarios flood the
pipeline, so give the observe assertions time — raise `--timeout` for big bursts.

