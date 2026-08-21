# Sizing And Capacity

This page explains how to size Impulse nodes and how to think about safe concurrency. Exact numbers must be validated against your own workload.

## Inputs That Matter Most

Safe capacity depends primarily on:

- concurrent downstream connections
- concurrent in-flight requests
- backend latency distribution
- request and response body sizes
- percentage of long-lived or streaming traffic
- number of distinct routes, upstreams, and backends
- enabled logging, metrics, tracing, auth, quota, retry, and hedge policy

## Start With a Traffic Model

Before tuning limits, write down:

1. expected steady-state requests per second
2. expected peak requests per second
3. expected concurrent connections
4. longest common request and response durations
5. largest realistic request and response sizes
6. backend p95 and p99 latency under load

Without that model, capacity tuning becomes guesswork.

## CPU Guidance

Use more CPU when you expect:

- high QUIC handshake churn
- heavy TLS activity
- many active concurrent streams
- many routes with latency-sensitive traffic
- aggressive retry, hedge, auth, or quota evaluation volume

Reasonable starting points:

- small controlled rollout: 2 to 4 cores
- serious production node: 4 to 8 cores
- higher-throughput nodes: scale upward only after workload validation

## Memory Guidance

Memory use is driven mainly by:

- active connection count
- inflight request count
- buffered request bodies
- buffered or prebuffered response bodies
- long-lived streams during drain or overload
- body-size caps and queue caps

Do not size memory from idle behavior or smoke tests. Validate under:

- peak connection churn
- slow backends
- large bodies
- streaming traffic
- overload and brownout conditions

## Concurrency Guidance

The important question is not only "how many requests can the node accept" but also "how many should it admit before self-protection is healthier than continuing."

Treat these as protection boundaries, not throughput goals:

- `global_inflight_limit`
- `per_upstream_inflight_limit`
- `per_backend_inflight_limit`
- `max_active_connections`
- `request_buffer_global_cap_bytes`
- body-size limits

If you increase them, confirm:

- node memory still has headroom
- backend latency does not collapse
- overload recovery is still fast
- tail latency does not become unacceptable

## Worker And Shard Guidance

- start with one worker per core
- add packet sharding only when packet-rate pressure or worker imbalance justifies it
- use `SO_REUSEPORT` style multi-worker ingress as the baseline deployment model
- enable worker pinning only after testing on the production host class

## When 503s Appear

If you see 503s, do not immediately widen limits.

First decide whether the cause is:

- overload shedding
- backend timeout or backend failure
- quota backend failure under fail-closed policy
- mis-sized concurrency limits
- routing concentration on too few backends

The answer determines whether you should add capacity, fix backends, adjust policy, or change limits.

## Recommended Capacity Process

1. Establish a known-good baseline config.
2. Benchmark and soak-test from that baseline.
3. Increase one high-impact limit at a time.
4. Record latency, memory, shed rate, and backend health effects.
5. Re-run validation after enabling more advanced features such as quota, retries, hedging, or heavier auth paths.
