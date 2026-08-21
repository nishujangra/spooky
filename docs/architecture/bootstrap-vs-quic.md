# Bootstrap vs QUIC

This document explains the two ingress paths in `impulse-edge` and the intended boundary between them.

## Short Version

- QUIC is the main data-plane ingress path.
- Bootstrap is a compatibility ingress path for HTTP/1.1 and HTTP/2.
- Both paths should share the same policy, routing, transport, and observability layers.
- They should differ mainly in ingress and egress mechanics.

## Why Both Paths Exist

Impulse is built around QUIC and HTTP/3 at the edge, but operators still need a compatibility path for environments that cannot enter over QUIC immediately.

The bootstrap path exists so Impulse can:

- accept HTTP/1.1 or HTTP/2 traffic where needed
- support compatibility migration scenarios
- preserve shared policy behavior while using different wire protocols at ingress

Bootstrap is not meant to be a second independent runtime architecture.

## Boundary At a Glance

| Concern | QUIC path | Bootstrap path | Should semantic policy differ? |
|---|---|---|---|
| Downstream protocol | HTTP/3 over QUIC | HTTP/1.1 or HTTP/2 | No |
| Intake mechanics | UDP, QUIC, HTTP/3 streams | HTTP server request intake | No |
| Upgrade handling | Native stream model | WebSocket and HTTP upgrade handling | Only where protocol requires it |
| Response writeback | HTTP/3 stream emission | HTTP/1.1 or HTTP/2 response emission | No |
| Routing, auth, quota, overload, transport, observability | Shared | Shared | No |

## QUIC Path

The QUIC path is the primary ingress model.

It owns:

- UDP socket ingress
- QUIC handshake and connection lifecycle
- HTTP/3 stream handling
- stream progression and chunk emission
- QUIC-specific response writeback

Most of this logic lives under `crates/edge/src/quic_listener/` and its forwarding/runtime modules.

## Bootstrap Path

The bootstrap path is the compatibility ingress model.

It owns:

- bootstrap listener startup
- HTTP request intake and validation
- websocket and upgrade follow-through
- bootstrap-specific upstream dispatch glue
- bootstrap response writeback

Most of this logic now lives under `crates/edge/src/quic_listener/bootstrap/`.

The bootstrap façade should stay thin and focused on compatibility mechanics.

## Shared Layers Under Both Paths

The two ingress paths should converge on the same internal policy layers as early as possible.

That shared stack includes:

- admission and pre-forward policy evaluation
- route resolution and backend selection
- load-balancing key resolution
- external auth decision logic
- canonical request building in `bridge`
- transport execution in `transport`
- canonical response normalization in `bridge`
- streaming/body guardrail policy
- retry and hedge policy
- request/backend outcome recording
- runtime generation and backend lifecycle state

If a new policy exists only in QUIC or only in bootstrap, that is usually a design smell unless it is truly protocol-specific.

## Where They Should Differ

The paths are expected to differ in a few places.

### Ingress mechanics

QUIC owns packet, connection, and HTTP/3 stream handling.

Bootstrap owns HTTP accept, request parsing, and upgrade mechanics.

### Egress mechanics

QUIC writes normalized responses back through HTTP/3 stream APIs.

Bootstrap writes normalized responses back through HTTP/1.1 or HTTP/2 response handling and may need websocket upgrade follow-through.

### Protocol-specific validation

Some request and response validation is protocol-specific at the edge of the ingress path. That logic should stay local to the path that owns the protocol.

## Where They Should Not Differ

The following should remain shared and semantically identical:

- auth allow/deny/challenge behavior
- quota, rate-limit, overload, and brownout behavior
- route matching and upstream lookup
- backend selection semantics
- retry and hedge eligibility
- backend health feedback
- outcome reason vocabularies
- metrics and logging dimensions
- runtime-generation and Control API views

If bootstrap and QUIC start producing different policy decisions for the same logical request, the shared layer is in the wrong place or one path has leaked local policy logic.

## Code Ownership Guide

Add code to bootstrap when the concern is compatibility-path specific, such as:

- request intake differences
- HTTP upgrade mechanics
- bootstrap response writeback details

Add code to shared layers when the concern is common policy or state, such as:

- admission
- auth decision mapping
- routing and backend selection
- request building
- response normalization
- guardrails
- outcome recording

Add code to transport when the concern is backend protocol execution, not ingress behavior.

## Design Rule

QUIC is the main data-plane path.

Bootstrap should be treated as a compatibility wrapper around the same internal policy and execution model, not as an alternate architecture with its own independent decisions.
