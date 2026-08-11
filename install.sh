#!/usr/bin/env sh
#
# noworries installer (no Rust required — downloads a prebuilt binary).
#
#   curl -fsSL https://raw.githubusercontent.com/guvense/noworries/main/install.sh | sh
#
# Env overrides:
#   NOWORRIES_REPO     owner/repo         (default: guvense/noworries)
#   NOWORRIES_VERSION  release tag        (default: latest)
#   NOWORRIES_BIN_DIR  install directory  (default: $HOME/.local/bin)
#
set -eu

REPO="${NOWORRIES_REPO:-guvense/noworries}"
BIN_DIR="${NOWORRIES_BIN_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# --- detect target triple ---------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$os/$arch" in
  Darwin/arm64)   target="aarch64-apple-darwin" ;;
  Darwin/x86_64)  target="x86_64-apple-darwin" ;;
  Linux/x86_64)   target="x86_64-unknown-linux-gnu" ;;
  Linux/aarch64)  err "prebuilt Linux arm64 binary isn't published yet; install with: cargo install --git https://github.com/$REPO" ;;
  *) err "unsupported platform: $os/$arch. Try: cargo install --git https://github.com/$REPO" ;;
esac

# --- resolve version --------------------------------------------------------
version="${NOWORRIES_VERSION:-}"
if [ -z "$version" ]; then
  have curl || err "curl is required"
  version="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep -o '"tag_name": *"[^"]*"' | head -n1 | cut -d'"' -f4)"
  [ -n "$version" ] || err "could not determine latest version (set NOWORRIES_VERSION)"
fi
say "==> noworries $version for $target"

# --- download + verify ------------------------------------------------------
asset="noworries-$target.tar.gz"
base="https://github.com/$REPO/releases/download/$version"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

have curl || err "curl is required"
curl -fsSL "$base/$asset" -o "$tmp/$asset" || err "download failed: $base/$asset"

if curl -fsSL "$base/$asset.sha256" -o "$tmp/$asset.sha256" 2>/dev/null; then
  want="$(cut -d' ' -f1 <"$tmp/$asset.sha256")"
  if have sha256sum; then got="$(sha256sum "$tmp/$asset" | cut -d' ' -f1)";
  elif have shasum; then got="$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)"; else got=""; fi
  if [ -n "$got" ] && [ "$got" != "$want" ]; then err "checksum mismatch"; fi
  [ -n "$got" ] && say "==> checksum ok"
fi

# --- install binary ---------------------------------------------------------
tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$BIN_DIR"
mv "$tmp/noworries" "$BIN_DIR/noworries"
chmod +x "$BIN_DIR/noworries"
say "==> installed $BIN_DIR/noworries"

# --- install the /noworries slash command (embedded in the binary) ----------
"$BIN_DIR/noworries" install-command || say "   (run 'noworries install-command' later to add the /noworries slash command)"

# --- PATH hint --------------------------------------------------------------
case ":$PATH:" in
  *":$BIN_DIR:"*) : ;;
  *)
    say ""
    say "Add $BIN_DIR to your PATH (in ~/.zshrc or ~/.bashrc):"
    say "  export PATH=\"$BIN_DIR:\$PATH\""
    ;;
esac

say ""
say "Done. Requires Docker running at use time. Try /noworries in Claude Code."
