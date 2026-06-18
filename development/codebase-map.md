# Codebase Map

This page maps the repository by crate and by major responsibility so contributors know where a change belongs before touching code.

## Workspace Layout

| Path | Responsibility |
| --- | --- |
| `spooky/` | Binary entrypoint, process bootstrap, worker orchestration, privilege-drop helpers |
| `crates/config/` | YAML schema, defaults, loader, runtime normalization, validation |
| `crates/edge/` | Main edge runtime: QUIC/H3 ingress, routing, resilience, streaming, metrics, control-plane hooks |
| `crates/bridge/` | H3-to-H2 request adaptation and forwarding-header policy |
| `crates/transport/` | Upstream H2 clients, pooling, upstream TLS handling, DNS integration |
| `crates/lb/` | Backend health state and load-balancing algorithms |
| `crates/utils/` | Logging and tracing helpers |
| `crates/errors/` | Shared error taxonomy |
| `crates/bench/` | Benchmark harness and regression-gate tooling |
| `config/` | Example configs |
| `docs/` | Operator, reference, architecture, and contributor docs |
| `packaging/` | Docker, Debian, and systemd-related packaging assets |
| `scripts/` | Load, benchmark, and operational helper scripts |

## Main Runtime Boundaries

### `crates/config`

Use this crate when a feature changes:

- config schema
- defaults
- validation rules
- runtime projection of config into operational structures

### `crates/edge`

Use this crate when a feature changes:

- QUIC packet handling
- HTTP/3 request lifecycle
- route lookup
- admission and overload behavior
- body buffering and response streaming
- metrics export
- watchdog or drain behavior

This is the most sensitive crate in the project and currently carries the most implementation complexity.

### `crates/bridge`

Use this crate when a feature changes:

- request translation from downstream semantics to upstream H2
- `Host` rewrite behavior
- forwarded-header behavior
- trace/request-id propagation

### `crates/transport`

Use this crate when a feature changes:

- upstream TLS trust behavior
- HTTP/2 client pool behavior
- DNS refresh and backend address rotation
- connect and pool timeout behavior

### `crates/lb`

Use this crate when a feature changes:

- backend-health transition logic
- load-balancing algorithms
- weighted selection
- latency-aware behavior
- consistent-hash ring logic

## Sensitive Areas

Contributors should treat the following areas as high-risk:

- `crates/edge/src/quic_listener/mod.rs`
- request validation and pseudo-header handling
- connection/CID lifecycle logic
- drain and teardown behavior
- upstream TLS trust and verification behavior
- route determinism rules

## Where To Add New Work

| Feature Type | Primary Home |
| --- | --- |
| New config field | `crates/config` |
| New runtime limit/timeout | `crates/config` and `crates/edge` |
| New routing matcher | `crates/config` and `crates/edge/route_index.rs` |
| New LB algorithm | `crates/lb` |
| New upstream transport behavior | `crates/transport` |
| New request-header forwarding policy | `crates/bridge` |
| New metrics family | `crates/edge/src/metrics/` |
| New operator docs | `docs/operations/` or `docs/reference/` |
