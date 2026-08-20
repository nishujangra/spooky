# Reload and Drain

This document explains which changes activate live, which changes require a restart, and what operators should expect from reload, rollback, cert reload, and drain workflows.

## Core Rule

Spooky does not patch runtime state in place. It prepares a complete next runtime generation and swaps it atomically when the change is compatible.

That means:

- readers observe complete generations, not partial mutation
- in-flight requests continue on the generation they started with
- rejected activations leave the active generation unchanged

## Preferred Runtime Change Workflow

For normal runtime-managed changes, use the staged Control API flow:

1. `POST /admin/runtime/validate`
   Confirms whether the candidate config can be activated and reports rejected domains.
2. `POST /admin/runtime/preview`
   Produces the same planning result, but records the attempt in operator history.
3. `POST /admin/runtime/activate`
   Commits the compatible candidate generation.
4. `GET /admin/runtime/history`
   Confirms the active generation and retains a rollback target.

Use `expected_generation` on activation so concurrent changes fail with `409` instead of silently overwriting each other.

## What Activates Live

Live activation is the normal path for runtime-managed domains such as:

- routes and upstream definitions
- backend sets and backend weights
- load-balancing policy
- admission, overload, retry, hedge, and quota policy
- runtime timeout and transport policy
- `log.level`

After a successful activation:

- new requests use the new generation immediately
- existing requests continue on the previous generation until they complete or are otherwise terminated by normal request-path behavior

## What Requires Restart

Some changes are restart-required because they affect startup-owned or separately bound service domains.

Plan a drain-aware restart or node replacement for changes such as:

- listener removal
- listener bind-address or port changes
- control-plane or metrics bind changes
- logging sink settings such as log format or file output
- tracing startup configuration
- control-plane thread-count changes

These changes should be rejected during activation planning, not partially applied.

## Legacy Runtime Reload Shortcut

`POST /admin/runtime/reload` still exists, but it is a compatibility shortcut.

Use it only when:

- you intentionally want a direct apply path
- you do not need the fuller staged diff and rejection reporting from `activate`

For production automation, prefer `validate` plus `activate`.

## Certificate Reload

`POST /admin/runtime/reload-certs` is only for listener TLS material and related trust material used by new downstream handshakes.

It does not:

- rebuild the full runtime generation
- mutate route or policy state
- change existing live sessions

Use it for certificate rotation when only the cert or trust material changed.

Upstream client certificates, upstream CA bundles, and other secret-backed upstream TLS material are never rotated through `reload-certs` — they are generation-owned and go through `validate`/`preview`/`activate` instead. See [Secret and Certificate Rotation](secret-and-cert-rotation.md) for the full rotation and rollback runbook.

## Rollback

Rollback restores a previously retained runtime generation through `POST /admin/runtime/rollback`.

Operator rules:

- choose a target from `GET /admin/runtime/history`
- confirm `rollback_candidate: true`
- pass `expected_active_generation`
- treat rollback as a first-class production workflow, not as a last-minute improvisation

Retained generations are bounded. Do not assume very old generations are still available.

## Drain Semantics

Drain is the controlled path for stopping admission of new useful work while allowing existing work to finish when possible.

At a high level:

- the listener enters draining mode
- new connection or request admission is reduced or stopped according to lifecycle behavior
- in-flight work is given time to finish
- when the drain timeout is reached, remaining work is force-closed

Drain is distinct from activation:

- activation swaps runtime generations
- drain moves the process toward restart or shutdown

## Restart Workflow

A restart can be operator-initiated or watchdog-initiated, but operators should expect the same high-level lifecycle:

1. restart is requested
2. the runtime enters draining mode
3. workers drain or the configured drain grace elapses
4. the process or node completes the restart workflow

Use restart for:

- binary upgrades
- restart-required config changes
- controlled maintenance windows

## What Operators Should Verify

After activation:

- the active generation changed as expected
- runtime history recorded the action
- the diff matches the intended change
- route, backend, quota, and overload metrics still look healthy

After cert reload:

- new handshakes present the expected certificate
- certificate-expiry and handshake dashboards stay healthy

During drain:

- readiness state reflects the transition
- worker drain progresses
- drain duration stays within the configured budget
- remaining traffic is handled by other healthy instances

## Failure Expectations

If activation fails:

- the active generation remains unchanged
- the rejection should identify the incompatible or invalid domain
- operators should correct the config or use a restart path if the change is restart-required

If rollback fails:

- inspect retained-generation history first
- confirm the target still has a retained bundle
- confirm the active generation did not move unexpectedly

If drain times out:

- remaining connections are closed
- the workflow should still complete rather than hanging indefinitely
- treat repeated drain timeout as a signal to inspect long-lived streams, drain budget, and restart policy

## Related Pages

- [Production Deployment](../deployment/production.md)
- [Production Readiness](production-readiness.md)
- [Runbook](runbook.md)
- [Secret and Certificate Rotation](secret-and-cert-rotation.md)
- [Control API Reference](../reference/control-api-reference.md)
