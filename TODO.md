# TODO

Open work, roughly in the order it's worth doing. Kept in the repo (rather than
in a chat) so the next session — human or AI — starts from the same list.

## Ship 0.16.0

- [ ] **Push `main`.** Three commits are local only; the release workflow fires
      on the version in `Cargo.toml` (0.16.0) and cuts the tag, npm package and
      Homebrew tap from it.
- [ ] **`.github/workflows/release.yml:198`** still says "slash command" —
      should be "skill". (Protected file; needs a local edit.)
- [ ] **Confirm the tap actually moved.** The v0.14.0 run failed at
      `softprops`' finalize step with a transient GitHub 5xx, which skipped the
      dependent npm + homebrew jobs. If the tap is still behind, re-run the
      *failed* jobs of that run — a fresh run skips everything, because `guard`
      sees the tag already exists.

## Verify against real containers

Everything below has in-process tests but has never met the real thing. The
pattern this project keeps hitting is that a surface breaks on its *first* real
use, so treat these as unverified until a docker run says otherwise.

- [ ] `smtp` (Mailpit) and `minio` — `mail_s3_real_it` runs them in CI; confirm
      the vendor healthchecks (`/mailpit readyz`, `mc ready local`) really pass,
      and that MinIO accepts our SigV4 signature.
- [ ] `auth.oidc` against a real provider (Keycloak is the easy one) — discovery
      URL shape, `client_auth: basic` for Cognito.
- [ ] MariaDB, OpenSearch, MSSQL/ClickHouse env wiring, `.NET` port binding,
      CockroachDB `db:`/`schema:` — all fixed but only field-tested by hand.

## Finish what 0.16 started

- [x] `noworries init --with email,s3` templates.
- [x] Skill's minimum-version line now names what each release added.
- [x] README's service list includes `smtp` and `minio`.
- [ ] MinIO starts with no buckets. Either document harder or give the service
      an optional `buckets: [uploads]` that creates them before the app starts.

## Known UX gap

- [ ] **The installed skill goes stale silently.** The skill is embedded in the
      binary, so `brew upgrade` updates the tool while
      `~/.claude/skills/noworries/` keeps the old instructions — the agent then
      works from a doc that predates the fixes. Stamp a version into
      `SKILL.md`'s frontmatter, compare it at run time, and warn: "your
      installed skill is older than this binary — run `noworries
      install-command`".

## Next features (agreed order)

1. [ ] `--only "<name pattern>"`, `--exclude-tags`, `--diff` (delta vs the last
       `results.json`). Cheap, and they cut minutes and tokens off every agent
       iteration.
2. [ ] `vars:` (constants) + `includes:` (split a spec across files). `fixtures`
       is sugar over `vars` — skip it.
3. [ ] `companions:` — extra processes noworries starts and wires but no check
       targets. Covers "orders needs merchant running" without touching check
       targeting; full multi-app (`apps:` + per-check targeting) is a much
       larger change and can wait.
4. [ ] Auth abuse sweep (`security.abuse`: `missing_auth`, `expired_token`,
       `wrong_role`, `forged_jwt`). Generated checks must carry explicit names
       (`<check> [missing_auth]`) or the report becomes unreadable.
5. [ ] `schema` drift: snapshot the schema, fail on `column_removed` /
       `type_changed`. Sits on the existing schema + snapshot machinery.
6. [ ] `logs.error_rate` and `expect.p95_ms`/`p99_ms` — natural companions to
       `scenario`, which already floods the system.
7. [ ] Framework detection for Rust (Axum/Actix), Elixir Phoenix, Symfony,
       Nest.js. Mechanical.
8. [ ] Endpoint coverage report: "12 endpoints, this run exercised 9, missing
       […]". Directly attacks the tool's weak spot — the agent writes the checks
       *and* grades them.

## Deliberately not doing

- **Browser/E2E (`playwright`)** — drags in a browser runtime and a whole new
  flake surface, and blurs what the tool is. Hand off to a real E2E tool via
  the JUnit report instead.
- **OpenAPI fuzzing** — generated failures are noisy, and here the agent would
  "fix" the wrong thing. A narrow version (assert the responses of checks you
  already wrote conform to the schema) is worth doing; the fuzzer isn't.
- **`groups:` with before/after hooks** — test-framework lifecycle semantics for
  a problem that top-level `setup:` and explicit seeds already cover.

## Standing rule

Every new service or check type ships with a docker-backed smoke test in CI.
Breadth without per-surface verification is what produced the MariaDB,
OpenSearch, MSSQL and CockroachDB bugs: each was broken on the first real use of
that surface, in a released version.
