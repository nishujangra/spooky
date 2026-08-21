#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${ROOT_DIR}/target/debug"

H2_BACKEND="${BIN_DIR}/h2_backend"
H3_CLIENT="${BIN_DIR}/h3_client"
IMPULSE_BIN="${BIN_DIR}/impulse"

ensure_bins() {
  if [[ ! -x "${H2_BACKEND}" || ! -x "${H3_CLIENT}" || ! -x "${IMPULSE_BIN}" ]]; then
    (cd "${ROOT_DIR}" && cargo build -p impulse)
  fi
}

start_backend() {
  local port="$1"
  "${H2_BACKEND}" --port "${port}" >"/tmp/impulse-backend-${port}.log" 2>&1 &
  echo $!
}

start_impulse() {
  local cfg="$1"
  RUST_LOG=info "${IMPULSE_BIN}" --config "${cfg}" >"/tmp/impulse-edge.log" 2>&1 &
  echo $!
}

run_client() {
  local host="$1"
  "${H3_CLIENT}" --connect 127.0.0.1:9889 --host "${host}" --path / --insecure >/tmp/impulse-client.out 2>&1 || true
}

print_selection_log() {
  if [[ -f /tmp/impulse-edge.log ]]; then
    rg "Selected backend" /tmp/impulse-edge.log || true
  fi
}

cleanup_pids() {
  for pid in "$@"; do
    if [[ -n "${pid}" ]]; then
      kill "${pid}" 2>/dev/null || true
    fi
  done
}
