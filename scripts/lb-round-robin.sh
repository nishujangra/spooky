#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/common.sh"

ensure_bins

cat > /tmp/impulse-lb-rr.yaml <<'YAML'
version: 1

listen:
    protocol: http3
    port: 9889
    address: "127.0.0.1"
    tls:
        cert: "certs/proxy-cert.pem"
        key: "certs/proxy-key-pkcs8.pem"

upstream:
    default:
        load_balancing:
            type: round-robin
        route:
            path_prefix: "/"
        backends:
            -   id: "backend1"
                address: "http://127.0.0.1:8081"
                weight: 1
                health_check:
                    path: "/health"
                    interval: 5000
            -   id: "backend2"
                address: "http://127.0.0.1:8082"
                weight: 1
                health_check:
                    path: "/health"
                    interval: 5000
            -   id: "backend3"
                address: "http://127.0.0.1:8083"
                weight: 1
                health_check:
                    path: "/health"
                    interval: 5000

log:
  level: info
YAML

B1=$(start_backend 8081)
B2=$(start_backend 8082)
B3=$(start_backend 8083)
IMPULSE=$(start_impulse /tmp/impulse-lb-rr.yaml)

sleep 1

for i in {1..6}; do
  run_client "rr-test"
done

sleep 1
print_selection_log

cleanup_pids "${IMPULSE}" "${B1}" "${B2}" "${B3}"
