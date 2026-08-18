# API Overview

This page is the short map for Spooky's operator-facing programmatic surfaces.

Use the linked reference pages for canonical endpoint behavior, status codes, metrics, and configuration semantics.

## CLI Surface

Basic usage:

```bash
spooky --config /etc/spooky/config.yaml
```

Core options:

| Option | Meaning |
| --- | --- |
| `--config` / `-c` | Path to config file |
| `--version` / `-V` | Print version |
| `--help` / `-h` | Print usage |

## Surface Map

| Surface | Protocol | Main use | Canonical page |
| --- | --- | --- | --- |
| metrics endpoint | HTTP `GET` | scrape, dashboarding, alerting, trend analysis | [Metrics Reference](../reference/metrics-reference.md) |
| Control API | HTTP/1.1 over TLS | runtime state, staged activation, rollback, cert reload, restart | [Control API Reference](../reference/control-api-reference.md) |
| config file | YAML | runtime configuration input | [Configuration Reference](../configuration/reference.md) |

## Common Entry Points

| Task | Start here |
| --- | --- |
| check process liveness and readiness | `GET /health` and `GET /ready` in [Control API Reference](../reference/control-api-reference.md) |
| inspect active runtime state | `GET /admin/runtime` in [Control API Reference](../reference/control-api-reference.md) |
| validate, preview, activate, or roll back runtime config | [Control API Reference](../reference/control-api-reference.md) |
| understand metric names and labels | [Metrics Reference](../reference/metrics-reference.md) |
| use dashboards, alerts, and SLO views | [Observability Operator Bundle](../operations/observability-bundle.md) |
| understand reload, drain, and restart boundaries | [Reload and Drain](../operations/reload-and-drain.md) |
| understand exact config shape and examples | [Configuration Reference](../configuration/reference.md) and [Configuration Examples](../configuration/examples.md) |

## Scope Note

The Control API is a file-reload control surface, not a granular per-object mutation API.

For the canonical behavior of:

- `validate`, `preview`, `activate`, `rollback`, `reload`, `reload-certs`, and `restart`
- generation history and rollback eligibility
- authn, authz, and mTLS behavior
- response status and failure semantics

use [Control API Reference](../reference/control-api-reference.md).

For runtime-managed versus restart-required configuration boundaries, use [Reload and Drain](../operations/reload-and-drain.md).

## Related Pages

- [Control API Reference](../reference/control-api-reference.md)
- [Metrics Reference](../reference/metrics-reference.md)
- [Configuration Reference](../configuration/reference.md)
- [Observability Operator Bundle](../operations/observability-bundle.md)
- [Operations Runbook](../operations/runbook.md)
