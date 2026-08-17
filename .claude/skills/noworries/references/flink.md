# noworries — Flink pipelines

_Reference for the `noworries` skill. Read this when the change is an Apache Flink job rather than an HTTP app._

## Flink pipelines

If the change is an Apache Flink **job** (not an HTTP app), use a `flink:` block
**instead of `app:`**. noworries stands up an ephemeral Flink session cluster
(jobmanager + taskmanager) on the same network as the declared services, builds
and submits the job(s) over the REST API, waits for `RUNNING`, then runs checks.

```yaml
version: 1
services: [kafka, postgres, elastic]
flink:
  image: flink:1.19        # optional
  slots: 2                 # optional task slots
  jobs:
    - build: "mvn -q -DskipTests package"   # optional; runs first
      jar: target/pipeline-0.1.jar          # required
      entry_class: com.acme.Pipeline        # optional
      args: ["--source", "events-in"]       # optional
checks:
  - name: "kafka -> postgres -> topic -> ES flows end to end"
    kafka:   { produce: { topic: "events-in", key: "E1", message: { id: "E1" } } }
    db:      { query: "SELECT status FROM processed WHERE id='E1'", expect_row: { status: "OK" } }
  - name: "enriched event indexed"
    kafka:   { expect_message: { topic: "events-enriched", contains: { id: "E1" }, timeout_ms: 15000 } }
    elastic: { index: "events", doc_id: "E1", expect_source_contains: { id: "E1" } }
```

**Critical — in-network addresses.** The job runs *inside* the cluster, so it
reaches services by compose name + container port, NOT the host ports: `kafka:9092`,
`postgres:5432`, `elastic:9200`, `redis:6379`, `mysql:3306`, `mongodb:27017`.
These are also injected as `NOWORRIES_<SERVICE>_HOST`/`_PORT`. Configure the job
to use those. First run pulls the ~600MB Flink image — use `--timeout 600`.

**Flink gotchas — check these before blaming the spec:**

- **Java 17 job?** The default `flink:1.19` is **Java 11**; set
  `image: flink:1.19-java17` or the job won't load.
- **Kafka source topic must pre-exist** (`KafkaSource` uses
  `AdminClient.describeTopics`, which does *not* auto-create). noworries now
  pre-creates every topic a check's `kafka.produce`/`kafka.expect_message` names,
  plus any in `flink.topics`, before the jobs start — so no `ensureTopics()` in
  the job. Just ensure the source topic is named by a check or listed in
  `flink.topics: [my-source]`.
- **ES 7 connector + default ES 8 = incompatible.** `flink-connector-elasticsearch7`
  can't parse ES 8 bulk responses (`Unable to parse response body`); pin
  `services: [elasticsearch:7.17.22]` when using that connector.
- **ES refresh:** the connector rejects item-level `setRefreshPolicy`; set
  `refresh_interval: "100ms"` in the `elastic.template` settings. (noworries also
  `_refresh`es before each search assertion.)
- **Observe async in stages:** `kafka.expect_message` on the downstream topic
  (with `timeout_ms`) **before** the `elastic`/`db` assert — fails fast and clear
  when the job never emitted.

