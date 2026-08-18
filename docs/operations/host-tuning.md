# Host Tuning

This page groups host-level tuning guidance for production Spooky deployments. Use it together with [Production Deployment](../deployment/production.md) and [Sizing And Capacity](sizing-and-capacity.md).

## Primary Goals

Tune the host so the edge runtime gets:

- enough UDP and TCP buffer headroom
- enough file descriptors and process limits
- predictable CPU scheduling
- stable network behavior under packet bursts
- minimal interference from unrelated co-located workloads

## Baseline Tuning Areas

Validate these areas first:

- socket receive and send buffer ceilings
- device backlog and packet-processing budget
- file descriptor ceilings
- privileged-port bind strategy
- conntrack impact, if your environment inserts it in the path
- CPU pinning or IRQ placement only after measurement

## Recommended Starting Posture

- start from the Linux baseline in [Production Deployment](../deployment/production.md)
- use `scripts/sysctl-linux-network-tuning.sh` only as a baseline helper, not as a final answer
- keep the metrics and Control API surfaces reachable from operations tooling but isolated from public traffic
- isolate Spooky from unrelated batch or noisy-neighbor workloads where possible

## Host Validation Checklist

Before rollout, confirm:

1. UDP and TCP buffer values are high enough for your expected traffic shape.
2. `nofile` and related service limits exceed expected connection and socket usage.
3. The host can bind the required ports using either capability-based bind or privileged start plus drop.
4. MTU and network path behavior are stable between clients, the edge, and upstream networks.
5. Scraping, log shipping, and control-plane access do not contend heavily with the data plane.

## CPU Guidance

- Start with one worker per core as a baseline.
- Add packet sharding only if packet-rate pressure or worker imbalance justifies it.
- Add worker pinning only after measuring improvement on the target host.
- Avoid sharing the same cores with aggressive background jobs, log processors, or unrelated proxies.

## Network Guidance

- Validate QUIC traffic on the real ingress path, not only from local loopback tests.
- Measure packet drops, backlog pressure, and receive errors under burst traffic.
- If conntrack is present, confirm it is not becoming a hidden bottleneck for UDP traffic.
- Treat MTU changes carefully. Validate with the same client and network shapes you expect in production.

## Security And Permissions

- Keep config and certificate paths readable only to the minimum required service identities.
- Keep the Control API on loopback or an isolated admin network.
- Prefer explicit service-account ownership and minimal writable paths.

## Tuning Rules

- Do not copy aggressive sysctl values between environments blindly.
- Change one tuning area at a time and record the latency, drop-rate, and memory effect.
- Re-test after binary upgrades, kernel upgrades, or major traffic-shape changes.
- If a host change improves benchmark results but makes drain, rollout, or observability behavior worse, treat it as incomplete.
