# Release Maturity

## Current Stage: Beta

Spooky is currently **beta**. That means:

- the core H3 edge to H2 upstream flow is implemented and heavily exercised
- the project has meaningful operational and correctness work in place
- broader platform maturity, control-plane depth, and production-history confidence are still incomplete

Beta does **not** mean:

- full reverse-proxy feature parity with Envoy, NGINX, or Caddy
- full dynamic config management
- broad protocol parity across legacy and modern upstream shapes

## What Operators Can Assume Today

Operators can reasonably assume:

- the project is serious about correctness on its core path
- the docs now state key limitations explicitly
- controlled production rollout is the right stance

Operators should **not** assume:

- every interface is frozen
- every deployment shape has been equally hardened
- a restart-free config workflow exists

## Current Strong Areas

- downstream HTTP/3 ingress
- downstream bootstrap HTTP/1.1 and HTTP/2 support
- upstream HTTP/2 forwarding
- deterministic routing
- multiple load-balancing strategies
- strong teardown and resource-bound behavior
- active and passive health handling
- built-in observability and control-plane surfaces

## Current Hardening Gaps

- full config hot reload
- richer dynamic control plane
- broader upstream protocol support
- stronger service-discovery integrations
- auth, policy, and rate-limiting feature depth
- broader ecosystem and long-horizon production history

## GA Direction

The most important maturity gates before a broader GA-style claim are:

1. full config hot reload or an equally strong dynamic reconfiguration model
2. stronger parser and protocol hardening, including fuzzing
3. broader production validation across load, churn, and failure scenarios
4. clearer support boundaries and upgrade discipline
5. continued refactoring of the most concentrated runtime code

## Beta Deployment Guidance

- use canaries or bounded traffic first
- keep rollback warm and tested
- treat non-certificate config changes as drain-and-restart operations
- read release notes before upgrade
- pin to tagged versions, not moving branches

## Related Docs

- [Production Readiness](operations/production-readiness.md)
- [Feature Matrix](reference/feature-matrix.md)
- [Limitations](reference/limitations.md)
- [Roadmap](roadmap.md)
