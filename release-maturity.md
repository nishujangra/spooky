# Release Maturity

## Current Stage: Beta (v0.1.x)

Spooky is currently in **Beta**. For an infrastructure component that sits on the critical path of inbound traffic, beta carries a precise meaning: the core protocol and proxying behavior are implemented, exercised by an automated test suite, and capable of handling real production traffic under controlled conditions. What beta does not yet provide is the extended operational history, soak validation, ecosystem breadth, and artifact provenance guarantees that justify treating an upgrade as routine. Operators should plan accordingly — Spooky can serve production traffic today, but doing so requires staged rollout discipline, maintained rollback capability, and closer-than-usual attention to release notes. The project team does not treat beta as a marketing label; it reflects a specific set of hardening work that remains in progress before GA can be declared with integrity.

---

## What Is Stable in v0.1.x

The following behaviors and interfaces are stable for the v0.1.x series. Stability here means: these are tested, the team treats regressions as release-blocking bugs, and no breaking changes will be introduced within the 0.1.x line without a documented deprecation notice.

**HTTP/3 ingress and HTTP/2 backend forwarding**
QUIC connection termination and HTTP/3 request handling are the core of what Spooky does. The ingress-to-backend forwarding path — accepting HTTP/3 from clients, translating and forwarding to HTTP/2 upstreams — is stable and regression-tested on every release.

**Configuration schema**
The configuration file schema is versioned. Within the 0.1.x series, all changes to the schema are additive. Fields will not be renamed or removed, and default values for new fields will not alter existing behavior. Operators can upgrade patch and minor versions within 0.1.x without auditing configuration for breaking changes.

**Load balancing algorithms**
All six load balancing algorithms — round-robin, least-connections, weighted round-robin, weighted least-connections, random, and IP hash — are stable. Selection logic, weight semantics, and tie-breaking behavior will not change within 0.1.x.

**Health check behavior**
Active health check configuration (interval, timeout, threshold counts for healthy/unhealthy transitions) and passive health check behavior (connection error detection, upstream ejection) are stable. The lifecycle of an upstream — how it transitions between healthy, unhealthy, and draining states — is deterministic and test-gated.

**Metrics endpoint format**
The Prometheus metrics endpoint format is stable. Metric names, label sets, and the meaning of each metric will not change within 0.1.x. New metrics may be added in minor releases; no existing metrics will be renamed or removed.

**Control API endpoints**
The following control plane endpoints are stable in path, method, request schema, and response schema:

- `GET /health` — liveness check
- `GET /ready` — readiness check (reflects upstream health state)
- `GET /admin/runtime` — current runtime configuration and state snapshot

These endpoints will not return different fields or change HTTP status semantics within 0.1.x.

---

## What Is Still Hardening

The following areas are under active development or have not yet completed the validation work required for a stability commitment. Operators should treat these as subject to change.

**Extended soak validation**
The test suite covers correctness and regression. What it does not yet cover is sustained operation at production-like concurrency over multi-day windows. Extended soak runs (72h or longer) at representative load profiles are required before GA and are not yet complete. Edge-case failure modes that only surface under sustained load may still be present.

**Dynamic configuration reload**
Configuration changes currently require a process restart. Hot reload — applying configuration changes without dropping connections — is planned and in progress. Until it ships, configuration management in production requires a restart strategy (graceful drain, rolling restart, or equivalent). The exact interface for dynamic reload is not yet finalized and will not be stable until it ships.

**Kubernetes and service mesh integration**
Spooky runs in Kubernetes today, but first-class integration — including a Helm chart in a stable release channel, a Kubernetes operator, xDS/control-plane compatibility, and validated service mesh interoperability — is still being built out. Operators running Spooky in Kubernetes should expect to manage integration details manually during beta.

**Failure-mode coverage and runbook hardening**
Known failure modes are documented. Coverage of less-common failure paths — upstream TLS renegotiation, partial QUIC handshake failures under load, memory pressure behavior, graceful shutdown edge cases — is still being expanded. Runbooks for operational response will be extended as failure modes are validated.

**Performance at very high scale**
Spooky performs well under typical production loads. Behavior above 10,000 concurrent connections per node has not been characterized at the level of rigor required for a GA commitment. Allocation patterns and scheduler interaction under sustained high concurrency are still being profiled and optimized.

---

## GA Exit Criteria

GA will be declared when all of the following gates are met. These are not aspirational targets — GA will not be declared until each criterion is verifiably satisfied.

1. **Protocol, routing, load balancing, resilience, and security behavior are deterministic and test-gated.** Every specified behavior has a corresponding automated test that is required to pass on every release. No behavioral specification exists only in documentation without test coverage.

2. **Extended soak runs passing at production-like concurrency.** A minimum of three independent 72-hour soak runs, at concurrency representative of a meaningful production deployment, complete without critical failures, unacceptable memory growth, or correctness regressions.

3. **Signed release artifacts with SBOM and provenance.** Every release artifact is signed. A Software Bill of Materials is published for each release. Build provenance meets SLSA Level 2 or higher.

4. **Reproducible benchmark methodology published.** A documented, reproducible benchmark suite is published alongside GA. Any operator can reproduce headline performance numbers using the published methodology. Numbers are not cherry-picked configurations.

5. **Zero critical regressions across three consecutive releases.** The three releases immediately preceding GA contain no regressions classified as critical (correctness bugs, data loss, security issues, or availability impact in a standard deployment). Minor releases within a patch series count.

6. **Config, metrics, and API contracts versioned with documented deprecation policy.** A formal deprecation policy is published and in force. Any future breaking change to a stable interface requires a documented deprecation period of at least one minor release cycle.

7. **Upgrade and rollback procedures validated.** Upgrade paths from each supported prior minor version to the GA release are tested. Rollback from GA to the prior minor version is tested and documented. Operators have a validated procedure, not just advice.

---

## Operator Guidance for Beta Deployments

**Use canary or bounded-traffic deployments to start.**
Do not route your full production traffic through Spooky on the first deployment. Start with a traffic segment you can afford to lose — a canary pool, a non-critical region, or a bounded percentage of traffic behind a feature flag or weighted DNS. Validate behavior against your workload before expanding.

**Keep rollback paths tested before increasing traffic share.**
A rollback path that exists on paper but has not been exercised is not a rollback path. Before increasing Spooky's traffic share at any stage, verify that you can revert within your acceptable time window. This means your prior proxy is still running, your traffic-shifting mechanism works in both directions, and your team has executed the rollback procedure at least once in a non-production environment.

**Subscribe to release notes — breaking changes will be documented in CHANGELOG.**
The project maintains a CHANGELOG that documents all breaking changes, behavioral changes, and deprecations with each release. Before upgrading in production, read the CHANGELOG entries for all versions between your current version and the target. During beta, configuration schema changes, metric renames, and API changes may occur between minor versions, always with prior notice where feasible.

**Pin to a specific version in production. Do not track `main`.**
The `main` branch reflects development state and is not suitable for production deployments. Pin your production deployment to a specific tagged release (e.g., `v0.1.1-beta`). Upgrade deliberately, after reading release notes and validating in staging. Automated version tracking against `main` or `latest` is not appropriate for any component on your request path.

---

## Version Policy

Spooky uses semantic versioning. The following definitions apply specifically to this project:

**Major version (1.0.0+)**
A major version increment signals breaking changes to the configuration schema, the control API (paths, request/response contracts, or HTTP semantics), or the metrics interface. Operators must review a migration guide before upgrading across a major version boundary. Major version 1.0.0 will coincide with the GA declaration.

**Minor version (0.X.0)**
A minor version increment adds new features, new configuration options, new endpoints, or new metrics. Changes within a minor release are non-breaking with respect to existing configuration and API usage. New fields may be added; existing fields will not be changed or removed. Operators should read the CHANGELOG before upgrading but can expect existing configuration to continue working without modification.

**Patch version (0.0.X)**
A patch version increment contains bug fixes and security patches only. No new features, no new configuration fields, no behavioral changes beyond fixing the identified defect. Patch upgrades are intended to be low-risk and should be applied promptly, particularly for security patches.

---

## Related Docs

- [Roadmap](roadmap.md)
- [Production Deployment](deployment/production.md)
- [Troubleshooting](troubleshooting/common-issues.md)
- [CHANGELOG](../CHANGELOG.md)
