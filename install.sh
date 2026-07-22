#!/bin/sh
# open-interceptor installer.
#
#   curl -fsSL https://raw.githubusercontent.com/ragnito/open-interceptor/master/install.sh | sh
#
# Downloads the right prebuilt binary for your OS/arch from the latest GitHub
# release, verifies its SHA-256 checksum, and installs it to ~/.local/bin
# (override with OPEN_INTERCEPTOR_BIN_DIR).
#
# Environment overrides:
#   OPEN_INTERCEPTOR_VERSION   tag to install (default: latest, e.g. v1.0.1)
#   OPEN_INTERCEPTOR_BIN_DIR   install directory (default: $HOME/.local/bin)
#
# Supported: macOS (arm64, x86_64) and Linux (x86_64, aarch64).

set -eu

REPO="ragnito/open-interceptor"
BIN="open-interceptor"

# ---- pretty output --------------------------------------------------------
if [ -t 1 ]; then
  BOLD=$(printf '\033[1m'); BLUE=$(printf '\033[34m'); GREEN=$(printf '\033[32m')
  YELLOW=$(printf '\033[33m'); RED=$(printf '\033[31m'); RESET=$(printf '\033[0m')
else
  BOLD=''; BLUE=''; GREEN=''; YELLOW=''; RED=''; RESET=''
fi

info()  { printf '%s==>%s %s\n' "$BLUE" "$RESET" "$1"; }
ok()    { printf '%s✓%s %s\n' "$GREEN" "$RESET" "$1"; }
warn()  { printf '%s!%s %s\n' "$YELLOW" "$RESET" "$1" >&2; }
err()   { printf '%serror:%s %s\n' "$RED" "$RESET" "$1" >&2; exit 1; }

# ---- prerequisites --------------------------------------------------------
# Prefer curl, fall back to wget.
if command -v curl >/dev/null 2>&1; then
  http_get() { curl -fsSL "$1"; }
  http_dl()  { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  http_get() { wget -qO- "$1"; }
  http_dl()  { wget -qO "$2" "$1"; }
else
  err "need curl or wget installed"
fi

command -v tar >/dev/null 2>&1 || err "need tar installed"

# ---- detect platform → Rust target triple --------------------------------
os=$(uname -s)
arch=$(uname -m)

case "$os" in
  Darwin)
    case "$arch" in
      arm64 | aarch64) target="aarch64-apple-darwin" ;;
      x86_64)          target="x86_64-apple-darwin" ;;
      *) err "unsupported macOS architecture: $arch" ;;
    esac
    ;;
  Linux)
    case "$arch" in
      x86_64 | amd64)  target="x86_64-unknown-linux-musl" ;;
      aarch64 | arm64) target="aarch64-unknown-linux-musl" ;;
      *) err "unsupported Linux architecture: $arch" ;;
    esac
    ;;
  *)
    err "unsupported OS: $os (only macOS and Linux are supported)"
    ;;
esac

# ---- resolve version ------------------------------------------------------
version="${OPEN_INTERCEPTOR_VERSION:-latest}"
if [ "$version" = "latest" ]; then
  info "Resolving latest release..."
  # Parse the tag_name from the GitHub API without requiring jq.
  version=$(
    http_get "https://api.github.com/repos/$REPO/releases/latest" \
      | grep '"tag_name"' | head -n1 | cut -d'"' -f4
  )
  [ -n "$version" ] || err "could not resolve the latest release tag (is the repo published with a release?)"
fi

asset="$BIN-$target.tar.gz"
base="https://github.com/$REPO/releases/download/$version"

info "Installing $BOLD$BIN $version$RESET for $BOLD$target$RESET"

# ---- download + verify ----------------------------------------------------
tmp=$(mktemp -d 2>/dev/null || mktemp -d -t open-interceptor)
trap 'rm -rf "$tmp"' EXIT INT TERM

info "Downloading $asset ..."
http_dl "$base/$asset" "$tmp/$asset" \
  || err "download failed: $base/$asset"

# Checksum is best-effort: verify when both the file and a hashing tool exist.
if http_dl "$base/$asset.sha256" "$tmp/$asset.sha256" 2>/dev/null; then
  expected=$(cut -d' ' -f1 < "$tmp/$asset.sha256")
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$asset" | cut -d' ' -f1)
  elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)
  else
    actual=""
  fi
  if [ -n "$actual" ]; then
    [ "$expected" = "$actual" ] || err "checksum mismatch (expected $expected, got $actual)"
    ok "checksum verified"
  else
    warn "no sha256 tool found — skipping checksum verification"
  fi
else
  warn "no checksum published for this asset — skipping verification"
fi

info "Extracting ..."
tar -xzf "$tmp/$asset" -C "$tmp"
[ -f "$tmp/$BIN" ] || err "archive did not contain the expected '$BIN' binary"

# ---- install --------------------------------------------------------------
bin_dir="${OPEN_INTERCEPTOR_BIN_DIR:-$HOME/.local/bin}"
mkdir -p "$bin_dir"
install -m 755 "$tmp/$BIN" "$bin_dir/$BIN" 2>/dev/null \
  || { cp "$tmp/$BIN" "$bin_dir/$BIN" && chmod 755 "$bin_dir/$BIN"; }

ok "installed to $BOLD$bin_dir/$BIN$RESET"

# ---- post-install guidance ------------------------------------------------
echo
printf '%sNext steps%s\n' "$BOLD" "$RESET"

# 1) PATH check
case ":$PATH:" in
  *":$bin_dir:"*) : ;;
  *)
    warn "$bin_dir is not on your PATH"
    echo "    Add it to your shell profile (~/.zshrc, ~/.bashrc):"
    echo "      export PATH=\"$bin_dir:\$PATH\""
    echo
    ;;
esac

# 2) config
cfg="$HOME/.config/open-interceptor/config.yaml"
if [ ! -f "$cfg" ]; then
  echo "  1. Create your config:"
  echo "       mkdir -p $HOME/.config/open-interceptor"
  echo "       # download the example and edit it with your providers/API keys:"
  echo "       curl -fsSL https://raw.githubusercontent.com/$REPO/$version/config.yaml.example \\"
  echo "         -o $cfg"
else
  ok "config found at $cfg"
fi

# 3) daemon + Claude Code
echo "  2. Start the background daemon (launchd on macOS, systemd on Linux):"
echo "       $BIN start --install"
echo "       $BIN status"
echo "  3. Point Claude Code at the proxy (add to your shell profile):"
echo "       export ANTHROPIC_BASE_URL=http://127.0.0.1:3300"
echo "       export CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1"
echo
echo "  Or just run Claude through the proxy in one shot:"
echo "       $BIN claude"
echo
ok "done"
