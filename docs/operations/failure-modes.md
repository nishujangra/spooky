# Failure Modes And Status Codes

This page documents the main operator-visible failure classes and how to interpret them.

## Read Failures in the Right Order

When a request fails, decide in this order:

1. Was it a request-shape or policy rejection?
2. Was it quota enforcement?
3. Was it overload self-protection?
4. Was it upstream transport failure or timeout?
5. Was it a genuine upstream application response?

Do not collapse these classes into one generic "proxy error."

## Common Status Codes

| Status | Typical meaning | Operator interpretation |
|---|---|---|
| `400` | malformed or unsupported request shape | client or ingress semantics problem |
| `403` | route or request policy denial | not normal quota exhaustion |
| `405` | method policy denial | route policy mismatch |
| `408` | request body stalled past idle timeout | slow or broken client body upload |
| `413` | request body exceeded configured cap | policy or client size issue |
| `429` | quota contract exhausted | contract enforcement, not overload |
| `502` | upstream transport or bridge failure before a valid backend response | backend connectivity, TLS, or protocol execution issue |
| `503` | overload shed, timeout insulation, temporary backend unavailability, or fail-closed quota backend failure | inspect reason and metrics before acting |

## 429: Quota Contract Failure

Typical reasons:

- `burst_quota_exhausted`
- `sustained_quota_exhausted`
- selector-derived quota denial

Interpretation:

- this is the normal distributed quota contract response
- treat it as quota enforcement, not overload
- inspect quota policy outcomes and quota backend health before tuning inflight limits

## 503: Do Not Assume One Cause

In Impulse, `503` can mean:

- overload shedding
- queue-cap or buffer-cap protection
- upstream timeout insulation
- temporary backend unavailability
- fail-closed quota backend failure

Operator rule:

- inspect the body text, logs, and metrics first
- check overload metrics and quota-backend health separately
- do not widen limits until you know whether the system is protecting itself correctly

## Genuine Upstream 5xx Responses

If the upstream returned a real 5xx response, that is usually a backend signal rather than a proxy-generated failure.

Check:

- backend error distribution
- per-upstream latency
- backend health transitions
- recent backend deploys or dependency failures

## Silent Drop Cases

Some traffic is dropped rather than turned into a rich HTTP response.

Examples include:

- malformed packets before a request lifecycle exists
- new connection attempts during drain
- packets for unknown connections in certain QUIC states

These are visible through observability and lifecycle signals rather than always through an HTTP status code.

## Stream Reset Versus HTTP Error

Impulse deliberately distinguishes between:

- returning an HTTP response such as `408`, `413`, `429`, or `503`
- resetting or terminating a stream when protocol or teardown semantics require it

This matters during client debugging and incident analysis. A missing HTTP status does not automatically mean the failure was invisible; it may have happened before a stable request-response boundary existed.
