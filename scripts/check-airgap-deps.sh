#!/usr/bin/env bash
# Fail if the airgap shell's normal dependency tree pulls in network or bridge crates.
#
# dioxus-desktop / dioxus-devtools pull tungstenite + native-tls for hot-reload
# websockets. Those are allowed when they appear only under dioxus*. Everything
# else on the phase-2 forbidden list must not appear at all.
set -euo pipefail

cd "$(dirname "$0")/.."

# Ensure cargo is on PATH when this runs under Git Bash on Windows.
if ! command -v cargo >/dev/null 2>&1; then
  if [ -n "${CARGO_HOME:-}" ] && [ -x "${CARGO_HOME}/bin/cargo" ]; then
    export PATH="${CARGO_HOME}/bin:${PATH}"
  elif [ -x "${HOME}/.cargo/bin/cargo" ]; then
    export PATH="${HOME}/.cargo/bin:${PATH}"
  elif [ -x /c/Users/*/.cargo/bin/cargo ]; then
    # shellcheck disable=SC2086
    export PATH="$(echo /c/Users/*/.cargo/bin | awk '{print $1}'):${PATH}"
  fi
fi

ALWAYS='reqwest|hyper|solana-client|solana-rpc-client|interprocess|wallet-rpc|bridge-host'
# Plan also lists these; they may appear only via dioxus*.
VIA_DIOXUS='tokio-tungstenite tungstenite rustls native-tls'

tree="$(cargo tree --locked -p airlock-offline -e normal)"

if echo "$tree" | grep -E "$ALWAYS"; then
  echo "::error::airlock-offline depends on a forbidden network or bridge crate"
  exit 1
fi

for pkg in $VIA_DIOXUS; do
  inv="$(cargo tree --locked -p airlock-offline -e normal -i "$pkg" 2>/dev/null || true)"
  if [ -z "$inv" ]; then
    continue
  fi
  # Any path that names our online/network crates is a failure.
  if echo "$inv" | grep -E 'wallet-rpc|bridge-host|interprocess|app-online|reqwest|hyper|solana-client|solana-rpc-client'; then
    echo "::error::${pkg} is reachable outside dioxus in airlock-offline"
    echo "$inv"
    exit 1
  fi
done

echo "airlock-offline normal deps are clean of network and bridge crates."
