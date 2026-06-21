# Control API Reference

This page documents the current operator-facing control-plane endpoints and their intended use.

## Scope

The control API is a privileged admin surface. It should be treated as operator-only infrastructure, not as a public application endpoint.

Current endpoint family:

- health
- readiness
- runtime snapshot
- certificate reload
- full config reload
- restart request

## Protocol

The control API uses **HTTP/1.1 over TLS**. HTTP/2 is not supported.

When using curl, pass `--http1.1` explicitly — curl negotiates h2 by default when connecting to a TLS endpoint and the server will reject the connection:

```bash
curl -k --http1.1 https://<address>:<port>/...
```

The `-k` flag skips certificate verification for self-signed certs.

## Security Expectations

- bind to loopback or a strongly isolated admin network whenever possible
- require a strong bearer token for privileged endpoints
- avoid broad public exposure even when authentication is enabled

## Authentication

Privileged endpoints use bearer-token authentication via:

```http
Authorization: Bearer <token>
```

The token is configured with `observability.control_api.auth_token`.

## Endpoints

### `GET /health`

Purpose:

- liveness check
- watchdog state visibility

Expected use:

- load balancer or platform liveness probe
- operator sanity check

### `GET /ready`

Purpose:

- readiness state for serving traffic

Expected use:

- deployment orchestration
- maintenance and rollout checks

### `GET /admin/runtime`

Purpose:

- runtime snapshot for operators

Typical contents include:

- worker and runtime state
- key counters
- admission state
- backend health summary

Expected use:

- debugging
- rollout validation
- incident response

### `POST /admin/runtime/reload`

Purpose:

- reload the full config from disk and apply changes to upstreams, backends, policies, and timeouts

Important scope note:

- listener bind addresses, control API bind, and metrics bind cannot change without a restart
- in-flight requests on the old config complete normally; new requests use the new config immediately

Expected use:

- adding or removing backends
- changing load balancing, timeouts, resilience, or routing policy at runtime

Example:

```bash
curl -k --http1.1 -X POST https://127.0.0.1:9890/admin/runtime/reload \
  -H "Authorization: Bearer <token>"
```

### `POST /admin/runtime/reload-certs`

Purpose:

- reload listener certificate and related trust material for **new handshakes**

Important scope note:

- this is not full config hot reload
- existing sessions keep their already-negotiated certificate and auth state

Expected use:

- listener certificate rotation
- listener trust-material refresh

### `POST /admin/runtime/restart`

Purpose:

- request a controlled restart/drain workflow through the watchdog coordinator

Expected use:

- operational restart requests
- orchestrated maintenance flow

## Operator Notes

- use cert reload for cert-only changes
- use full config reload for backend, policy, timeout, or routing changes that don't require rebinding listeners
- use drain-and-restart when listener addresses or control API/metrics bind must change
- keep rollback available before using restart-triggering control-plane actions in production
- all curl invocations must use `--http1.1` — the control API does not support HTTP/2

## Related Pages

- [Metrics Reference](metrics-reference.md)
- [Production Readiness](../operations/production-readiness.md)
- [Operations Runbook](../operations/runbook.md)
