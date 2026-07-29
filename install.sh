#!/bin/sh
# open-interceptor installer.
#
#   curl -fsSL https://ragnito.github.io/open-interceptor/install.sh | bash
#
# (the same script is also served from
#  https://raw.githubusercontent.com/ragnito/open-interceptor/master/install.sh)
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
  version=''

  # Preferred: follow the /releases/latest redirect and read the tag out of
  # the final URL. Unlike api.github.com this is not rate limited, which
  # matters for an installer people run from shared IPs and CI.
  if command -v curl >/dev/null 2>&1; then
    effective=$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
      "https://github.com/$REPO/releases/latest" 2>/dev/null) || effective=''
    case "$effective" in
      */releases/tag/*) version="${effective##*/tag/}" ;;
    esac
  fi

  # Fallback: the JSON API (parsed without jq).
  if [ -z "$version" ]; then
    version=$(
      http_get "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null \
        | grep '"tag_name"' | head -n1 | cut -d'"' -f4
    ) || version=''
  fi

  [ -n "$version" ] || err "could not resolve the latest release tag.
  Check https://github.com/$REPO/releases, or pin a version:
    OPEN_INTERCEPTOR_VERSION=v1.0.3 sh install.sh"
fi

asset="$BIN-$target.tar.gz"
# The release workflow publishes the checksum as `<name>.sha256` (sibling of
# the tarball), NOT `<tarball>.sha256`. Keep this in sync with release.yml.
checksum_asset="$BIN-$target.sha256"
base="https://github.com/$REPO/releases/download/$version"

info "Installing $BOLD$BIN $version$RESET for $BOLD$target$RESET"

# ---- download + verify ----------------------------------------------------
tmp=$(mktemp -d 2>/dev/null || mktemp -d -t open-interceptor)
trap 'rm -rf "$tmp"' EXIT INT TERM

info "Downloading $asset ..."
http_dl "$base/$asset" "$tmp/$asset" \
  || err "download failed: $base/$asset"

# Every release published by our workflow ships a checksum, so a missing one
# means something is wrong with the download — fail closed rather than
# installing an unverified binary.
http_dl "$base/$checksum_asset" "$tmp/$checksum_asset" \
  || err "could not download the checksum: $base/$checksum_asset"

expected=$(cut -d' ' -f1 < "$tmp/$checksum_asset")
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
  # No hashing tool available; we cannot verify. Say so loudly.
  warn "neither sha256sum nor shasum found — could not verify the download"
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

# PATH check first: everything below assumes the binary is reachable.
case ":$PATH:" in
  *":$bin_dir:"*) : ;;
  *)
    warn "$bin_dir is not on your PATH"
    echo "    Add it to your shell profile (~/.zshrc, ~/.bashrc):"
    echo "      export PATH=\"$bin_dir:\$PATH\""
    echo
    ;;
esac

cfg="$HOME/.config/open-interceptor/config.yaml"

if [ -f "$cfg" ]; then
  ok "config found at $cfg"
  echo
  printf '%sNext steps%s\n' "$BOLD" "$RESET"
  echo "  Restart the daemon to pick up the new binary:"
  echo "       $BIN stop && $BIN start"
  echo "  Then run Claude Code through the proxy:"
  echo "       $BIN claude"
  echo
  ok "done"
  exit 0
fi

# First install: offer the guided setup. Under `curl | sh` this script's stdin
# is the pipe, not the keyboard, so the prompt and the wizard both have to read
# from the controlling terminal explicitly.
#
# `[ -r /dev/tty ]` is not enough: the node exists in containers and CI runners
# that have no controlling terminal, and opening it there fails with ENXIO.
# Actually open it (discarding the error) before offering anything.
if { : < /dev/tty; } 2>/dev/null; then
  printf '%sRun the guided setup now?%s [Y/n] ' "$BOLD" "$RESET"
  answer=''
  # A failed read means the terminal went away mid-prompt; treat it as "no".
  # An empty answer only counts as "yes" when the user pressed Enter.
  if read -r answer < /dev/tty; then
    case "$answer" in
      '' | y | Y | yes | YES)
        echo
        # exec: hand the terminal to the wizard and keep its exit code.
        exec "$bin_dir/$BIN" setup < /dev/tty
        ;;
    esac
  fi
  echo
fi

printf '%sNext steps%s\n' "$BOLD" "$RESET"
echo "  1. Configure providers and start the daemon (guided):"
echo "       $BIN setup"
echo
echo "  2. Then run Claude Code through the proxy:"
echo "       $BIN claude"
echo
echo "  Prefer to configure it by hand? See"
echo "       https://github.com/$REPO#install"
echo
ok "done"
