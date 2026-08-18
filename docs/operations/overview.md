# Operations Overview

This section is the main entry point for deploying, rolling out, operating, and recovering Spooky in production.

Use it to answer:

- where Spooky fits well today
- how to prepare hosts and capacity
- how to roll out config and binary changes safely
- how reload, drain, rollback, and restart actually work
- where to look during an incident

## Start Here

| Goal | Document |
|---|---|
| Decide whether the current release is ready for your environment | [Production Readiness](production-readiness.md) |
| Prepare a production host and service layout | [Production Deployment](../deployment/production.md) |
| Understand safe activation, restart-required changes, drain, and rollback | [Reload and Drain](reload-and-drain.md) |
| Plan host sizing and concurrency limits | [Sizing And Capacity](sizing-and-capacity.md) |
| Tune the host OS and runtime environment | [Host Tuning](host-tuning.md) |
| Choose a rollout shape | [Deployment Patterns](deployment-patterns.md) |
| Validate before and after a change | [Validation](../deployment/validation.md) |
| Troubleshoot incidents quickly | [Runbook](runbook.md) |
| Interpret visible failures and status codes | [Failure Modes](failure-modes.md) |
| Operate distributed quota safely | [Distributed Quota](distributed-quota.md) |
| Use the shipped dashboards, alerts, and SLO views | [Observability Operator Bundle](observability-bundle.md) |

## Core Operating Model

Spooky has three distinct change paths:

1. Runtime-managed config changes
   Use the Control API staged flow: `validate`, `preview`, then `activate`. This is the normal path for routes, upstreams, backends, timeouts, resilience policy, and other live-reloadable runtime state.
2. Certificate-only changes
   Use `POST /admin/runtime/reload-certs`. This updates listener TLS material for new handshakes only.
3. Restart-required changes
   Use a drain-aware restart or instance replacement workflow when the change affects startup-owned state such as listener bind changes, control-plane bind changes, tracing startup settings, or logging sink configuration.

Do not treat all changes as restarts, and do not treat all changes as live-reloadable.

## Common Workflows

### Deploy a new environment

Start with:

- [Production Readiness](production-readiness.md)
- [Production Deployment](../deployment/production.md)
- [Host Tuning](host-tuning.md)
- [Sizing And Capacity](sizing-and-capacity.md)

### Roll out a runtime config change

Start with:

- [Validation](../deployment/validation.md)
- [Reload and Drain](reload-and-drain.md)
- [Runbook](runbook.md)

### Roll out a binary upgrade or restart-required config change

Start with:

- [Deployment Patterns](deployment-patterns.md)
- [Production Deployment](../deployment/production.md)
- [Reload and Drain](reload-and-drain.md)

### Investigate production failures

Start with:

- [Runbook](runbook.md)
- [Failure Modes](failure-modes.md)
- [Observability Operator Bundle](observability-bundle.md)
- [Troubleshooting](../troubleshooting/common-issues.md)

## Operator Rules

- Keep the Control API on loopback or a strongly isolated admin network.
- Use `--http1.1` for all `curl` calls to the Control API.
- Prefer `validate` and `activate` over the legacy `reload` shortcut in production automation.
- Pass `expected_generation` on activation and rollback workflows so concurrent changes fail safely.
- Keep at least one known-good rollback target and one known-good binary available during every rollout.
- Treat quota denials and overload shedding as separate operational signals.
