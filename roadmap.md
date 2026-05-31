# Roadmap

Spooky's roadmap is organized into two tracks. **Part A** covers the hardening work required to reach GA across correctness, protocol coverage, routing, resilience, security, observability, configuration, and packaging. **Part B** covers the areas where Spooky is building a stronger position: performance, operator simplicity, reliability intelligence, and change safety.

Items ship in waves: correctness and production trust first, then migration tooling, then measurable capability leadership.

---

## What Is Shipped (v0.1.1-beta)

- HTTP/3 termination via quiche (RFC 9114) and QUIC transport (RFC 9000)
- HTTP/2 backend connectivity with connection pooling
- Path prefix and host-based routing with longest-match selection
- Six load balancing algorithms: random, round-robin, consistent hash, least-connections, latency-aware, sticky-cid
- Active health checking with configurable thresholds and cooldown
- Circuit breaker, retry budgets, request hedging, brownout mode, adaptive admission
- Per-upstream and per-backend inflight caps and queue limits
- TLS 1.3 with certificate chain loading; downstream mTLS via `listen.tls.client_auth.*`
- Upstream TLS peer verification on by default; cleartext requires explicit opt-in
- Prometheus metrics endpoint, control API (`/health`, `/ready`, `/admin/runtime`)
- Structured JSON logging with standard and Spooky-themed log levels
- Debian package with systemd unit; Docker packaging
- Configuration validation at startup with actionable error output

---

## Execution Priorities

| Priority | Scope |
|---|---|
| **P0** | Correctness, safety, security hardening, test matrix integrity |
| **P1** | Routing/LB/resilience depth, dynamic config safety, production operations, cloud-native packaging |
| **P2** | Differentiation tracks — starting with performance leadership and operator simplicity |

---

## Part A: Hardening Track

### A1 — Core Runtime Correctness and Safety

**Goal:** Eliminate correctness hazards in the concurrency model, error taxonomy, and protocol handling that could cause data loss, misrouting, or silent failure.

- Replace relaxed atomic ordering with Acquire/Release semantics for all inter-thread coordination flags. Validate with concurrency tests covering shutdown, queue accounting, and watchdog coordination.
- Separate error taxonomy into transport, config, and internal-runtime classes so that non-health errors do not incorrectly trigger backend health transitions.
- Unify hop-by-hop header filter pipeline across HTTP/1.1, HTTP/2, and HTTP/3 ingress paths. The same header strip set must be validated across all three paths.
- Add strict UTF-8 parse with explicit 4xx rejection on invalid pseudo-header bytes. Fuzz corpus must include malformed UTF-8 and pass rejection invariants.
- Establish canonical error-class mapping so metrics and logs show identical normalized failure categories.
- Remove dead branches and runtime checks that are unreachable given validator guarantees.

### A2 — Protocol Consistency (HTTP/1.1, HTTP/2, HTTP/3)

**Goal:** Guarantee that the same route and payload behaves identically across all three protocol paths.

- Protocol consistency contract covering status codes, headers, retries, and timeouts — validated by cross-protocol golden tests.
- Documented and integration-tested ALPN/Alt-Svc upgrade state machine for the bootstrap-to-H3 upgrade path.
- QUIC connection churn and handshake failure hardening: token bucket + handshake budgets with sustained-churn stability tests.
- Explicit max-stream, idle timeout, and stream reset policy controls for HTTP/2 and HTTP/3, with stream-stress tests covering defined latency and error budgets.

### A3 — Routing Engine

**Goal:** Expand the matching surface to cover the full set of conditions that reverse proxy operators expect.

- Route matching for exact path, prefix, regex, method, host, header values, and query parameters. Precedence and tie-break behavior must be deterministically specified and conformance-tested.
- RFC 3986 normalization policy modes: strict, passthrough, and safe-normalize. Encoded slashes, dot segments, and duplicate slashes must be deterministic across all modes.
- Route-level traffic split, mirror traffic, and staged rollout policy — with weighted distribution verified to stay within configured statistical bounds.
- Route graph versioning with atomic swap and rollback so live updates never produce invalid route windows.

### A4 — Load Balancing

**Goal:** Make selection deterministic, fair, and operationally transparent under concurrency.

- Deterministic fair selection primitives with lock-aware hot path for round-robin, least-connections, weighted, and sticky algorithms.
- Weighted round-robin and power-of-two-choices variants with target-percent drift guard, validated by 95% confidence interval checks.
- Cookie, header, CID, and hash-based session affinity with TTL and rebalance semantics. Stickiness and failover tests must prove deterministic rebinding.
- Latency-based and success-rate-based outlier ejection with configurable cooldown curves. Flapping backend tests must show controlled ejection and re-entry.
- Graceful drain state with admission cutoff and inflight completion budget. Zero-downtime backend drain scenario must pass.

### A5 — Resilience and Fault Tolerance

**Goal:** Make every resilience control safe, observable, and non-destructive under edge cases.

- Policy-driven retry classifier by method, status code, and error class. POST and unsafe requests must never be retried unless explicitly permitted.
- Circuit-breaker open/half-open/closed transition metrics and event logs per route and upstream, making transition timelines auditable.
- Sharded or atomic fast-path queue counters to keep admission overhead bounded at peak concurrency.
- Explicit degrade tiers: shed, partial features, static fallback — with measurable SLO preservation playbooks.
- Bulkhead boundaries per route and upstream pool so one pool failure does not cause global latency collapse.

### A6 — Security

**Goal:** Reach enterprise baseline security posture across TLS, header handling, and control plane access.

- Security profiles (`strict`, `balanced`, `compat`) with explicit cipher and TLS version policy. Profile tests must verify the protocol/cipher acceptance and rejection matrix.
- Complete mTLS negative-case coverage: cert chain, EKU, SAN, expiry, and revocation handling. Invalid, wrong-CA, and expired cert tests must reliably reject.
- Request smuggling defense: CL/TE ambiguity rejection and malformed framing protection validated against a smuggling attack corpus.
- Hard limits on max header size and count with clear rejection responses and no crash or latency blowup.
- Fine-grained authorization model for control endpoints with optional mTLS for the control channel. Unauthorized access must be fully denied and audited.
- Environment sanitization and secure command execution policy for restart hooks. Secret env variables must never be inherited.
- Insecure upstream TLS verification must require an explicit environment gate with startup warning. The production profile must block insecure upstream verification by default.

### A7 — Observability

**Goal:** Make Spooky's telemetry stable, machine-readable, and trustworthy under all operational conditions.

- Versioned metrics schema with deprecation windows and compatibility docs. Contract tests must enforce required metric names, labels, and types.
- Structured JSON access logs with required fields and request correlation IDs. Log schema validator must pass across all scenario families.
- End-to-end span propagation with consistent `traceparent` behavior. Trace continuity tests must cover retry, circuit, and failover paths.
- SLO-driven alert packs (latency, error, saturation, availability) with burn-rate rules. Alert simulation must show high precision and actionable context.
- Runtime event bus for reloads, drains, ejections, and policy changes. Every control-plane transition must have both event and metric evidence.

### A8 — Configuration and Control Plane

**Goal:** Make configuration lifecycle safe, introspectable, and recoverable.

- Safe dynamic config apply: validation, staging, atomic activation, and rollback. A failed config must never impact the active dataplane.
- Semantic config versioning and built-in migration assistant. Backward-compatible minor upgrades must generate a migration report.
- Full semantic preflight validator with actionable diagnostics. Validation error output must pinpoint field path and remediation.
- Effective config dump, runtime counters, and active policy snapshot endpoints so operators can reconstruct the runtime decision path from API and logs.

### A9 — Performance Engineering

**Goal:** Eliminate known allocation and contention hotspots in the hot path.

- Allocation reduction and lock sharding in ingress, LB, and admission paths. Target: measurable p99 latency and CPU profile improvement under baseline load.
- End-to-end backpressure contract from socket accept to upstream dispatch. Overload tests must show bounded queue growth and controlled shedding.
- True streaming for request and response bodies with bounded buffering and flow-control correctness. Large body tests must pass memory ceilings and latency budgets.
- Adaptive worker and shard auto-tuning hints from runtime telemetry. Auto-tuned profile must perform within 5% of a hand-tuned baseline.

### A10 — Packaging, Release, and Production Operations

**Goal:** Make every release verifiable, safe to upgrade, and operationally supportable.

- Signed binaries and container images, SBOM, and provenance attestations shipped on every release.
- Version compatibility matrix with automated upgrade canary tests. Two-hop upgrade and rollback must succeed.
- Incident runbooks for timeout spikes, backend collapse, cert failures, and reload faults — with game-day drills meeting MTTD/MTTR targets.
- Secure-by-default production config profile with startup hardening checks that pass an external security checklist.
- Reproducible benchmark harness with published raw datasets. Third-party reruns must produce comparable results.

### A11 — Kubernetes and Cloud-Native

**Goal:** Remove adoption friction for platform teams operating in cloud-native environments.

- Official Kubernetes deployment manifests with health probes validated in CI for HA cluster deployment.
- Production-grade Helm charts and Terraform modules with version pinning, verified across install, upgrade, and rollback workflows.
- Kubernetes ingress/Gateway integration repository with mapping docs. Existing ingress definitions must migrate with minimal rewrite.
- Official Prometheus rules, Grafana dashboards, and OpenTelemetry collector recipes enabling one-command observability bootstrap.

### A12 — Test Matrix

**Goal:** Make the test suite a reliable gate — not a confidence theater.

- Strict endpoint intent tags (data plane vs control plane vs metrics) so scenarios validate the right behavior class.
- Dedicated harness checks for smuggling, header abuse, TLS downgrade, and mTLS failure — with intentionally vulnerable fixtures confirming the suite fails correctly.
- Statistical trend checks for memory, FD, CPU variance, and error drift across 24h, 72h, and 7-day soak gates.
- Scenario isolation hooks and environment reset contracts so test order cannot affect results.
- Shared parser library across all scripts to eliminate duplicated and fragile extraction logic.

---

## Part B: Differentiation Track

### B1 — Performance Leadership (H3-First Edge)

- QUIC-native fast path with low-copy and low-allocation dispatch. Target: measurably better p99 tail latency and CPU/RPS on representative production hardware.
- Kernel-optimized UDP ingress profiles: socket buffers, pacing, and reuse strategies.
- Dual-mode latency governor: throughput mode and tail-latency mode with per-route latency caps and adaptive shedding before collapse.
- In-process micro-profiler endpoint for live hotspot snapshots with automatic profile diffing between versions.

### B2 — Simplicity and Operator Experience Leadership

- Opinionated production profiles: `edge-low-latency`, `api-gateway`, `zero-trust`.
- Guided config linter with severity levels and autofix hints.
- `spooky doctor` command for runtime diagnostics and misconfiguration detection.
- Human-readable decision traces: why a request was routed to a specific backend, why a circuit opened, why a retry fired.
- Built-in incident timeline reconstruction correlating events, metrics, and logs into a single command evidence bundle.
- Safe-mode startup that auto-disables risky options in production environments with guardrails refusing known-dangerous config combinations.

### B3 — Reliability Intelligence Leadership

- Predictive brownout based on trend forecasting of saturation and error growth — preventive controls that activate before outage thresholds are crossed.
- Adaptive retry budget and hedging controller driven by live success and latency signals.
- Per-route anomaly detection for sudden behavior drift.
- Self-healing backend scoring using a multidimensional latency/error/timeout model with automatic failback smoothing to prevent thundering herd after recovery.
- Runtime resilience simulation endpoint for what-if policy replay on sampled traffic with a blast-radius estimator for config changes before apply.

### B4 — Security Leadership

- Built-in policy engine for zero-trust edge controls: identity, cert posture, and route trust levels.
- Native certificate lifecycle checks with proactive expiry and failure forecasting.
- Security posture score emitted continuously as a metric so drift is visible and actionable before incident.
- Request integrity policy layer: header canonicalization signatures and anti-tamper rules.
- Advanced abuse detection heuristics for protocol anomalies with dynamic threat response controls.
- Confidential runtime mode baseline with strict memory and secret handling and a built-in compliance evidence generator.

### B5 — Observability and Explainability Leadership

- Unified telemetry schema across logs, metrics, and traces with shared correlation IDs.
- Route-level SLO objects natively in config with automatic burn-rate output.
- Single-pane runtime "why" answers: why a request failed, why a backend was chosen, why a retry fired.
- High-cardinality telemetry safeguards with auto-downgrade policies and per-tenant cost-aware telemetry budgets.

### B6 — Configuration and Change Safety Leadership

- Transactional config deploys with staged percentage rollout and auto-rollback triggers. Bad config rollout impact near-zero with automatic containment.
- Route-level canary for config changes — not only backend canary.
- Built-in differential analyzer between running and candidate config.
- Config policy testing against replayed traffic samples before apply with approval workflow hooks for controlled environments.

### B7 — Multi-Cluster and Fleet Leadership

- Lightweight fleet control plane for policy and config fanout with a cluster capability registry.
- Global traffic policy overlays with local override safety and consistency guarantees.
- Regional failover orchestration with health-weighted routing intents.
- Progressive global rollout with blast-radius guardrails.
- Fleet-wide SLO and risk dashboards with per-cluster drilldown.

### B8 — Benchmark and Transparency Leadership

- Public benchmark matrix covering hardware profiles, workload classes, and TLS modes.
- Version-over-version performance regression leaderboard with open replay traces for independent verification.
- Benchmark fairness charter: same backend, same tuning class, same topology.
- Automatic benchmark CI on release candidates with tail latency and jitter as first-class published metrics.

### B9 — Developer and Ecosystem Leadership

- Stable extension API (WASM/plugin model) with strict safety boundaries, first-party extension examples for auth, transformation, policy, and telemetry.
- Rich migration toolkit including config translators and diff reports for teams moving from other reverse proxies.
- Shadow mode migration assistant to compare decisions side-by-side in production with a cutover safety score.

### B10 — Cost and Efficiency Leadership

- Cost-per-million-requests telemetry and optimizer hints.
- Adaptive resource governor minimizing CPU while preserving SLO.
- Runtime profile advisor for instance sizing and worker/shard tuning.
- Automatic policy tuning recommendations from historical telemetry.

### B11 — Governance and Enterprise Readiness Leadership

- Full audit trail for control actions, config changes, and policy decisions.
- RBAC/ABAC policy model for control-plane access with break-glass and just-in-time elevated access flow.
- Lifecycle support policy with clear LTS branches.
- CVE response automation and patch impact advisories.
- Security and reliability scorecards per release.

### B12 — Release Validation Gates (No-Miss)

- Mandatory release gate set: correctness, security, performance, soak, upgrade, rollback. Hard fail if any contract test, benchmark threshold, or security baseline fails.
- Signed release manifest linking artifacts, SBOM, provenance, and benchmark report.
- Pre-release chaos campaign with pass thresholds.
- Synthetic customer workload replay as a release gate.
- Live canary auto-revert based on burn-rate triggers.

---

## Non-Goals

Features explicitly not planned for the core runtime:

- **Full service mesh** — Spooky is an edge reverse proxy, not a service mesh.
- **Content caching** — use a CDN or dedicated cache layer.
- **WAF capabilities** — use dedicated security tooling.
- **In-process plugin ABI** — no WASM, eBPF, or Lua middleware model until a safe isolation and lifecycle model is defined.
- **Database proxying** — use specialized database proxies.

---

## Contributing

Contributions are welcome. See [contributing guide](https://github.com/Supernova-Labs-Org/spooky/blob/master/CONTRIBUTING.md) for development setup and conventions.

High-value contribution areas in the current phase:

1. Concurrency correctness tests (Miri/Loom coverage for A1 items)
2. Cross-protocol golden tests (A2 protocol consistency)
3. Streaming request/response body handling (A9)
4. Configuration hot reload (A8)
5. OpenTelemetry distributed tracing integration (A7)
6. Kubernetes deployment manifests and Helm charts (A11)
