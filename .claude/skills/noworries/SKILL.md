---
name: noworries
description: Use when someone wants a change to a backend service actually run and checked against live infrastructure before they trust it — real Postgres/MySQL/Mongo/Redis/Kafka/Elasticsearch in throwaway Docker containers, upstream third-party APIs stubbed. Trigger on the intent "I wrote/changed something and want proof it behaves": a new or modified endpoint, handler, consumer, job, or data-access path that the user wants verified, exercised, tested for real, confirmed to return the right status codes, reject bad auth, survive malformed input, or persist and emit what it should — especially before merging or shipping, or when they distrust mocks and want real databases instead. Also on "verify", "prove it works", "does this actually work", "is this ready", "run noworries", "/noworries", "noworries init/force". Not for: unit tests, CI/pipeline config, Dockerfile or compose troubleshooting, load/perf benchmarking, or code edits with no ask to see them run.
allowed-tools: Bash(noworries:*), Bash(cargo:*), Bash(git:*), Bash(docker:*), Read, Edit, Write
---

# noworries

Code that compiles is not code that works. After a change, stand up its
infrastructure in ephemeral Docker containers, start the app against them, run
the checks in `noworries.yml`, and read the results. All green → tell the user
it's **READY**. Otherwise fix the code and run again — loop until green.

> **Requires `noworries` >= 0.6.0** (`noworries --version`). Older builds lack
> the framework adapters (dotnet/rails/laravel/django), the `mariadb` service,
> `noworries spec`/`validate`, mock `body_contains`/`delay_ms`, and the
> extensible check types (graphql/metrics/snapshot/schema/sse/websocket/grpc/
> traces/security) plus `externals`/`mock`.

`noworries` writes `.noworries/results.json` and prints a
`=== noworries results ===` block ending in `Result: READY` / `NOT READY`.
Exit code: `0` READY, `1` NOT READY/error, `2` not confirmed.

## Modes

- `/noworries` — scope to what changed since HEAD (`noworries changed`).
- `/noworries force` — regression: everything (`noworries changed --all`), full suite.
- `/noworries init` — scaffold a starter `noworries.yml` (`noworries init`), then
  show it to the user and tailor its `checks` to the code. Add ready-to-edit
  example blocks with `noworries init --with externals-mock,scenario,graphql,…`.

## The loop

1. **See what changed** (`noworries changed` / `git diff`). Identify observable
   behaviour: HTTP endpoints/status/body, DB writes, Kafka consumers/producers,
   Redis caching, Elasticsearch indexing, MySQL/Mongo reads-then-writes.
   - Data pipeline / streaming / concurrent writes → also plan edge-case checks
     (burst, races, duplicates, out-of-order): `references/scenarios.md`.
   - New or changed endpoint → add a defensive `security:` check (auth enforced,
     hostile input doesn't 5xx, no error leak, security headers). It probes only
     the app under test and asserts safe handling.
   - Endpoint that only some roles may call → declare the roles under `users:`
     and write one check per role with `as:` (`references/checks.md`).
   - Flink job instead of an HTTP app → `references/flink.md`.
   - App calls an upstream noworries can't run → `references/externals.md`.
2. **Write/update `noworries.yml`** (shape below; full field reference in
   `references/checks.md`). For a **new** feature or new checks, show the
   proposed checks to the user before the first run. For a pure re-run or a
   tag/scope change, just run — don't gate on the user.
3. **Run** `noworries --yes [--tags <tags>]` (confirm services on a project's
   very first run). `force` → no tag filter.
4. **Read** `.noworries/results.json` + the printed block. Each failed assertion
   has `expected` vs `actual`; check `.noworries/app.log` for app errors.
5. **Decide.** READY → tell the user, summarise what was verified. NOT READY →
   fix the **code** (not the check, unless the check was wrong) and go to 3.
   After ~2 failed attempts on the same check, stop and ask the user.

## The shape of a spec

```yaml
version: 1
services: [ postgres:16-alpine, redis, kafka ]   # ephemeral containers
app:                          # optional; auto-detected from mvnw/gradlew/go.mod/…
  start: "./mvnw spring-boot:run"
  health: "/actuator/health"
auth:                         # optional; applied to every request. ${VAR} from .noworries.env
  bearer: { token: "${API_TOKEN}" }   # or login / basic / api_key / oidc
users:                        # optional; named identities for RBAC checks
  reader: { bearer: { token: "${READER_TOKEN}" } }
checks:
  - name: "create order: 201, persists, emits"
    tags: [orders]
    request: { method: POST, path: /orders, body: { sku: "ABC", qty: 2 } }
    expect:  { status: 201, body_contains: { sku: "ABC" } }
    db:      { query: "SELECT status FROM orders WHERE sku='ABC'", expect_row: { status: "PENDING" } }
    kafka:   { expect_message: { topic: "order-events", contains: { type: "OrderCreated" }, timeout_ms: 5000 } }
```

A check may combine assertion types and passes only if **all** pass. Order
within a check: **seed** → **trigger** (`request`, `kafka.produce`, `scenario`)
→ **observe**. Ports, URLs and credentials are injected into the app's
environment automatically — you usually don't touch config files.

## Getting the schema right (authoritative sources)

The references cover the common fields. For anything whose **exact YAML shape
isn't shown**:

- `noworries spec` (alias `noworries schema`) prints the **full field reference
  bundled with the installed binary**. `noworries spec --format json` prints a
  **JSON Schema** generated from the actual types — query it programmatically
  (`.definitions.MockStub.properties`) or point a YAML editor at it for completion.
- `noworries validate` (or `noworries --file <f> validate`) parses the spec
  **without starting containers** and prints a precise error, e.g.
  `checks[0].request: unknown field "verb", expected one of "method", "path",
  "headers", "body" at line 7 column 9`. Write a minimal block, validate, read the
  error, fix — iterate fast.

> **`noworries spec` is authoritative.** When its output (or `--format json`)
> differs from these files, **trust `noworries spec`** — it matches the installed
> binary's accepted schema; these files may lag it.

> **If a field's shape is still unclear, do NOT infer it from a local source
> checkout.** Either ask the user, or make a minimal probe and read the
> `noworries validate` error. Do not rely on files outside the tool's install
> prefix (they may not exist on another machine / CI).

## Behaviours that bite

- **Checks run in order and share state.** One shared infra instance per run — a
  check that writes `ABC` is visible to later checks. Make check data disjoint,
  or reset in a `seed`, before asserting "exactly 1 row".
- **Secrets:** reference `${VAR}` in `auth`/`externals`/headers/body; put values
  in a **`.noworries.env`** (gitignored automatically). Derive auth from config
  or code where possible; **ask the user** for anything you can't derive.
- **Container-healthy ≠ protocol-ready.** RabbitMQ, MySQL/MariaDB and
  Cassandra accept TCP before they finish an application-level handshake, so
  the app's *first* `connect()` can still fail. Add a short connect-retry loop
  in the app — it is the most common cause of a "crashed at startup" app in an
  otherwise green run.
- **Naming the app's port.** Relative paths (`sse`, `websocket`, `graphql`,
  `metrics`) resolve against the app automatically; a full target like
  `grpc.target` interpolates it — `${NOWORRIES_APP_PORT}` (also
  `${NOWORRIES_APP_URL}`, `${PORT}`/`${SERVER_PORT}`, and
  `${NOWORRIES_<SERVICE>_PORT}` for containers).
- **First run is slow:** Elasticsearch (~600MB), Kafka, Mongo and Flink images
  pull on first use — run with `--timeout 600`, otherwise a slow pull shows up as
  `NOT READY` (a timeout, not an app bug).
- Everything is torn down (`docker compose down -v` + the app) after each run
  unless `--keep-alive`.

## References

Read these on demand — don't load them all up front.

| File | Read it when |
| ---- | ------------ |
| `references/checks.md` | writing or fixing `checks:` — every assertion type, fields, examples |
| `references/services-and-env.md` | picking `services:`, container credentials, framework detection, injected env vars |
| `references/scenarios.md` | the change touches a pipeline/consumer — burst, concurrent, duplicates, out-of-order |
| `references/externals.md` | the app calls an upstream noworries can't run — `externals`, mocks, `external_calls` |
| `references/flink.md` | the change is a Flink job, not an HTTP app |
| `references/troubleshooting.md` | a run fails for a reason that isn't an app-side assertion |
