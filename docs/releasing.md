# Releasing (maintainers)

Releases are automated on merge to `main`, driven by the `version` in
`Cargo.toml` (the single source of truth). The pipeline lives in
`.github/workflows/release.yml`.

## Cutting a release

1. In your PR, bump `version` in `Cargo.toml` (e.g. `0.1.0` → `0.1.1`).
2. Merge to `main`.

On that push, the workflow runs and — **only if there is no `v<version>` tag
yet** — does everything:

- **build**: compiles macOS arm64/x64 + Linux x64 binaries, creates the GitHub
  Release `v<version>`, and uploads `noworries-<target>.tar.gz` + `.sha256`.
- **npm**: syncs `dist/npm/package.json` to the version and `npm publish`es.
- **homebrew**: writes `Formula/noworries.rb` (new version + checksums) into the
  `guvense/homebrew-noworries` tap and commits it.

Pushing to `main` **without** bumping the version does nothing — the tag already
exists, so `guard` skips the release. You can also trigger a run manually from
the **Actions** tab (`workflow_dispatch`).

## One-time setup

### Secrets (in the code repo → Settings → Secrets and variables → Actions)

| Secret               | What |
| -------------------- | ---- |
| `NPM_TOKEN`          | npm **Automation** (or granular, read/write) token. |
| `HOMEBREW_TAP_TOKEN` | A PAT with **Contents: read and write** on the tap repo. |

### The Homebrew tap repo

Create a **public** repo named exactly `homebrew-noworries` under your account
(the `homebrew-` prefix is required). It can be empty — the workflow writes
`Formula/noworries.rb` into it. Users then install with:

```bash
brew install guvense/noworries/noworries
```

### npm name

If the name `noworries` is taken, switch to a scoped name in
`dist/npm/package.json` (`@guvense/noworries`); the workflow already publishes
with `--access public`.

## How each channel consumes a release

- **curl | sh** (`install.sh`) and **npm** (`dist/npm/install.js`) download
  `noworries-<target>.tar.gz` from the GitHub Release for the requested version.
  So the Release (binaries) must exist before an install works — the pipeline
  guarantees this by running `npm`/`homebrew` only after `build`.
- **Homebrew** downloads the same tarballs; the formula pins the version and the
  three SHA-256 checksums.

## Verifying a release

- **GitHub → Releases**: `v<version>` with 6 assets (3 tarballs + 3 checksums).
- **npm**: `npm view noworries` shows the new version.
- **Homebrew**: `Formula/noworries.rb` in the tap updated to the new version.
- **Actions**: all four jobs green. A red job's log shows the cause (usually a
  missing/incorrect secret).

## Local dev

```bash
cargo build            # debug
cargo build --release  # optimized binary at target/release/noworries
cargo clippy --all-targets
cargo test --lib       # unit tests
```

Integration tests need real services and are gated behind `NOWORRIES_IT_*` env
vars — see [architecture](architecture.md#testing).
