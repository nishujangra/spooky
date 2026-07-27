# Runtime Generation Model

This document explains how `spooky-edge` owns live runtime state, how reloads replace that state, and how workers and control-plane services are expected to read it.

## Purpose

The runtime generation model exists to answer three questions clearly:

- what is fixed at startup
- what can change on reload
- what must survive across reloads

The canonical implementation lives under:

- `crates/edge/src/runtime/bundle.rs`
- `crates/edge/src/runtime/generation.rs`
- `crates/edge/src/runtime/shared_state.rs`
- `crates/edge/src/runtime/tasks.rs`

## Ownership Classes

The runtime is intentionally split into three ownership classes.

### Startup-owned state

Startup-owned state is established once for the process and must not change while the process keeps running.

Examples:

- config path
- non-reloadable logging sink configuration

This state is represented by `StartupOwnedRuntimeState`.

If a reload would require changing startup-owned state, it should be rejected as restart-required rather than silently applied.

### Generation-owned state

Generation-owned state is the live, reloadable runtime snapshot used by workers and operator surfaces.

Examples:

- listener runtime configs
- routing index
- upstream policies
- backend endpoint maps
- backend health-check definitions
- per-upstream pools and inflight semaphores
- resilience and admission state
- generation-scoped background task registry

This state is represented by `RuntimeGenerationState`.

When a reload is committed, this is the state that gets replaced.

### Long-lived shared services

Long-lived shared services are the services the data plane reaches through the active generation but which are not modeled as per-request state.

Examples:

- metrics
- listener TLS reload store
- upstream transport pool shell
- backend lifecycle coordinator
- backend resolution store
- watchdog coordinator
- shared DNS resolver

This state is represented by `RuntimeSharedServices`.

Some of these services are rebuilt from config on reload, while others are deliberately carried forward so in-flight operational state survives the swap.

## Core Types

### `RuntimeBundle`

`RuntimeBundle` is the canonical published unit of runtime state. It contains:

- generation number
- startup-owned state
- runtime config snapshot
- shared runtime state

This is the boundary between assembly time and live execution time.

### `SharedRuntimeState`

`SharedRuntimeState` groups:

- `RuntimeSharedServices`
- `RuntimeGenerationState`

This gives workers and service surfaces one stable handoff point without exposing the internal reload planner.

### `RuntimeGenerationView`

`RuntimeGenerationView` is the canonical read-only view for callers that need the active runtime state without caring about swap mechanics.

It bundles:

- generation id
- startup-owned state
- runtime config
- shared services
- generation-owned state

Listener workers, metrics rendering, control-plane surfaces, and bootstrap compatibility code should prefer a generation view over reaching into internal storage directly.

### `RuntimeBundleHandle`

`RuntimeBundleHandle` is the active-generation publisher and read interface.

Its responsibilities are:

- expose the current active generation
- provide stable read helpers such as `current_view()` and `with_current_view(...)`
- gate reload commits against lifecycle phase
- atomically replace the active bundle
- retire the previous generation's generation-scoped tasks

## Read Path

The intended read path is:

1. caller obtains `RuntimeBundleHandle`
2. caller asks for `current_view()` or `with_current_view(...)`
3. caller reads runtime state through `RuntimeGenerationView`

Callers should not manually reconstruct active runtime state by following ad hoc chains through listener-local fields.

This keeps control-plane, listener, and metrics code aligned on the same active generation contract.

## Reload Flow

At a high level, reload follows this sequence:

1. build a new runtime bundle from the new config
2. carry forward any process-scoped shared services that must survive reload
3. validate that startup-owned state did not change in a reload-illegal way
4. stage generation-owned state and generation-scoped tasks
5. atomically replace the active bundle in `RuntimeBundleHandle`
6. retire the previous generation's generation task registry

The critical point is that readers only ever observe whole bundles. There is no partial in-place mutation of the live generation.

## Bundle Replacement Semantics

Bundle replacement is intentionally narrow.

### What replacement changes

- generation-owned runtime state
- the active runtime config snapshot
- any shared services that are intentionally rebuilt with the generation

### What replacement must not silently change

- startup-owned state
- process identity assumptions
- restart-required ownership domains

### What survives replacement

- readers holding the old `Arc<RuntimeBundle>` until they finish
- process-scoped services that are explicitly carried forward
- operational lifecycle phase state shared through the handle

## Task Ownership

Generation-scoped background tasks belong to `RuntimeTaskRegistry` inside `RuntimeGenerationState`.

That means:

- tasks are owned by the generation that created them
- retiring old generation tasks happens in one place during bundle replacement
- task retirement should not be scattered across unrelated reload callers

This avoids ambiguous ownership over which generation is responsible for stopping background work.

## Lifecycle Gating

Reload is not allowed in every process phase.

`RuntimeBundleHandle` owns the runtime lifecycle gate and rejects reload commits once drain or shutdown has begun.

That matters because it prevents:

- installing a fresh generation during shutdown
- racing reload and drain into inconsistent state
- forcing leaf callers to implement their own shutdown/reload conflict logic

## Startup, Reload, and Shutdown Roles

### Startup path

Startup assembles the first runtime bundle and publishes generation 0 or generation 1 as the initial active state.

### Reload path

Reload prepares a complete next generation and swaps the bundle if lifecycle gating and validation allow it.

### Shutdown and drain path

Drain and shutdown do not mutate the active generation in place to express policy changes. They move lifecycle phase forward and let listener/control-plane services react accordingly.

## Contributor Rules

When adding code:

- put new reloadable runtime state in `RuntimeGenerationState`
- put fixed process-start state in `StartupOwnedRuntimeState`
- put shared services in `RuntimeSharedServices`
- expose read access through `RuntimeGenerationView`
- keep bundle replacement in `RuntimeBundleHandle`

Do not:

- add new ad hoc active-runtime lookup paths
- let listener code invent its own startup-vs-reload fallback rules
- spread generation-task retirement across multiple modules

## Mental Model

The simplest way to think about the runtime is:

- `RuntimeBundle` is what the process is currently serving
- `RuntimeBundleHandle` decides which bundle is live
- startup-owned state defines what cannot change without restart
- generation-owned state defines what reload replaces
- shared services define what the running process keeps reaching through

If a contributor can answer those questions for a new piece of state, it will land in the right part of the runtime model.
