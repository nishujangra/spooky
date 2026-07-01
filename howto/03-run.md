# How to Run Spooky

This guide covers running Spooky directly, as a systemd service, and in Docker — including startup validation, graceful shutdown, and health checking.

---

## Prerequisites

Before starting Spooky you need:

1. A valid config file (see [02-configuration.md](02-configuration.md))
2. TLS certificates (see [01-certificates.md](01-certificates.md))
3. The `spooky` binary — built from source or installed via package

---

## Build from Source

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Clone and build
git clone https://github.com/Supernova-Labs-Org/spooky.git
cd spooky
cargo build --release

# Binary is at
./target/release/spooky
```

---

## Run Directly

### Basic start

```bash
spooky --config /etc/spooky/config.yaml
```

### Validate config without starting

```bash
spooky --config /etc/spooky/config.yaml --validate
```

Spooky exits 0 on success, 1 on validation failure, with a descriptive error message.

### Foreground with debug logging (development)

Override the log level at runtime by setting the level in your config or using a dev config:

```yaml
log:
  level: debug
  format: plain
```

```bash
spooky --config config/config.development.yaml
```

### Binding port 443 without root

```bash
# Grant the binary permission to bind privileged ports
sudo setcap cap_net_bind_service=+ep /usr/bin/spooky

# Now run as a regular user
spooky --config /etc/spooky/config.yaml
```

### Binding port 443 as root with privilege drop

If Spooky starts as root and `security.privileges.enabled=true`, it drops to the configured user/group after binding the socket:

```bash
sudo spooky --config /etc/spooky/config.yaml
# Spooky binds port 443 as root, then drops to user 'spooky'
```

---

## Run as a systemd Service

### Create the system user and directories

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin spooky
sudo mkdir -p /etc/spooky/certs /var/log/spooky
sudo chown -R spooky:spooky /etc/spooky /var/log/spooky
```

### Install the binary

```bash
sudo cp target/release/spooky /usr/bin/spooky
sudo chmod 755 /usr/bin/spooky

# Grant port 443 binding if not running as root
sudo setcap cap_net_bind_service=+ep /usr/bin/spooky
```

### Copy your config and certificates

```bash
sudo cp config/config.reverse.yaml /etc/spooky/config.yaml
sudo cp certs/fullchain.pem /etc/spooky/certs/fullchain.pem
sudo cp certs/privkey.pem   /etc/spooky/certs/privkey.pem
sudo chown spooky:spooky /etc/spooky/certs/*
sudo chmod 640 /etc/spooky/certs/*
```

### Create the systemd unit file

Create `/etc/systemd/system/spooky.service`:

```ini
[Unit]
Description=Spooky HTTP/3 Reverse Proxy
Documentation=https://github.com/Supernova-Labs-Org/spooky
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=spooky
Group=spooky
ExecStart=/usr/bin/spooky --config /etc/spooky/config.yaml
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
RestartSec=5s
LimitNOFILE=65536

# Logging — journald captures stdout/stderr
StandardOutput=journal
StandardError=journal
SyslogIdentifier=spooky

# Security hardening
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=/var/log/spooky
ReadOnlyPaths=/etc/spooky

[Install]
WantedBy=multi-user.target
```

> If you run Spooky as root to bind port 443 and rely on privilege drop, change `User=` and `Group=` to `root` and let `security.privileges` handle the drop. Otherwise use `AmbientCapabilities=CAP_NET_BIND_SERVICE` with the `spooky` user.

### Enable and start

```bash
sudo systemctl daemon-reload
sudo systemctl enable spooky
sudo systemctl start spooky

# Check status
sudo systemctl status spooky

# Follow logs
sudo journalctl -u spooky -f
```

### Graceful reload (after cert renewal)

```bash
# Signal spooky to reload (currently triggers restart)
sudo systemctl reload spooky

# Or full restart
sudo systemctl restart spooky
```

---

## Run in Docker

### Dockerfile

```dockerfile
FROM debian:bookworm-slim

RUN useradd --system --no-create-home --shell /usr/sbin/nologin spooky

COPY target/release/spooky /usr/bin/spooky
RUN chmod 755 /usr/bin/spooky

RUN mkdir -p /etc/spooky/certs /var/log/spooky \
    && chown -R spooky:spooky /etc/spooky /var/log/spooky

USER spooky

EXPOSE 9889/udp 9889/tcp

ENTRYPOINT ["/usr/bin/spooky", "--config", "/etc/spooky/config.yaml"]
```

### docker-compose.yml

```yaml
services:
  spooky:
    build: .
    ports:
      - "9889:9889/udp"
      - "9889:9889/tcp"
    volumes:
      - ./config/config.reverse.yaml:/etc/spooky/config.yaml:ro
      - ./certs:/etc/spooky/certs:ro
      - spooky-logs:/var/log/spooky
    restart: unless-stopped

volumes:
  spooky-logs:
```

```bash
docker compose up -d
docker compose logs -f spooky
```

---

## Startup Sequence

When Spooky starts, it follows this order:

1. Reads and parses the config file
2. Initializes logging and tracing
3. Validates the config — exits with error on failure
4. Checks if root is required (port < 1024)
5. Builds shared runtime state (route index, connection pools)
6. Binds UDP sockets (one per worker, or SO_REUSEPORT group)
7. Starts the bootstrap TLS listener (HTTP/1.1 + HTTP/2 compatibility)
8. Drops privileges if running as root and `security.privileges.enabled=true`
9. Spawns worker threads (data plane)
10. Spawns control-plane tasks (health checks, metrics)
11. Emits structured startup logs for topology, worker layout, and runtime settings — ready to accept connections

---

## Health and Readiness Checks

If `observability.control_api.enabled=true`:

```bash
# Liveness — is the process alive?
curl http://127.0.0.1:9902/health

# Readiness — is Spooky ready to serve traffic?
curl http://127.0.0.1:9902/ready

# Runtime info (requires auth token)
curl -H "Authorization: Bearer <token>" http://127.0.0.1:9902/admin/runtime
```

---

## Graceful Shutdown

Spooky handles `SIGTERM` and `SIGINT` (Ctrl+C):

1. Stops accepting new QUIC connections
2. Waits for in-flight requests to complete (up to `performance.shutdown_drain_timeout_ms`)
3. Exits cleanly

Set a generous drain timeout for long-lived streaming requests:

```yaml
performance:
  shutdown_drain_timeout_ms: 10000   # 10 seconds
```

---

## Verifying Spooky is Running

### Test HTTP/3 (QUIC)

```bash
# Requires curl with HTTP/3 support
curl --http3-only -k https://localhost:9889/

# With a hostname
curl --http3-only -k https://api.example.com/health
```

### Test HTTP/2 (bootstrap TLS listener)

```bash
curl --http2 -k https://localhost:9889/
```

### Check Alt-Svc header (upgrade advertisement)

```bash
curl -Ik https://localhost:9889/ | grep -i alt-svc
# Should show: alt-svc: h3=":9889"; ma=86400
```

### Check Prometheus metrics

```bash
curl http://127.0.0.1:9901/metrics
```

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|-------------|-----|
| `Failed to bind UDP socket: Permission denied` | Port < 1024 without root or CAP_NET_BIND_SERVICE | Use `sudo` or `setcap` |
| `Cannot open listen.tls.cert` | Wrong path or permissions | Check path; `chown spooky:spooky /etc/spooky/certs/*` |
| `worker_threads > 1 requires reuseport=true` | Config mismatch | Add `reuseport: true` to performance |
| Clients get `connection refused` on TCP | Bootstrap TLS listener failed to bind | Check logs for bootstrap bind error |
| `curl: (35) OpenSSL SSL_connect` | Certificate mismatch or untrusted | See [01-certificates.md](01-certificates.md) |
| Health check always fails | Backend unreachable or wrong health path | Verify backend is up and health path returns 200 |
| High memory usage | `max_response_body_bytes` too high or streaming not draining | Tune body caps in performance section |
