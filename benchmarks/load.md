# Benchmarks

These benchmarks exercise the HTTP/3 data plane under realistic burst and degraded-backend conditions. All tests run over loopback (`127.0.0.1`) — they isolate proxy throughput and latency from network-level variables, not from them. See [How to Reproduce](#how-to-reproduce) for the exact commands used to generate these numbers.

## Test Environment

| | |
|---|---|
| **Machine** | Intel i5-11320H @ 3.20 GHz, 4 physical cores / 8 logical, 15 GiB RAM |
| **OS** | Linux 6.14, NVMe storage |
| **Target** | `127.0.0.1:9889` (loopback) |
| **Protocol** | HTTP/3 over QUIC → HTTP/2 upstream, TLS (self-signed) |
| **Backends** | 2 × upstream servers on `127.0.0.1:7001` and `127.0.0.1:7002` |
| **Config** | `config/config.development.yaml` with benchmark-specific performance overrides (`worker_threads=4`, `per_backend_inflight_limit=256`, `global_inflight_limit=4096`, adaptive admission enabled) |
| **Run ID** | `20260501T170035Z` |

## How to Reproduce

The benchmark suite is self-contained. To run equivalent benchmarks on your own hardware:

1. **Build the bench binary:**
   ```
   cargo run -p spooky-bench --release
   ```

2. **Run micro benchmarks:**
   ```
   ./scripts/bench-micro.sh
   ```
   Results are written to `bench/micro/latest.md`.

3. **Run macro benchmarks:**
   ```
   ./scripts/bench-macro.sh
   ```
   Results are written to `bench/macro/latest.md`.

4. **Run with regression gates** (this is what CI runs — fails on severe regressions):
   ```
   ./scripts/bench-gate.sh
   ```

5. **Loopback vs. production:** Loopback tests remove the network as a variable, which makes results reproducible and comparable across hardware generations. For production capacity planning, run tests from a separate client machine against your target host — loopback numbers will overstate throughput and understate p99 latency relative to a real deployment.

## Scenarios

### Burst — peak concurrency

3,000 requests, 120 concurrent connections.

| Metric | Value |
|---|---|
| Throughput | **21,235 req/s** |
| Success rate | 3000/3000 — **100%** |
| p50 latency | 19.1 ms |
| p95 latency | 87.8 ms |
| p99 latency | 102.4 ms |

### Burst — high concurrency

3,000 requests, 80 concurrent connections.

| Metric | Value |
|---|---|
| Throughput | **14,691 req/s** |
| Success rate | 3000/3000 — **100%** |
| p50 latency | 25.1 ms |
| p95 latency | 57.7 ms |
| p99 latency | 64.6 ms |

### Slow upstream — backend latency simulation

1,000 requests, 80 concurrent connections, upstream introduces delay.

| Metric | Value |
|---|---|
| Throughput | **9,549 req/s** |
| Success rate | 1000/1000 — **100%** |
| p50 latency | 26.9 ms |
| p95 latency | 58.1 ms |
| p99 latency | 62.0 ms |

### QUIC packet loss — lossy network simulation

1,500 requests, 120 concurrent connections, simulated packet loss.

| Metric | Value |
|---|---|
| Throughput | **12,500 req/s** |
| Success rate | 1500/1500 — **100%** |
| p50 latency | 44.2 ms |
| p95 latency | 79.7 ms |
| p99 latency | 91.2 ms |

## Interpreting These Numbers

These are single-host loopback results. Real-world numbers depend on your network topology, backend latency distribution, and request size — do not treat them as production throughput targets.

The burst-peak scenario (21K req/s) represents an upper bound for this hardware class. In practice, your backends will introduce additional latency, and your client machines will be separated from the proxy by at least one network hop, both of which will reduce observed throughput and increase latency.

The slow upstream scenario (9.5K req/s) is the most representative of a real API proxy workload where backends have non-trivial response times. Use this figure as your starting point for capacity estimates, then adjust downward based on your measured backend p50.

p99 latency is the number to watch in production. Under these loopback test conditions it stays under 103 ms across all scenarios. With real network round-trips and backend latency added, your p99 will be higher — the loopback baseline tells you what the proxy itself is contributing, which should help you attribute latency budget between proxy overhead and backend/network.

The 100% success rate across all scenarios reflects the overload admission controls rather than the absence of overload: requests that cannot be served within the inflight limits are shed immediately with `503 + Retry-After` rather than queued until they time out. In a genuine overload event, throughput will be lower and callers will receive fast 503s; the success rate metric as reported here will drop, but clients get a retryable signal rather than a silent timeout.

## Regression Gates

The project tracks benchmark regressions automatically using baseline files stored in `bench/baselines/`. The gate script (`./scripts/bench-gate.sh`) runs on every release candidate and compares current results against the stored baseline. A severe regression in CPU utilization, peak memory, or p99 latency causes the gate to exit non-zero and blocks the release. Minor fluctuations within expected variance are tolerated. Operators who need to promote a new baseline after a justified performance change — for example, after adding a feature that trades some throughput for correctness — can do so with:

```
./scripts/bench-promote-baseline.sh vX.Y.Z
```

This records the new numbers as the reference point for future gate runs.
