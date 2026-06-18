# Control API Reference

This page documents the current operator-facing control-plane endpoints and their intended use.

## Scope

The control API is a privileged admin surface. It should be treated as operator-only infrastructure, not as a public application endpoint.

Current endpoint family:

- health
- readiness
- runtime snapshot
- certificate reload
- restart request

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
- use drain-and-restart for route, upstream, timeout, or policy changes
- keep rollback available before using restart-triggering control-plane actions in production

## Related Pages

- [Metrics Reference](metrics-reference.md)
- [Production Readiness](../operations/production-readiness.md)
- [Operations Runbook](../operations/runbook.md)
