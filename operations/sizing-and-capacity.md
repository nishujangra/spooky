# Sizing And Capacity

This page documents how to think about Spooky sizing. Exact safe numbers must still be validated against your own traffic shape.

## Capacity Inputs That Matter

- concurrent downstream connections
- concurrent in-flight requests
- request and response body sizes
- percentage of long-lived streams
- backend latency distribution
- number of distinct upstreams and backends
- enabled observability and logging volume

## CPU Guidance

Use more CPU when you expect:

- high QUIC handshake churn
- many concurrent active streams
- heavy TLS activity
- many latency-sensitive routing decisions
- lots of slow-stream and timeout management

Start with:

- small rollout: 2 to 4 cores
- serious production edge node: 4 to 8 cores
- high-throughput nodes: validate upward from there with real load

## Memory Guidance

Memory use is strongly shaped by:

- active connection count
- inflight request count
- request buffering under slow upstream conditions
- unknown-length response prebuffering
- configured body-size caps

Do not size memory only from idle or smoke-test behavior. Validate under:

- high connection churn
- large request bodies
- streaming responses
- overload conditions
- brownout or queueing conditions

## Worker And Shard Guidance

- start with one worker per core as a baseline
- increase packet sharding only when you have evidence the extra dispatch layer helps
- use reuseport for multi-worker deployments
- enable worker pinning only after measuring benefit on the target host

## Limit Tuning Guidance

Treat these as capacity controls, not just feature toggles:

- `global_inflight_limit`
- `per_upstream_inflight_limit`
- `per_backend_inflight_limit`
- `max_active_connections`
- `max_request_body_bytes`
- `max_response_body_bytes`
- `request_buffer_global_cap_bytes`

Increase them only after validating:

- memory headroom
- tail latency
- overload recovery behavior
- backend tolerance for the higher concurrency

## Operational Rule

If you need to raise limits to stop 503s, first determine whether:

- the limits are genuinely too low
- the backend fleet is unhealthy
- the routing policy is concentrating load incorrectly
- the system is correctly shedding to protect itself

## Recommended Practice

- establish a known-good baseline config
- benchmark and soak-test from that baseline
- change one high-impact limit at a time
- record resulting latency, memory, and shed behavior
