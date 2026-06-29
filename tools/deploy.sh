#!/bin/bash
# deploy.sh — Build, install and restart the open-interceptor daemon.
#
# Cross-platform: macOS (launchd) and Linux (systemd user service). The
# platform-specific service wiring lives in the `open-interceptor` binary
# itself (`start --install`); this script only builds, places the binary and
# drives the lifecycle commands.
#
# Usage:
#   ./tools/deploy.sh            # default: release build, ~/.local/bin
#   ./tools/deploy.sh --debug    # debug build
#   ./tools/deploy.sh --binary /path/to/binary  # use existing binary
#
# What this script does:
#   1. Build (release by default) the open-interceptor binary
#   2. Copy it to ~/.local/bin/open-interceptor
#   3. Stop the running daemon (if any)
#   4. Reinstall the service definition and start the daemon
#   5. Wait for health check and report
#
# Why not `cargo install`? Because we need the binary in a known location
# (~/.local/bin) so the service definition can reference it with an absolute
# path. launchd does NOT inherit the user's PATH, and a systemd unit's
# ExecStart also needs an absolute path — neither can resolve a bare
# "open-interceptor" name.

set -e

OS="$(uname -s)"

BUILD_MODE=release
BINARY_SOURCE="build"

while [[ $# -gt 0 ]]; do
  case $1 in
    --debug)
      BUILD_MODE=debug
      shift
      ;;
    --binary)
      BINARY_SOURCE="$2"
      shift 2
      ;;
    *)
      echo "Unknown option: $1"
      echo "Usage: $0 [--debug] [--binary /path/to/binary]"
      exit 1
      ;;
  esac
done

BINARY_PATH="${HOME}/.local/bin/open-interceptor"
mkdir -p "$(dirname "$BINARY_PATH")"

echo "=== open-interceptor deploy (${OS}) ==="

# 1. Build or use provided binary
if [[ "$BINARY_SOURCE" == "build" ]]; then
  echo "[1/4] Building ($BUILD_MODE)..."
  if [[ "$BUILD_MODE" == "release" ]]; then
    cargo build --release --bin open-interceptor
    cp target/release/open-interceptor "$BINARY_PATH"
  else
    cargo build --bin open-interceptor
    cp target/debug/open-interceptor "$BINARY_PATH"
  fi
else
  echo "[1/4] Using provided binary: $BINARY_SOURCE"
  cp "$BINARY_SOURCE" "$BINARY_PATH"
fi
echo "    → installed to $BINARY_PATH"

# 2. Stop daemon
echo "[2/4] Stopping daemon..."
if open-interceptor stop 2>/dev/null; then
  echo "    → stopped"
else
  echo "    → not running (ok)"
fi

# 3. Reinstall the service definition with current binary path
#    (--install also starts the daemon)
echo "[3/4] Reinstalling service definition..."
open-interceptor start --install 2>&1 | sed 's/^/    /'

# Give the service manager + the daemon a moment to initialize
sleep 5

# 4. Wait for health check
# The health endpoint may return 503 while providers are being probed (that's ok -
# it means the server is up but still starting). We check for 200 OR 503.
echo "[4/4] Waiting for daemon to be ready..."
DEADLINE=$(($(date +%s) + 20))
HEALTHY=false
while [[ $(date +%s) -lt $DEADLINE ]]; do
  # Note: curl -f exits 22 on 503, which would trigger set -e — use || true to allow the capture
  HTTP_CODE=$(curl -sf --noproxy '*' -o /dev/null -w '%{http_code}' http://127.0.0.1:3300/healthz 2>/dev/null || true)
  if [[ "$HTTP_CODE" == "200" ]] || [[ "$HTTP_CODE" == "503" ]]; then
    HEALTHY=true
    break
  fi
  sleep 1
done

if $HEALTHY; then
  echo "    → daemon ready at http://127.0.0.1:3300"
  curl -sf --noproxy '*' http://127.0.0.1:3300/healthz | python3 -m json.tool 2>/dev/null || true
else
  echo "    ⚠️  daemon not responding after 20s"
  echo "    Check: open-interceptor status"
  echo "    Check: open-interceptor logs"
  if [[ "$OS" == "Darwin" ]]; then
    echo "    Check: launchctl print gui/\$(id -u)/com.open-interceptor"
  else
    echo "    Check: systemctl --user status open-interceptor"
    echo "    Check: journalctl --user -u open-interceptor -e"
  fi
  exit 1
fi

echo "=== deploy complete ==="
