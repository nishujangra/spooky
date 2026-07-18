# Migrating to Spooky from NGINX or Envoy

This guide is for platform and SRE engineers who already operate NGINX or Envoy and want to put Spooky in front of their stack, or replace their existing proxy entirely. It assumes you know how reverse proxies work but are new to Spooky's config model.

---

## Before You Start

**What Spooky is and is not.** Spooky is a QUIC-native edge reverse proxy. It terminates HTTP/3 connections over QUIC, forwards to upstream backends over HTTP/2 (`https://` backends) or HTTP/1.1 (`http://` backends), and handles upstream pool management with named pools, path- and host-based routing, and per-upstream health checks. Mixed HTTP/1.1 and HTTP/2 backend deployments are supported in the same config. It is not a full API gateway, but it does include scoped rate limiting (by route/client/tenant/token), per-upstream API-key and local HS256-JWT auth (with scope/role checks), and external auth via HTTP subrequest or OIDC. What it lacks is a request/response transformation pipeline, JWKS/asymmetric-JWT validation, and a generic policy engine. It is not a service mesh control plane: it does not speak xDS, does not distribute config to sidecars, and does not manage mTLS between services. It is not a WAF: it has no request inspection, no ModSecurity integration, no bot detection. If your current NGINX or Envoy setup relies on any of those capabilities, read the "Things That Don't Translate Directly" section before proceeding — you will need to keep those concerns handled elsewhere in your stack.

**Two migration patterns.** This document covers two approaches. Pattern A is additive: Spooky sits in front of your existing proxy and acts as an HTTP/3 ingress layer, while NGINX or Envoy continues to handle all the routing and backend logic it currently handles. This is the lowest-risk starting point and requires no changes to your existing proxy config or your backends. Pattern B is a replacement: Spooky takes over routing duties route by route, and you eventually decommission your old proxy entirely. Pattern B is recommended for teams who want to move fully to Spooky, but it should be done incrementally — one route at a time — never as a big-bang cutover.

**Prerequisites before you start either pattern.** Every backend that Spooky will proxy to must expose a health check endpoint that returns a non-5xx status code when the instance is healthy; Spooky uses these for upstream health transitions and you need them to distinguish Spooky-induced errors from pre-existing backend problems. Have a tested rollback procedure ready before you touch DNS or your load balancer — the rollback steps are documented at the end of this guide, but you should run through them in staging first. Set up monitoring for upstream error rate (5xx responses from backends) and p99 latency for the routes you are migrating; you need a baseline from your existing proxy to compare against during and after the migration window.

---

## Pattern A: Spooky as HTTP/3 Ingress in Front of Your Existing Proxy

In this pattern, your existing NGINX or Envoy instance stays up and continues doing exactly what it does today. Spooky sits in front of it on the same host (or on a dedicated edge host) and accepts HTTP/3 connections from modern clients, then forwards all traffic to your existing proxy over HTTP/2 or HTTP/1.1. Clients that do not support QUIC connect to Spooky's TCP bootstrap listener and their traffic is forwarded the same way.

**Step 1: Leave your existing proxy untouched.**

Do not change any NGINX or Envoy config. Do not move its listen ports. If it listens on 443 (TCP), leave it there. You are not replacing it yet — you are adding Spooky in front.

**Step 2: Install and configure Spooky.**

Spooky needs two listen addresses: UDP 9889 for QUIC/HTTP/3 connections, and TCP 9889 as the bootstrap path for clients that need to negotiate up to HTTP/3 or fall back gracefully. Configure a single upstream pool pointing at your existing proxy's listen address.

The following is a complete working config for Pattern A, assuming your existing proxy listens on `127.0.0.1:443`:

```yaml
# /etc/spooky/config.yaml — Pattern A: Spooky as HTTP/3 ingress in front of NGINX/Envoy

# A single `listen` block defines the QUIC/HTTP-3 listener. Spooky automatically starts a
# TCP+TLS bootstrap listener on the SAME address/port for HTTP/1.1 and HTTP/2 clients and
# advertises `Alt-Svc: h3` so they upgrade to HTTP/3 — there is no separate TCP listener entry.
listen:
  protocol: http3           # the only valid value; QUIC (UDP) + auto TCP bootstrap
  address: "0.0.0.0"        # host only — NOT combined with the port
  port: 9889
  tls:
    cert: /etc/spooky/tls/fullchain.pem
    key: /etc/spooky/tls/privkey.pem

# `upstream` is a MAP keyed by pool name (not a `upstreams:` list, and no `name:` field).
# The route match lives inside the pool entry — there is no top-level `routes:` list.
upstream:
  existing-proxy:
    load_balancing:
      type: round-robin
    route:
      host: "*.example.com" # host and/or path_prefix; matched to this pool
      path_prefix: "/"
    backends:
      - id: existing-proxy-1
        address: "https://127.0.0.1:443"  # scheme in the address: https:// = HTTP/2 upstream
        health_check:
          path: /healthz
          interval: 10000     # milliseconds (integer), not "10s"
          timeout_ms: 3000
          failure_threshold: 3
          success_threshold: 2
```

> **TLS note.** Spooky must present a certificate that your clients trust. If your existing proxy terminates TLS with a cert from Let's Encrypt or your CA, use the same cert and key here — or provision a new one for the Spooky host. Spooky forwards to your existing proxy over HTTPS; if your existing proxy uses a self-signed cert on the loopback interface you may need to configure trust appropriately or use HTTP on the loopback leg.

**Step 3: Update DNS.**

Point your domain's A/AAAA records to the host running Spooky. For a canary rollout, use weighted DNS to send a small percentage of traffic to the Spooky host first. Make sure your TTL is low (60–300 seconds) before you make the change so you can roll back quickly.

**Step 4: Watch traffic flow.**

Modern clients that advertise QUIC support will connect over HTTP/3 to UDP 9889. Older clients will use the TCP listener. All traffic ends up at your existing proxy unchanged — Spooky is transparent to your backends and to your existing proxy's access logs.

**Step 5: No backend or application changes required.**

Your upstream services see exactly the same requests they always have. Your NGINX/Envoy config does not change. This is a zero-touch migration for everything behind the proxy.

---

## Pattern B: Replacing Your Existing Proxy Route by Route

Pattern B is the recommended path for teams who want Spooky to be the sole proxy. Do not attempt a big-bang cutover — the failure mode is a full-site outage with a multi-minute blast radius. Migrate one route at a time, validate it under real traffic, then move to the next.

**Step 1: Pick the first route to migrate.**

Choose something low-risk and high-observability: a static asset path (`/static`), a read-only API endpoint, or a health endpoint. Do not start with payments, authentication, or any route where a 5xx error has a direct user or business impact. The goal of the first migration is to validate your Spooky config and your monitoring pipeline, not to capture the highest-traffic route.

**Step 2: Write the Spooky config for that route only.**

Define the target backend as a named upstream pool. Define a fallback pool that points at your existing proxy. All traffic not explicitly matched by a Spooky route should fall through to the fallback. See the config sketch in this step below.

**Step 3: Route traffic to Spooky for only that path prefix.**

Options:
- If you have a cloud load balancer, add a rule that sends requests matching the path prefix to the Spooky target group and leaves all other rules pointing at your existing proxy.
- If you use weighted DNS per subdomain (e.g., `static.example.com`), cut the subdomain over to Spooky.
- If Spooky and your existing proxy are on the same host, have Spooky listen on a separate port initially and shift the LB rule to it for the target path only.

**Step 4: Validate before proceeding.**

Monitor for 24–48 hours under real traffic. Compare:
- 5xx error rate for the migrated route against your pre-migration baseline from the old proxy
- p99 latency for the migrated route
- Health check transition events in Spooky logs (a backend should not be flapping between healthy and unhealthy)

If error rate is elevated, latency has regressed, or health checks are unstable, roll back to the old proxy for that route and investigate before continuing.

**Step 5: Migrate the next route.**

Once the first route is clean for 24–48 hours, repeat steps 1–4 for the next route. Work through your route inventory in order of increasing criticality.

**Step 6: Decommission the old proxy.**

When all routes are on Spooky and you have at least one full week of clean metrics, stop and remove the old proxy. Keep its binary and config archived for at least one additional week in case you need to reconstruct a rollback.

### Pattern B Config Sketch

The key principle is that every request must have a destination. Model your existing proxy as a named upstream pool and use it as the catch-all for anything not yet migrated.

```yaml
# /etc/spooky/config.yaml — Pattern B: incremental route migration

# Single QUIC/HTTP-3 listener; the TCP bootstrap listener is started automatically on the
# same address/port (see Pattern A note above).
listen:
  protocol: http3
  address: "0.0.0.0"
  port: 9889
  tls:
    cert: /etc/spooky/tls/fullchain.pem
    key: /etc/spooky/tls/privkey.pem

# `upstream` is a map keyed by pool name. Routing is expressed by each pool's own `route:`
# block; there is no separate top-level `routes:` list. Longest-prefix wins, so a pool with
# `path_prefix: /static` takes precedence over the `path_prefix: /` catch-all pool.
upstream:
  # The first route migrated to Spooky — static asset backend
  static-origin:
    load_balancing:
      type: round-robin
    route:
      path_prefix: "/static"   # more specific → matched first
    backends:
      - id: static-origin-1
        address: "http://10.0.1.20:8080"   # http:// = HTTP/1.1 upstream
        health_check:
          path: /healthz
          interval: 10000
          timeout_ms: 3000
          failure_threshold: 3
          success_threshold: 2

  # Everything else still goes to your existing proxy (catch-all via path_prefix "/")
  legacy-proxy:
    load_balancing:
      type: round-robin
    route:
      path_prefix: "/"         # least specific → the fallback
    backends:
      - id: legacy-proxy-1
        address: "https://127.0.0.1:443"   # https:// = HTTP/2 upstream
        health_check:
          path: /healthz
          interval: 15000
          timeout_ms: 5000
          failure_threshold: 2
          success_threshold: 1
```

As you migrate each subsequent route, add a new upstream pool (with its own more-specific
`route.path_prefix`) for its backend. Longest-prefix matching routes those paths to the new pool
while everything else keeps falling through to the `legacy-proxy` pool. When the fallback pool has
no traffic, remove it and decommission the old proxy.

---

## NGINX to Spooky Config Translation

| NGINX directive | Spooky equivalent |
|---|---|
| `upstream mypool { server 10.0.0.1:8080; }` | A `mypool:` key under the `upstream:` map with a `backends:` list containing `id:` + `address: "http://10.0.0.1:8080"` |
| `proxy_pass http://mypool` | The `mypool:` pool's own `route:` block (`host`/`path_prefix`); there is no separate name reference — the match lives on the pool |
| `location /api { ... }` | `route: { path_prefix: "/api" }` inside the relevant upstream pool |
| `proxy_next_upstream error timeout http_502` | Per-backend `health_check:` config with `failure_threshold` controlling how many consecutive failures remove a backend from rotation; retries on connection error are automatic |
| `least_conn` | `load_balancing: { type: least-connections }` inside the upstream pool |
| `ip_hash` | `load_balancing: { type: consistent-hash }` (hashes on client address by default) |
| `keepalive 32` | Spooky maintains a connection pool to upstream backends automatically; the pool size is not separately configurable |
| `ssl_certificate /path/cert.pem` | `listen.tls.cert: /path/cert.pem` under the relevant listener |
| `ssl_certificate_key /path/key.pem` | `listen.tls.key: /path/key.pem` under the relevant listener |

---

## Things That Don't Translate Directly

The following NGINX and Envoy features have no equivalent in Spooky v0.1.x. This is not a complete long-term roadmap statement — it is a description of what is absent right now. Plan around these gaps before starting a migration.

**NGINX dynamic modules (ModSecurity, gzip, Brotli, etc.)** are not available. Spooky has no module system and no request/response body processing pipeline. If you rely on ModSecurity for WAF rules, you must keep a WAF in the chain — either in front of Spooky or as a sidecar on the backend. For gzip/Brotli: serve pre-compressed assets from your origin, or keep an NGINX instance in the chain for compression.

**Envoy's xDS dynamic control plane** does not apply to Spooky. Spooky uses a file-based YAML config; changes are applied by editing the file and calling `POST /admin/runtime/reload`, which hot-swaps routes, upstreams, backends, and policies live (only startup-owned settings and listener bind/removal changes need a restart). There is no push-based config distribution to sidecars. If your current setup relies on Envoy's ADS/EDS for dynamic backend discovery (e.g., service discovery via Consul or a service mesh), you cannot replicate that behavior with Spooky today. Alternatives: use a configuration management tool to render Spooky's config and restart on change, or continue using Envoy for the service mesh interior and put Spooky only at the edge.

**Lua scripting and WASM filters** are not supported. Envoy's Lua filter and WASM extension model, and NGINX's `lua-nginx-module`, have no equivalent in Spooky. Any per-request logic (custom header inspection, A/B routing logic, request signing) must move to the application layer or to a middleware service in front of Spooky.

**URL rewriting** (`rewrite` in NGINX, `regex_rewrite` in Envoy route config) is not available in v0.1.x. Spooky forwards requests to upstreams with the path unchanged. Workaround: handle path normalization at the origin, or keep NGINX in the chain for routes that require rewriting.

**Response header manipulation** (`add_header`, `proxy_hide_header`, `more_set_headers` in NGINX; `response_headers_to_add` in Envoy) is not available. Spooky passes response headers from the upstream to the client unmodified. Workaround: set headers at the origin service, or use a thin middleware layer (e.g., a simple HTTP wrapper service) for routes where specific headers are required.

**Per-IP rate limiting** (`limit_req_zone` / `limit_conn_zone` in NGINX; rate limit filter in Envoy). Spooky has scoped rate limiting (`resilience.scoped_rate_limits`) keyed by route, client, tenant, or token, plus global and per-upstream admission control — but not the full breadth of NGINX/Envoy rate-limit expressions, and no distributed/cross-instance rate limiting. For advanced or cross-instance throttling, place a dedicated rate-limiting layer (e.g., a Redis-backed rate limiter, or a cloud provider's WAF/shield service) in front of Spooky, or keep NGINX in the chain for routes that require per-IP throttling.

---

## Rollback Procedure

If something goes wrong after you cut traffic to Spooky, here is how to get back to your old proxy cleanly. This procedure assumes you followed the prerequisite of keeping the old proxy intact.

**Step 1: Do not remove the old proxy binary or config.**

Before starting any migration, verify that your old proxy's binary, config files, and TLS certificates are still in place on the host. Do not uninstall NGINX or Envoy until you have at least one full week of clean production traffic on Spooky.

**Step 2: Stop Spooky.**

```bash
systemctl stop spooky
```

This immediately stops Spooky from accepting new connections. In-flight QUIC connections will close when they hit the QUIC idle timeout (the default idle timeout in Spooky v0.1.x applies; check your config's `quic.idle_timeout` if you have set it explicitly). TCP connections that were in progress will be dropped.

**Step 3: Start your old proxy.**

```bash
# For NGINX:
systemctl start nginx

# For Envoy:
systemctl start envoy
```

Confirm the old proxy is listening and healthy before updating DNS:

```bash
curl -sk https://127.0.0.1:443/healthz
```

**Step 4: Update DNS or your load balancer rule.**

Point your domain's A/AAAA records back to the old proxy host (or update the LB target group). If you used weighted DNS for a canary, shift weight back to 100% on the old proxy immediately. With a short TTL (which you set before the migration — see prerequisites), propagation should complete within minutes.

**Step 5: Confirm traffic is flowing through the old proxy.**

Check your old proxy's access logs to confirm requests are arriving. Check your error rate and latency monitors to confirm they match your pre-migration baseline. Once stable, investigate the Spooky issue in a non-production environment before attempting the migration again.
