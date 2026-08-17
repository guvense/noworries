# CLI reference

```
noworries [OPTIONS] [COMMAND]
```

## Commands

| Command                          | Description |
| -------------------------------- | ----------- |
| `noworries`                      | Run the harness: detect services, (confirm), bring infra up, start the app, run checks, tear down. |
| `noworries init`                 | Scaffold a starter `noworries.yml`. |
| `noworries init --with a,b`      | Also append ready-to-edit example blocks: `externals-mock`, `scenario`, `flink`, `auth`, `graphql`, `grpc`, `metrics`, `sse`, `websocket`, `schema`. |
| `noworries validate`             | Parse + validate the spec **without** starting containers. Prints a precise error (field + line:column) on failure. |
| `noworries spec` (`schema`)      | Print the full `noworries.yml` field reference bundled with this binary (authoritative for the installed version). |
| `noworries spec --format json`   | Print a **JSON Schema** generated from the actual types (editor completion / programmatic query, e.g. `.definitions.MockStub.properties`). |
| `noworries changed`              | List files changed vs `HEAD` (modified + staged + untracked). Used by `/noworries` to scope checks. |
| `noworries changed --all`        | List **all** tracked files (the `force` / regression scope). `--force` is an alias for `--all`. |
| `noworries install-command`      | Install the `/noworries` Claude Code skill into `~/.claude/skills/noworries/` (`SKILL.md` + `references/`). |
| `noworries install-command --project` | Install it into `./.claude/skills/noworries/` (this project only). |

## Options (for a run)

| Flag              | Default   | Description |
| ----------------- | --------- | ----------- |
| `-y`, `--yes`     | off       | Skip the confirmation prompt (assume yes). |
| `--tags a,b`      | all       | Only run checks tagged with any of these. |
| `--keep-alive`    | off       | Leave containers running after the run (for debugging). |
| `--timeout N`     | `180`     | Hard cap on the whole run, in seconds. |
| `--json`          | off       | Also print the results summary as JSON on stdout. |
| `--junit <path>`  | off       | Write a JUnit XML report (one test case per check) for CI. |
| `--html <path>`   | off       | Write a self-contained HTML results report. |
| `--update-snapshots` | off    | Write/refresh `snapshot` golden files instead of failing on a diff. |
| `--dir <path>`    | `.`       | Project directory. |
| `--file <path>`   | `<dir>/noworries.yml` | Use a specific spec file (one repo can hold several scopes). |

## Exit codes

| Code | Meaning |
| ---- | ------- |
| `0`  | READY — all selected checks passed. |
| `1`  | NOT READY, or an error (bad spec, Docker unavailable, app failed to start). |
| `2`  | Not confirmed (ran without `--yes` and declined the prompt). |

## Examples

```bash
noworries --yes                     # non-interactive run of all checks
noworries --yes --tags orders       # just the "orders" checks
noworries --keep-alive --tags smoke # leave infra up to poke at it
noworries changed --json            # {"...":["src/Foo.java", ...]} for tooling
NOWORRIES_REPO=me/fork noworries ... # (install.sh honors this; the binary reads noworries.yml)
```

## Generated files (`.noworries/`)

| File                  | What it is |
| --------------------- | ---------- |
| `compose.test.yml`    | The generated Docker Compose file for the run. |
| `app.log`             | The app process's stdout/stderr. |
| `results.json`        | Machine-readable check results. |

`.noworries/` is safe to add to `.gitignore`.
