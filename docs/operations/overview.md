# Operations Overview

This section is the main entry point for deployment, rollout, validation, observability, and day-2 operation of Spooky.

Use this section when you need to answer:

- where to deploy Spooky
- how to prepare a host or runtime environment
- how to validate a rollout
- how to observe, diagnose, and operate the system
- where to go during an incident

## Start Here

- Ready to deploy: [Production Deployment](../deployment/production.md)
- [Production Readiness](production-readiness.md) defines what is ready today and what still blocks a broader production claim.
- [Production Deployment](../deployment/production.md) covers host preparation, service layout, and runtime posture.
- [Validation](../deployment/validation.md) covers pre-production and post-change validation patterns.
- [Migration](../deployment/migration.md) covers moving traffic from an existing proxy stack.
- [Distributed Quota](distributed-quota.md) covers policy examples, Redis backend rollout, degraded-mode behavior, and operator interpretation.
- [Observability Operator Bundle](observability-bundle.md) covers the shipped dashboards, alerts, SLO package, and cross-surface incident workflow.
- [Runbook](runbook.md) is the incident and maintenance quick-reference.
- [Sizing And Capacity](sizing-and-capacity.md) covers the main host and concurrency inputs that shape safe operation.
- [Host Tuning](host-tuning.md) groups host-level guidance.
- [Deployment Patterns](deployment-patterns.md) explains where Spooky fits best today.
- [Failure Modes](failure-modes.md) documents the major operator-visible failure classes.
- [Troubleshooting](../troubleshooting/common-issues.md) covers common failure signatures and operator checks.

## Common Paths

- Deploy a new environment: [Production Deployment](../deployment/production.md)
- Check if a rollout posture is safe: [Production Readiness](production-readiness.md)
- Validate before or after a change: [Validation](../deployment/validation.md)
- Handle incidents and maintenance: [Runbook](runbook.md)
- Investigate failures: [Failure Modes](failure-modes.md) and [Troubleshooting](../troubleshooting/common-issues.md)
- Understand dashboards and alerts: [Observability Operator Bundle](observability-bundle.md)

## What This Section Covers

- rollout posture
- deployment patterns
- validation workflow
- migration planning
- operational debugging
- incident response
- capacity and host planning
