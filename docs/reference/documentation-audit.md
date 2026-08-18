# Documentation Audit

This page audits the current `docs/` tree and maps every documentation file into the product-reference buckets that should shape the final documentation set.

The target buckets are:

- overview
- getting started
- configuration
- architecture
- operations
- API and control plane
- observability
- troubleshooting
- strict reference

Some files currently live in legacy sections such as `development/`, `howto/`, `tutorials/`, and `concepts/`. They are included here with their recommended target bucket so the documentation tree can be tightened in later cleanup steps.

## Bucket Summary

| Target bucket | Current files |
| --- | --- |
| overview | `docs/README.md`, `docs/concepts/overview.md` |
| getting started | `docs/getting-started/overview.md`, `docs/getting-started/installation.md`, `docs/getting-started/docker.md`, `docs/getting-started/minimum-production.md`, `docs/tutorials/quickstart.md`, `docs/howto/README.md`, `docs/howto/01-certificates.md`, `docs/howto/02-configuration.md`, `docs/howto/03-run.md` |
| configuration | `docs/configuration/reference.md`, `docs/configuration/defaults.md`, `docs/configuration/examples.md`, `docs/configuration/tls.md` |
| architecture | `docs/architecture/overview.md`, `docs/architecture/components.md`, `docs/architecture/edge-runtime.md`, `docs/architecture/request-lifecycle.md`, `docs/architecture/transport.md`, `docs/architecture/backend-lifecycle.md`, `docs/architecture/runtime-generation.md`, `docs/architecture/bootstrap-vs-quic.md`, `docs/development/codebase-map.md`, `docs/development/invariants.md` |
| operations | `docs/operations/overview.md`, `docs/deployment/production.md`, `docs/deployment/validation.md`, `docs/deployment/migration.md`, `docs/operations/deployment-patterns.md`, `docs/operations/production-readiness.md`, `docs/operations/reload-and-drain.md`, `docs/operations/failure-modes.md`, `docs/operations/host-tuning.md`, `docs/operations/sizing-and-capacity.md`, `docs/operations/runbook.md` |
| API and control plane | `docs/api/overview.md`, `docs/operations/control-plane.md`, `docs/reference/control-api-reference.md` |
| observability | `docs/architecture/observability-contract.md`, `docs/operations/metrics-and-alerts.md`, `docs/operations/observability-bundle.md`, `docs/reference/metrics-reference.md` |
| troubleshooting | `docs/troubleshooting/common-issues.md` |
| strict reference | `docs/reference/overview.md`, `docs/reference/feature-matrix.md`, `docs/reference/limitations.md`, `docs/reference/terminology.md`, `docs/public-api-surface-inventory.md`, `docs/architecture/quota-policy-contract.md`, `docs/concepts/security-model.md`, `docs/protocols/http3.md`, `docs/protocols/quic.md`, `docs/changelog.md`, `docs/release-maturity.md`, `docs/roadmap.md`, `docs/references.md`, `docs/development/overview.md`, `docs/development/adding-features.md`, `docs/development/testing-strategy.md`, `docs/development/benchmarking.md`, `docs/operations/distributed-quota.md`, `docs/user-guide/basics.md`, `docs/user-guide/load-balancing.md` |

## File Inventory

| Current file | Target bucket | Notes |
| --- | --- | --- |
| `docs/README.md` | overview | Primary docs index and reader entry point. |
| `docs/api/overview.md` | API and control plane | High-level map of metrics and control surfaces. |
| `docs/architecture/backend-lifecycle.md` | architecture | Runtime backend state and lifecycle ownership. |
| `docs/architecture/bootstrap-vs-quic.md` | architecture | Explains ingress-path differences and shared semantics. |
| `docs/architecture/components.md` | architecture | Per-crate and subsystem responsibility map. |
| `docs/architecture/edge-runtime.md` | architecture | Listener/runtime ownership boundaries. |
| `docs/architecture/observability-contract.md` | observability | Canonical operator-signal vocabulary and field rules. |
| `docs/architecture/overview.md` | architecture | High-level runtime design and topology. |
| `docs/architecture/quota-policy-contract.md` | strict reference | Semantics-heavy policy contract; better treated as exact reference than introductory architecture. |
| `docs/architecture/request-lifecycle.md` | architecture | Canonical ordered request flow. |
| `docs/architecture/runtime-generation.md` | architecture | Runtime ownership and generation model. |
| `docs/architecture/transport.md` | architecture | Transport responsibilities and execution boundaries. |
| `docs/changelog.md` | strict reference | Release history, not a reader journey page. |
| `docs/concepts/overview.md` | overview | Product concept entry that should later be merged or tightened with top-level overview docs. |
| `docs/concepts/security-model.md` | strict reference | Trust boundaries and current safety assumptions. |
| `docs/configuration/defaults.md` | configuration | Default values and baseline behavior. |
| `docs/configuration/examples.md` | configuration | Configuration examples and usage patterns. |
| `docs/configuration/reference.md` | configuration | Authoritative schema and semantics reference. |
| `docs/configuration/tls.md` | configuration | Certificate, trust, and TLS configuration reference. |
| `docs/deployment/migration.md` | operations | Migration guidance for moving traffic from another edge layer. |
| `docs/deployment/production.md` | operations | Production deployment and runtime posture. |
| `docs/deployment/validation.md` | operations | Pre-deploy and post-change validation workflow. |
| `docs/development/adding-features.md` | strict reference | Contributor-focused process doc; currently outside product-reference ideal and likely to be de-emphasized later. |
| `docs/development/benchmarking.md` | strict reference | Maintainer tooling reference rather than product docs. |
| `docs/development/codebase-map.md` | architecture | Useful implementation map for understanding subsystem placement. |
| `docs/development/invariants.md` | architecture | Behavioral invariants that explain why runtime pieces are structured the way they are. |
| `docs/development/overview.md` | strict reference | Contributor index; currently outside the target product-reference core. |
| `docs/development/testing-strategy.md` | strict reference | Maintainer testing reference, not operator-facing product docs. |
| `docs/getting-started/docker.md` | getting started | Container-based first-run path. |
| `docs/getting-started/installation.md` | getting started | Install and prepare the product. |
| `docs/getting-started/minimum-production.md` | getting started | Fastest path from evaluation to first safe production posture. |
| `docs/getting-started/overview.md` | getting started | Reader entry point for setup and first run. |
| `docs/howto/01-certificates.md` | getting started | Task-focused onboarding content that overlaps with getting-started/configuration. |
| `docs/howto/02-configuration.md` | getting started | Task-focused first configuration path. |
| `docs/howto/03-run.md` | getting started | Task-focused first run and smoke test path. |
| `docs/howto/README.md` | getting started | Legacy how-to index that should later be folded into getting-started navigation. |
| `docs/operations/control-plane.md` | API and control plane | Operational view of the admin surface. |
| `docs/operations/deployment-patterns.md` | operations | Where Spooky fits in real deployments. |
| `docs/operations/distributed-quota.md` | strict reference | Operational semantics and examples for a specific advanced feature; likely to remain a specialist reference page. |
| `docs/operations/failure-modes.md` | operations | Canonical degraded-behavior guide. |
| `docs/operations/host-tuning.md` | operations | Host-level runtime tuning. |
| `docs/operations/metrics-and-alerts.md` | observability | Operational signal interpretation and alert usage. |
| `docs/operations/observability-bundle.md` | observability | Packaged dashboards, alerts, SLOs, and operator workflow. |
| `docs/operations/overview.md` | operations | Section landing page for deploy and operate workflows. |
| `docs/operations/production-readiness.md` | operations | Current readiness and GA-blocker positioning. |
| `docs/operations/reload-and-drain.md` | operations | Runtime reload and drain behavior. |
| `docs/operations/runbook.md` | operations | Incident and maintenance procedures. |
| `docs/operations/sizing-and-capacity.md` | operations | Capacity planning guidance. |
| `docs/protocols/http3.md` | strict reference | Protocol-specific behavior reference. |
| `docs/protocols/quic.md` | strict reference | Transport behavior and terminology reference. |
| `docs/public-api-surface-inventory.md` | strict reference | Exact public/exported surface inventory. |
| `docs/reference/control-api-reference.md` | API and control plane | Endpoint-by-endpoint admin API reference. |
| `docs/reference/feature-matrix.md` | strict reference | Supported, partial, and missing capability inventory. |
| `docs/reference/limitations.md` | strict reference | Current product limits. |
| `docs/reference/metrics-reference.md` | observability | Exact exported metrics and labels. |
| `docs/reference/overview.md` | strict reference | Landing page for exact-behavior reference. |
| `docs/reference/terminology.md` | strict reference | Canonical vocabulary page. |
| `docs/references.md` | strict reference | External standards and supporting references; likely to remain secondary. |
| `docs/release-maturity.md` | strict reference | Maturity statement and GA criteria. |
| `docs/roadmap.md` | strict reference | Planned direction and remaining gaps. |
| `docs/troubleshooting/common-issues.md` | troubleshooting | Symptom-driven issue diagnosis. |
| `docs/tutorials/quickstart.md` | getting started | Guided first success flow; overlaps with quickstart/getting-started content. |
| `docs/user-guide/basics.md` | strict reference | End-user style product usage page that may later be merged with overview or getting-started. |
| `docs/user-guide/load-balancing.md` | strict reference | Detailed behavior guide for a specific feature area. |

## Documentation Support Files

These files are not product-reference pages, but they support the published documentation site and should stay outside the content buckets:

| File | Purpose |
| --- | --- |
| `docs/javascripts/code-copy.js` | Documentation site behavior. |
| `docs/stylesheets/extra.css` | Documentation site styling. |

## Legacy Sections To Reclassify Later

These current folders do not match the desired final product-reference shape and should be absorbed into the target buckets over time:

| Current section | Recommended destination |
| --- | --- |
| `docs/howto/` | getting started and configuration |
| `docs/tutorials/` | getting started |
| `docs/concepts/` | overview and strict reference |
| `docs/development/` | architecture for runtime-ownership material, strict reference for contributor-only material |
| `docs/user-guide/` | strict reference or overview depending on page depth |
| `docs/deployment/` | operations |

## Immediate Observations

- The current documentation set is broad and already covers most major product areas.
- Navigation is strong at the section level, but several legacy folders still split similar content across multiple paths.
- `howto`, `tutorials`, and `getting-started` overlap and will need consolidation later.
- `development` contains both architecture-useful material and contributor-only material; those should not be treated the same way in the final reader journey.
- Observability content is now rich enough to stand as its own bucket instead of being spread only across operations and reference pages.
