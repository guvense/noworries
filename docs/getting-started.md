# Getting started

## 1. Install

Pick one (see the [README](../README.md#installation) for all options):

```bash
curl -fsSL https://raw.githubusercontent.com/guvense/noworries/main/install.sh | sh
```

Then install the Claude Code command (once):

```bash
noworries install-command          # global: ~/.claude/commands
# or, for a single project:
noworries install-command --project
```

Make sure **Docker is running** before you use it.

## 2. Scaffold a spec

From your project root:

```bash
noworries init
```

This writes a starter `noworries.yml`. Edit it to declare the services your app
needs and the checks that describe "correct" — see
[configuration](configuration.md) and [checks](checks.md).

## 3. Run

```bash
noworries               # asks for confirmation before starting anything
noworries --yes         # skip the prompt
noworries --tags orders # only run checks tagged "orders"
```

A run:

1. detects declared services and the framework,
2. asks you to confirm (unless `--yes`),
3. generates `.noworries/compose.test.yml`,
4. `docker compose up -d` and waits for health,
5. starts your app wired to the containers, waits for its health endpoint,
6. runs the selected checks,
7. prints `Result: READY` / `NOT READY`, writes `.noworries/results.json`,
8. tears everything down (`docker compose down -v`) — even on Ctrl-C.

Exit code is `0` when READY, `1` otherwise.

## 4. The loop, inside Claude Code

The whole point of `noworries` is that an AI can drive it. After Claude makes a
change, run:

```
/noworries              # scope to what changed
/noworries force        # full regression over everything that changed
```

Claude will read the diff, write/update `noworries.yml` for the feature, run the
harness, read the results, and — if it's NOT READY — fix the code and run again
until it's green. See [how it works](how-it-works.md).

## Debugging a run

- `--keep-alive` leaves the containers up after the run (the command prints the
  `docker compose ... down -v` to tear them down later).
- `.noworries/app.log` — your app's stdout/stderr (handy if it fails to start).
- `.noworries/compose.test.yml` — the generated compose file.
- `.noworries/results.json` — machine-readable results.
- `--timeout N` — hard cap on the whole run (default 180s); raise it if the
  first framework build is slow.
