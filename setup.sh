#!/usr/bin/env bash
#
# One-time setup so `/noworries` is available in EVERY Claude Code project.
# Run this from the noworries repo root:  bash setup.sh
#
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"

echo "==> Building + installing the noworries binary (cargo install --path .)"
if ! command -v cargo >/dev/null 2>&1; then
  echo "   ! Rust/cargo not found. Install from https://rustup.rs then re-run." >&2
  exit 1
fi
cargo install --path .          # installs `noworries` into ~/.cargo/bin

echo "==> Installing the /noworries slash command globally (~/.claude/commands)"
if command -v noworries >/dev/null 2>&1; then
  noworries install-command            # embedded in the binary; installs globally
else
  mkdir -p "$HOME/.claude/commands"
  cp "$here/.claude/commands/noworries.md" "$HOME/.claude/commands/noworries.md"
fi

# Sanity: is the binary reachable?
if command -v noworries >/dev/null 2>&1; then
  echo "==> Installed: $(command -v noworries)  ($(noworries --version))"
else
  echo "==> Installed the binary, but ~/.cargo/bin is not on your PATH yet."
  echo "    Add this to your shell profile (~/.zshrc or ~/.bashrc):"
  echo '      export PATH="$HOME/.cargo/bin:$PATH"'
fi

cat <<'EOF'

Done ✅
  • `noworries` is on your PATH (via ~/.cargo/bin)
  • `/noworries` is available in every Claude Code project (~/.claude/commands)

Requirements at run time: Docker running (Desktop on macOS), plus the app's
toolchain (e.g. JDK + Maven for Spring Boot).

Try it:  open any repo in Claude Code, make a change, then run  /noworries
(or  /noworries force  for a full regression pass).

Prefer per-project instead of global? Skip this script and just copy
.claude/commands/noworries.md into that project's .claude/commands/.
EOF
