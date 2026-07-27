# Reload and Drain

This document explains what changes live in Spooky, what requires restart, and what operators should expect during reload and drain workflows.

## Purpose

Spooky now has a clearer runtime generation model:

- startup-owned state
- generation-owned state
- long-lived shared services

Reload and drain behavior follows that model.

## Runtime Reload Model

Runtime reload works by preparing a complete next runtime bundle and atomically swapping it into place.

Important properties:

- readers observe whole bundles, not partial in-place mutation
- the previous generation remains valid for in-flight work that still holds it
- generation-scoped tasks for the previous bundle are retired after replacement

This means reload is a generation swap, not a best-effort patch of live state.

## What Reload Changes Live

Runtime reload is intended for changes such as:

- routing and upstream policy changes
- backend set changes
- load-balancing changes
- admission and resilience policy changes
- runtime timeout and transport policy changes that are modeled as reloadable generation state
- `log.level` changes

When the reload succeeds:

- new requests use the new generation immediately
- in-flight work on the old generation drains naturally

## What Does Not Reload Live

Some changes are restart-required because they belong to startup-owned or separately bound service domains.

Examples include:

- non-live logging sink settings such as log file path and log format
- tracing enablement and related startup-owned tracing settings
- some bind and service-surface compatibility changes that require new listeners or rebind validation

The system should reject these as incompatible reloads rather than partially applying them.

## Reload Compatibility Checks

Reload compatibility is validated centrally before swap.

The control-plane reload flow checks:

- listener/runtime compatibility
- control API compatibility
- metrics endpoint compatibility
- startup-owned state compatibility

Operators should expect:

- an explicit rejection when a reload is not safe
- the currently active generation to remain authoritative when rejection occurs

## Control API Reload Endpoints

Two control-plane actions matter here:

- runtime reload
- certificate reload

### Runtime reload

This rebuilds and validates a new runtime generation and swaps it if allowed.

### Certificate reload

This reloads listener TLS material for new handshakes only. It is not the same as full runtime config reload.

## Runtime Generation Visibility

Operators should verify reload outcomes using:

- control API runtime generation id
- reload response payloads
- logs around reload rejection or generation swap

Do not assume that file edits on disk imply the active runtime changed. The authoritative signal is the committed active generation.

## Drain Semantics

Drain is the controlled path for stopping new useful work while letting existing work finish when possible.

At a high level, drain means:

- listener enters draining mode
- existing active streams are allowed to finish if possible
- if no active streams remain, connections are closed and the listener can stop
- if the drain timeout expires, remaining connections are closed

Drain is part of graceful shutdown and also part of watchdog-driven restart workflows.

## Watchdog-Driven Restart Flow

The watchdog can request a restart when runtime progress degrades beyond policy thresholds.

The expected flow is:

1. watchdog requests restart and records a reason
2. listener sees the restart request and enters draining mode
3. workers eventually report drained state
4. watchdog completes the restart cycle once the workflow is done

Operators should expect readiness and runtime snapshot state to reflect this transition rather than needing to infer it from scattered logs.

## What Happens to In-Flight Requests

### On reload

- existing requests continue on the generation they started with
- new requests use the new generation after the swap

### On drain

- active requests may complete if they finish before drain timeout and before forced close
- if graceful completion is not possible, remaining connections are closed

This is why generation ownership and drain lifecycle are distinct concerns.

## Operator Expectations

Operators should expect reload to answer:

- did compatibility validation pass
- was a new generation committed
- what generation is active now

Operators should expect drain to answer:

- is draining active
- did the watchdog request it
- how long has restart been pending
- have workers drained yet

## Recommended Workflow

For routine runtime changes:

1. update config on disk
2. call runtime reload
3. verify the active generation changed
4. verify control API snapshot and metrics align with expected state

For restart-required changes:

1. stage the config change
2. use maintenance orchestration or restart workflow
3. treat the restart as a controlled drain event, not as a hot reload

For certificate-only changes:

1. update TLS material
2. use certificate reload
3. verify new handshakes use the refreshed material

## Failure Expectations

If reload fails:

- active generation remains unchanged
- rejection should be explicit
- operators should not assume partial progress

If drain times out:

- remaining connections are closed
- the process should still complete shutdown/restart progression rather than hanging indefinitely

## Contributor Rules

When adding new reloadable state:

- decide whether it is startup-owned, generation-owned, or process-shared
- make reload compatibility explicit
- expose reload result through canonical runtime generation/state surfaces

Do not:

- add silent partial reload behavior
- let control-plane code invent its own reload compatibility rules outside the canonical path
- mutate active runtime state in place when the generation model expects replacement

## Related Pages

- [Runtime Generation Model](../architecture/runtime-generation.md)
- [Control Plane](control-plane.md)
- [Control API Reference](../reference/control-api-reference.md)
