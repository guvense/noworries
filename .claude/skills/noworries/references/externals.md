# noworries — external / upstream services and mocks

_Reference for the `noworries` skill. Read this when the app calls out to a service noworries can't run._

## External / upstream services (app calls out to something noworries can't run)

If the change makes the app call an **upstream service you don't containerize** —
a partner/sandbox API, a separate auth server, a payment gateway — declare it
under `externals:`. noworries injects its URL + credentials into the app's
environment (it does **not** stand the service up). This is app → upstream;
`services` is what noworries runs, `auth` is noworries → app.

**How to fill it (do this from the code, then ask):**

1. **Detect** the dependency: look in `application.properties`/`.yml`, config
   classes, or client code for a base URL (e.g. `payments.base-url`,
   `PARTNER_API_URL`) and how it authenticates (basic, bearer, api-key header).
2. **Derive** what you can. Set `env`/`url_env` to the exact env var / property
   the app reads. If auth details are in config/code, wire them.
3. **Never hardcode secrets or guess a URL.** For the sandbox URL and any
   credential you can't derive, reference `${VAR}` and **ask the user** for the
   value (sandbox base URL, username/password, token, or API key). Values go in
   the gitignored `.noworries.env`; an interactive run also prompts for missing
   `${VAR}`.

```yaml
externals:
  - name: payments
    url: "${PAYMENTS_URL}"              # ask the user for the sandbox URL
    url_env: PAYMENTS_BASE_URL          # the property/env your app actually reads
    env:                                # optional: extra literal env vars (values interpolate ${VAR})
      PAYMENTS_TIMEOUT_MS: "3000"
    auth:
      basic: { username: "${PAY_USER}", password: "${PAY_PASS}", header_env: PAYMENTS_AUTHORIZATION }
      # or bearer: { token: "${PAY_TOKEN}", header_env: PAYMENTS_AUTHORIZATION }
      # or api_key: { value: "${PAY_KEY}", header: "X-Api-Key", value_env: PAYMENTS_API_KEY }
```

Every external also sets conventional vars the app can read with no mapping:
`NOWORRIES_EXTERNAL_<NAME>_URL`, `…_USER`/`…_PASSWORD`/`…_AUTHORIZATION` (basic,
ready `Basic base64` header), `…_TOKEN`/`…_AUTHORIZATION` (bearer),
`…_API_KEY`/`…_API_KEY_HEADER` (api-key). `<NAME>` is uppercased with
non-alphanumerics → `_`. If the app has no matching env override yet, prefer
adding one in code that reads the conventional var, or set the app's real var via
`url_env`/`*_env`.

`externals[].env` is a map of extra literal env vars for that dependency; values
interpolate `${VAR}` like everywhere else.

### Mocking an external (`externals[].mock`)

Instead of a real sandbox, stand up an **in-process mock**: noworries serves the
stubs on a local port, injects **that** URL as the external's URL (overrides
`url`), and **records every request** the app makes so `external_calls` can assert
on them.

```yaml
externals:
  - name: payments
    url_env: PAYMENTS_URL               # the mock's URL is injected here (+ NOWORRIES_EXTERNAL_PAYMENTS_URL)
    auth: { basic: { username: "u", password: "p", header_env: PAYMENTS_AUTHORIZATION } }
    mock:
      stubs:                            # ARRAY; matched top-to-bottom, FIRST match wins
        - when: { method: GET, path: /charge/1 }         # `path` is an EXACT match; query string ignored
          respond: { status: 200, body: { id: "1", status: "PAID" }, headers: { X-Trace: "t" } }
        - when: { path: /charge, body_contains: { amount: 200 } }   # match by request BODY too
          respond: { status: 402, body: { error: "too big" } }      # same path, different response per payload
        - when: { path: /slow }
          respond: { status: 200, delay_ms: 3000 }   # artificial latency → test client timeout/circuit-breaker
        - when: { path: /webhook }      # `method` OMITTED → matches ANY method
          respond: { status: 202 }      # body/headers optional; default status 200
      # A request matching NO stub is still RECORDED and answered `200` with an empty body.
```

`mock.stubs[]` — each: `when { method?, path, body_contains? }` and
`respond { status?=200, body?, headers?, delay_ms? }`. `path` matches exactly (no
prefix/regex; query ignored); `when.method` omitted = any; `when.body_contains` is
a deep-subset match on the request JSON body (put specific body stubs before a
catch-all). `respond.delay_ms` adds latency before replying. `body` is JSON.

Then assert the app actually called the mock:

```yaml
checks:
  - name: "creating an order charges the customer"
    request: { method: POST, path: /orders, body: { sku: "A", amount: 100 } }
    expect:  { status: 201 }
    external_calls:
      - external: payments              # matches externals[].name
        method: POST                    # OPTIONAL; omit to match ANY method
        path: /charge                   # EXACT path match (query string ignored)
        body_contains: { amount: 100 }  # deep-SUBSET match on the recorded JSON body
        times: 1                        # exact count; OMIT for "at least one"
        timeout_ms: 8000                # OPTIONAL wait for an async call (default ~6s window)
```

`external_calls` is retry-aware. Matching: `path` exact, `method` optional (any if
omitted), `body_contains` deep-subset, `times` exact-count or (omitted)
at-least-one, `timeout_ms` = how long to wait for an asynchronous call.

