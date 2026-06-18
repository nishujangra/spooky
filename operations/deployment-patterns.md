# Deployment Patterns

This page describes the deployment shapes that fit Spooky best today.

## Best-Fit Patterns

### HTTP/3 Edge To HTTP/2 Service Tier

Best current fit.

Use this when:

- clients need HTTP/3 at the edge
- services can accept HTTP/2 upstream traffic
- the environment benefits from explicit resource controls and strong teardown behavior

### Controlled Canary Rollout

Recommended current rollout model.

Use this when:

- you need to validate new versions or config changes gradually
- you can keep a rollback path warm
- you can bound blast radius during beta operations

### Single-Team Edge Tier

Good fit today when one team owns:

- proxy config
- backend topology
- TLS and cert rotation
- runtime tuning

## Weaker-Fit Patterns

### Dynamic Fleet-Managed Multi-Tenant Platform

Weaker fit today because:

- there is no rich dynamic config control plane
- there is no plugin system
- there is no broad policy engine

### General API Gateway Replacement

Weaker fit today because:

- JWT and auth features are missing
- broad rate limiting is missing
- rich request transformation is missing

### Broad Legacy Upstream Compatibility Proxy

Weaker fit today because:

- upstream forwarding is centered on HTTP/2
- protocol breadth is not yet the main strength of the product

## Recommended Rollout Shape

1. Start with one service or bounded traffic class.
2. Keep previous infrastructure available for rollback.
3. Use drain-and-restart for non-certificate config changes.
4. Expand only after stable latency, error rate, and health behavior.
