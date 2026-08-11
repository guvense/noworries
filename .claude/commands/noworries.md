---
description: Verify the change just made — auto-generate noworries.yml, stand up infra, run checks, fix and re-run until READY
argument-hint: "[force | init]"
allowed-tools: Bash(noworries:*), Bash(cargo:*), Bash(git:*), Bash(docker:*), Read, Edit, Write
---

# /noworries

You just wrote or changed some code. The user doesn't know end-to-end whether it
actually works. Your job: figure out what the change *should* do, stand up the
infrastructure it needs (Postgres / Kafka / Redis) in throwaway Docker
containers, start the app, exercise the new behaviour, and report **READY** or
**NOT READY**. If it's NOT READY, fix the code and run again — loop until it's
green or you're genuinely stuck.

`noworries` is the CLI (Rust). It generates `.noworries/compose.test.yml`,
brings the infra up, starts the app, runs the checks in `noworries.yml`, writes
`.noworries/results.json`, prints a `Result: READY / NOT READY` block, and tears
everything down. Exit code `0` = READY, `1` = NOT READY / error.

> If `noworries` isn't on PATH, build it once (`cargo build --release` in the
> noworries repo) and use `target/release/noworries`, or `cargo install --path
> .` to install it. Docker must be running.

## Modes

- `/noworries` — scope to what changed since HEAD (`noworries changed`).
- `/noworries force` — regression scope: cover everything
  (`noworries changed --all`) and run the full check suite (no tag filter).
- `/noworries init` — scaffold a starter `noworries.yml` (`noworries init`).

## The loop

1. **See what changed.** Run `noworries changed` (or `noworries changed --all`
   for `force`) and `git diff` on those files. Understand the observable
   behaviour the change introduces:
   - new/changed HTTP endpoints and their status codes / response bodies,
   - anything written to the **database**,
   - a **Kafka** consumer added (triggered by a message on a topic) or a
     producer added (emits to a topic),
   - anything **cached to Redis**.

2. **Write / update `noworries.yml`.** Declare the `services` the change needs
   and add `checks` that capture the behaviour:
   - HTTP: `request` + `expect` (`status`, `body_contains`).
   - DB: `db.query` + `expect_row` / `expect_row_count`.
   - Kafka consumer feature: `kafka.produce` a message to the topic, then a
     `db`/`redis` assertion for the effect it should have.
   - Kafka producer feature: `kafka.expect_message` on the output topic.
   - Redis cache feature: `redis.key` + `expect_exists` / `expect_value_contains`.

   Tag the new checks for the current task (e.g. `tags: [orders]`). **Show the
   proposed `noworries.yml` (or the new checks) to the user and get their review
   before the first run.** If there's no `noworries.yml`, run `noworries init`
   first, then edit it. In `force` mode, run the full suite instead of a tag
   subset.

3. **Run.** `noworries --yes` (add `--tags <tags>` to scope to the current
   task; omit for `force`). Confirm the detected services with the user on the
   very first run of a project.

4. **Read the result.** Parse `.noworries/results.json` (and the printed
   `=== noworries results ===` block). Each failed assertion carries `expected`
   vs `actual`. Check `.noworries/app.log` for app-side stack traces.

5. **Decide.**
   - **READY** → tell the user it's ready and summarise what was verified
     (which endpoints/DB/Kafka/Redis behaviours passed).
   - **NOT READY** → diagnose from the expected-vs-actual (and `app.log`), fix
     the **code** (not the check — unless the check itself was wrong), and go
     back to step 3. Repeat until READY.

   After ~2 failed fix attempts on the same check, stop and bring the user in
   with what you tried and what's still failing. Don't loop silently forever.

## noworries.yml reference

```yaml
version: 1
services:
  - postgres:16-alpine     # or: kafka (=> apache/kafka), redis
app:                       # optional; auto-detected from the framework
  start: "./mvnw spring-boot:run"
  health: "/actuator/health"
checks:
  - name: "create order returns 201 and persists"
    tags: [orders]
    request: { method: POST, path: /orders, body: { sku: "ABC123", qty: 2 } }
    expect:  { status: 201 }
    db:
      query: "SELECT status FROM orders WHERE sku = 'ABC123'"
      expect_row: { status: "PENDING" }

  - name: "OrderCreated event is consumed and persisted"
    tags: [orders]
    kafka:
      produce: { topic: "orders", message: { type: "OrderCreated", sku: "X1", qty: 1 } }
    db:
      query: "SELECT count(*) AS n FROM orders WHERE sku = 'X1'"
      expect_row: { n: 1 }

  - name: "order is cached to redis after creation"
    tags: [orders]
    request: { method: POST, path: /orders, body: { sku: "C1", qty: 1 } }
    expect:  { status: 201 }
    redis:
      key: "cache:order:C1"
      expect_exists: true
      expect_value_contains: { status: "PENDING" }
```

## Notes

- Everything (containers + the app process) is torn down after every run unless
  you pass `--keep-alive` for debugging. `--timeout N` caps the whole run.
- The framework is currently Spring Boot (auto-detected). The architecture is
  framework-agnostic; if the app isn't detected, set `app.start` in
  `noworries.yml`.
