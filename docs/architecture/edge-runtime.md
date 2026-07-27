# Edge Runtime Ownership

This document explains how `spooky-edge` is organized after the refactor work that split runtime ownership, listener orchestration, backend lifecycle management, and control-plane services into clearer boundaries.

## Purpose

`spooky-edge` is the data-plane crate for Spooky. It owns:

- edge-facing runtime state
- QUIC ingress and bootstrap compatibility ingress
- request admission, routing, forwarding, and response emission orchestration
- backend lifecycle coordination and health feedback
- operator-facing services such as metrics, control API wiring, and watchdog coordination

The important distinction is that `spooky-edge` no longer exposes its internal runtime mechanics directly. The crate root presents a narrow public surface, and most operational machinery lives behind internal subsystem modules.

## Public Façade vs Internal Subsystems

### Public crate façade

The crate root in `crates/edge/src/lib.rs` is the public entrypoint for the rest of the workspace. It intentionally exposes only stable cross-crate surfaces such as:

- shared metrics and observability reason vocabularies
- runtime state entrypoints under `edge::runtime`
- routing and resilience domain APIs
- worker/runtime startup entrypoints that other crates need to boot the edge

External callers should start from the crate root and `edge::runtime`, not from `quic_listener` internals.

### Internal listener subsystem façade

`crates/edge/src/quic_listener/mod.rs` is an internal subsystem façade. It owns:

- listener startup and bind orchestration
- worker lifecycle and shard runtime wiring
- QUIC request ingress
- bootstrap compatibility ingress
- control-plane service startup glue
- drain and shutdown coordination

It is intentionally internal because it composes many lower-level modules that should not become part of the stable crate API.

## Ownership Map

### `edge::lib`

Owns the public crate surface.

Use it for:

- stable re-exports
- crate-level docs
- public observability contracts

Do not put request-path logic or runtime orchestration here.

### `edge::runtime`

Owns the runtime state model.

This module is the stable place for:

- runtime bundle and generation ownership
- listener runtime state types
- backend lifecycle state and coordination
- shared runtime services
- health and policy state that must survive beyond one request path

`edge::runtime` should answer questions such as:

- what is startup-owned
- what is reloadable generation-owned state
- what survives across runtime generations
- what lifecycle state exists for a backend

### `edge::quic_listener`

Owns ingress/runtime orchestration, not durable runtime ownership.

This subsystem should:

- build listener execution contexts from runtime state
- accept client traffic
- translate ingress events into shared policy and transport calls
- coordinate shutdown and worker lifecycle

This subsystem should not become the long-term home of policy interpretation, backend lifecycle ownership, or transport-specific behavior.

### `edge::watchdog`

Owns watchdog coordination as an operator-facing service.

This includes:

- watchdog state and coordination loops
- service-facing state views
- operational safety checks that are not request hot-path logic

### `edge::metrics` and observability vocabularies

Own shared metrics storage plus the stable reason vocabularies used by:

- admission decisions
- retries and hedges
- backend health transitions
- route/request outcomes
- overload and operational events

These vocabularies matter because metrics, logs, and control-plane snapshots should describe the same events in the same terms.

## Runtime Ownership Model

At a high level, the runtime is split into three categories:

### Startup-owned state

This is fixed when the process or listener group starts and should not silently change during a runtime generation swap.

Examples:

- bound sockets and listener topology
- worker layout and shard startup wiring
- process-level startup services

### Generation-owned state

This is the active runtime configuration and derived state that can be replaced on reload.

Examples:

- routing index
- upstream policies
- listener runtime configs
- admission and resilience policy snapshots
- generation-scoped pools, semaphores, and backend maps

### Long-lived shared services

These survive across generation swaps and are shared by multiple services.

Examples:

- metrics
- TLS reload stores
- watchdog coordination
- runtime bundle handle
- long-lived resolution or transport service shells

## Request-Path Ownership

The request path is intentionally layered:

1. Ingress mechanics live in `quic_listener`.
2. Runtime-owned policy and lifecycle state live in `runtime`.
3. Request building and response normalization live in `bridge`.
4. Backend transport execution lives in `transport`.
5. Balancing/accounting substrate lives in `lb`.

That split matters because new features should extend the owning layer instead of recreating policy branches in listener code.

## Control-Plane Ownership

Admin and operator surfaces should depend on canonical runtime views, not on listener-local state.

That includes:

- control API snapshots
- metrics endpoint rendering
- watchdog coordination
- reload/drain reporting

If a control-plane service needs information, that information should come from the runtime bundle or shared service views, not from ad hoc listener internals.

## Public vs Internal Rule of Thumb

Make something public only if another crate needs it as a stable contract.

Keep it internal if it is:

- orchestration glue
- module-local policy plumbing
- listener mechanics
- transport/protocol implementation detail
- compatibility-path helper logic

## Contributor Guidance

When adding code:

- put runtime ownership and reload semantics in `edge::runtime`
- put ingress orchestration in `edge::quic_listener`
- put operator services in `control_api`, `metrics`, or `watchdog`
- keep the crate root limited to stable entrypoints and shared contracts

If a change makes `quic_listener` smarter by embedding more policy or lifecycle ownership, it probably belongs somewhere else.
