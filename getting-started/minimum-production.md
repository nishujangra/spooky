# Minimum Production Checklist

This page answers one question: **what is the minimum you must do to run Spooky responsibly in production?**

It is not a full hardening guide — that is [Production Deployment](../deployment/production.md). This page is a focused checklist for operators who have already completed the [quickstart](overview.md) and are preparing to serve real traffic for the first time.

---

## Minimum Production Config

The config below is a realistic starting point for a single-host deployment. Copy it to `/etc/spooky/config.yaml` and edit the addresses and certificate paths to match your environment.

```yaml
# /etc/spooky/config.yaml
version: 1

listen:
  protocol: http3
  address: "0.0.0.0"
  port: 443                          # Requires CAP_NET_BIND_SERVICE or root at start
  tls:
    cert: "/etc/spooky/certs/fullchain.pem"   # Full chain, not just the leaf
    key:  "/etc/spooky/certs/privkey.pem"     # PKCS#8 PEM; mode 640, owner root:spooky

upstream:
  # API pool — more-specific prefix wins over the default "/" below
  api:
    load_balancing:
      type: "round-robin"            # Even distribution; switch to least-connections if backends differ in capacity
    route:
      path_prefix: "/api"
    backends:
      - id: "api-1"
        address: "10.0.10.10:8080"
        weight: 100
        health_check:
          path: "/health"            # Must return 2xx; Spooky removes the backend after 3 consecutive failures
          interval: 5000             # ms — balance detection speed against backend poll load
          timeout_ms: 2000
          failure_threshold: 3
          success_threshold: 2
          cooldown_ms: 10000         # Wait 10 s before retrying a failed backend
      - id: "api-2"
        address: "10.0.10.11:8080"
        weight: 100
        health_check:
          path: "/health"
          interval: 5000
          timeout_ms: 2000
          failure_threshold: 3
          success_threshold: 2
          cooldown_ms: 10000

  # Default pool — catch-all for everything not matched above
  default:
    load_balancing:
      type: "round-robin"
    route:
      path_prefix: "/"               # Shortest prefix — matched only when nothing else fits
    backends:
      - id: "web-1"
        address: "10.0.20.10:8080"
        weight: 100
        health_check:
          path: "/health"
          interval: 10000
          timeout_ms: 2000
          failure_threshold: 3
          success_threshold: 2
          cooldown_ms: 10000

log:
  level: info
  format: json                       # Structured logs — required for any log aggregation pipeline

security:
  privileges:
    enabled: true
    user: "spooky"                   # Drop to unprivileged user immediately after binding port 443
    group: "spooky"

observability:
  control_api:
    enabled: true
    address: "127.0.0.1"             # Loopback only — never expose on 0.0.0.0 without auth and firewall
    port: 9902
  metrics:
    enabled: true
    address: "127.0.0.1"             # Expose to monitoring; restrict to your scrape network
    port: 9090
    path: "/metrics"
```

**What this config does not include:** mTLS (client certificate validation), custom QUIC performance tuning, and per-upstream overload caps. Those are optional; add them when you have a reason. See [Configuration Reference](../configuration/reference.md) for the full schema.

---

## System Requirements

| Requirement | Minimum | Notes |
|---|---|---|
| OS | Linux | x86\_64 or arm64; macOS is not supported for production |
| Kernel | 5.0 | Earlier kernels lack stable UDP GRO support needed by QUIC |
| Open file limit (`nofile`) | 65 536 | QUIC maintains one UDP socket per worker; each active connection also holds file descriptors for H/2 backend streams |
| Recommended `nofile` | 1 048 576 | Headroom for high connection counts without emergency restarts |
| Memory | 256 MB | Minimum; 1 GB recommended at moderate load |

**UDP receive buffer — required tuning.** QUIC delivers all traffic over UDP. The kernel's default UDP receive buffer (typically 212 KB) causes packet drops under any real load. Set it before starting Spooky:

```bash
# /etc/sysctl.d/99-spooky.conf
net.core.rmem_max = 67108864    # 64 MiB — maximum socket receive buffer size
net.core.rmem_default = 16777216  # 16 MiB — default for new sockets
```

Apply immediately with `sudo sysctl -p /etc/sysctl.d/99-spooky.conf`. Without this, the kernel silently drops UDP packets when bursts arrive faster than Spooky reads them, producing unexplained connection timeouts.

Set ulimits for the service account:

```
# /etc/security/limits.d/spooky.conf
spooky soft nofile 1048576
spooky hard nofile 1048576
```

---

## Before You Start Serving Traffic — Checklist

Work through this list top-to-bottom before pointing DNS or a load balancer at the instance.

1. **TLS certificate is from a trusted CA, not self-signed.**
   Browsers and HTTP/3 clients reject self-signed certificates without manual trust store configuration, making this a hard requirement for public traffic.
   _Verify:_ `openssl x509 -noout -issuer -in /etc/spooky/certs/fullchain.pem` — the issuer should be your CA, not the subject itself.

2. **Certificate file ownership is `root:spooky`, mode `640` for the key.**
   The `spooky` service user needs to read the key at startup; `640` gives it read access without making the key world-readable.
   _Verify:_ `ls -l /etc/spooky/certs/` — expect `-rw-r----- root spooky fullchain.pem` and `-rw-r----- root spooky privkey.pem`.

3. **Config passes startup validation.**
   Spooky validates its config before accepting connections. A config error will cause the service to exit immediately, but you want to confirm this before systemd is involved.
   _Verify:_ `sudo -u spooky /usr/local/bin/spooky --config /etc/spooky/config.yaml` — the process should start and begin logging without printing a fatal error. Stop it with Ctrl-C once you see the listening message.

4. **At least one health check endpoint is verified to return 2xx.**
   Spooky removes backends that fail health checks; if every backend in a pool is unhealthy at startup, all requests to that pool return 503 immediately.
   _Verify:_ `curl -sf http://<backend-ip>:<port>/health` for each backend listed in the config — expect HTTP 200.

5. **Control API is bound to loopback only (`127.0.0.1`) unless you have a strong administrative network boundary.**
   The control API supports bearer-token authentication, but it is still a privileged admin surface and should not be treated like a public endpoint.
   _Verify:_ Confirm `observability.control_api.address` is `127.0.0.1` in `/etc/spooky/config.yaml`, then after start: `ss -tlnp | grep 9902` — the local address column should show `127.0.0.1:9902`.

6. **Metrics endpoint is reachable from your monitoring system.**
   You cannot respond to incidents you cannot observe. Verify the scrape works before traffic arrives, not after something breaks.
   _Verify:_ From your Prometheus host (or monitoring agent): `curl -sf http://<spooky-host>:9090/metrics | head -5` — expect Prometheus text format output.

7. **Rollback plan documented: previous binary and config saved, procedure tested.**
   v0.1.x does not support hot reload — every config change requires a restart. If a bad config or binary is deployed, the service fails to start and traffic stops.
   _Verify:_ Confirm `/usr/local/bin/spooky.prev` and `/etc/spooky/config.yaml.backup` exist from your last deployment, and that the rollback steps in your runbook have been executed at least once in a non-production environment.

---

## Starting and Monitoring

Install the following systemd unit, then use the three commands below to confirm a healthy start.

```ini
# /etc/systemd/system/spooky.service
[Unit]
Description=Spooky HTTP/3 Edge Proxy
Documentation=https://github.com/Supernova-Labs-Org/spooky
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/spooky --config /etc/spooky/config.yaml
Restart=on-failure
RestartSec=5s
StartLimitBurst=3
StartLimitIntervalSec=60s

# Resource limits — must match /etc/security/limits.d/spooky.conf
LimitNOFILE=1048576

# Systemd sandbox — reduce blast radius if the process is compromised
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/log/spooky
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictNamespaces=true
SystemCallArchitectures=native
SystemCallFilter=@system-service

# Log to journald
StandardOutput=journal
StandardError=journal
SyslogIdentifier=spooky

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now spooky.service
```

After starting, run these three checks in order:

```bash
# 1. Confirm the unit reached 'active (running)' and did not immediately exit
sudo systemctl status spooky.service

# 2. Confirm no fatal errors in the first few seconds of startup
sudo journalctl -u spooky.service -n 50 --no-pager

# 3. Confirm Spooky is reachable and at least one backend is healthy
curl -sf http://127.0.0.1:9902/health
```

The control API `/health` endpoint returns HTTP 200 when the process is up. It does not verify backend health — use the metrics endpoint (`spooky_health_checks_success`) for that.

---

## First Scaling Steps

**When to add backends.** CPU saturation is rarely the first bottleneck in a QUIC proxy. Watch `spooky_overload_shed_by_reason_total{reason="backend_inflight"}` in your metrics — sustained shedding on that label means backends are saturated before Spooky is. Add backends to the pool when that counter climbs, not when Spooky's CPU rises.

**How to add a backend in v0.1.x.** Dynamic backend registration is not supported in v0.1.x. To add a backend, update `/etc/spooky/config.yaml` with the new entry and restart the service with `sudo systemctl restart spooky.service`. Prepare for a brief interruption (existing QUIC connections are dropped on restart). Schedule the change during a low-traffic window or behind a second proxy instance if zero-downtime is required. Hot reload is on the roadmap.

---

## Related Docs

- [TLS Setup](../configuration/tls.md) — Certificate formats, PKCS#8 conversion, Let's Encrypt automation, and rotation procedures
- [Production Deployment](../deployment/production.md) — Full hardening guide: HA architecture, nftables rules, AppArmor, alerting rules, and incident runbooks
- [Troubleshooting](../troubleshooting/common-issues.md) — Diagnosis commands for common startup failures, backend connection errors, and high latency
- [Load Balancing Guide](../user-guide/load-balancing.md) — Algorithm selection, consistent hashing key configuration, and least-connections vs. latency-aware trade-offs
