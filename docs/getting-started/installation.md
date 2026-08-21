# Installation

This page helps you install Impulse on a host and reach the point where you can run it safely.

## Start Here

- Want the fastest local success path: [Quickstart](../tutorials/quickstart.md)
- Want a container-first path: [Docker](docker.md)
- Want to install on a Linux host: continue below
- Want the minimum safe production posture after install: [Minimum Production](minimum-production.md)

## System Requirements

**Hardware:**
- CPU: 1 core minimum (2+ cores recommended for production)
- Memory: 256MB RAM minimum (1GB+ recommended)
- Disk: 100MB for binary and configuration files
- Network: UDP port access for QUIC traffic

**Software:**
- Rust 1.85 or later (2024 edition)
- Operating System: Linux (runtime supported; macOS and Windows may compile but are not supported for production use)
- Build tools: CMake, pkg-config, C compiler toolchain

**Permissions:**
- Root is only required when binding privileged ports (`<1024`).
- For typical deployments, run Impulse as an unprivileged user on a non-privileged port.

## Installation Methods

### Debian Package (Recommended for Linux)

Download and install the `.deb` from [GitHub Releases](https://github.com/Supernova-Labs-Org/impulse/releases):

```bash
wget https://github.com/Supernova-Labs-Org/impulse/releases/download/v0.5.1-beta/impulse_0.5.1-beta_amd64.deb
sudo dpkg -i impulse_0.5.1-beta_amd64.deb
```

The package installs:
- Binary: `/usr/bin/impulse`
- Default config: `/etc/impulse/config.yaml`
- Certs directory: `/etc/impulse/certs/`
- Log directory: `/var/log/impulse/`
- Systemd unit: `/lib/systemd/system/impulse.service`
- System user/group: `impulse`

After install, place your TLS certificates (see [TLS Certificates](#tls-certificates) below), edit `/etc/impulse/config.yaml`, then start the service:

```bash
sudo systemctl restart impulse
sudo systemctl status impulse
```

To build a `.deb` package from source in this repository:

```bash
./packaging/deb/make-deb.sh
sudo dpkg -i impulse_0.1.1-beta_amd64.deb
```

### Build from Source

**Install build dependencies** (required — quiche/BoringSSL needs cmake and a C++ compiler):

```bash
# Ubuntu/Debian
sudo apt install -y cmake build-essential pkg-config

# CentOS/RHEL
sudo dnf groupinstall -y "Development Tools" && sudo dnf install -y cmake pkgconfig

# macOS
brew install cmake pkg-config
```

**Clone and build:**
```bash
git clone https://github.com/Supernova-Labs-Org/impulse.git
cd impulse
cargo build --release
```

The binary is generated at `target/release/impulse`.

**Run tests (optional):**
```bash
cargo test
cargo test -p impulse-edge --test lb_integration
```

**System-wide installation:**
```bash
sudo install -m 755 target/release/impulse /usr/bin/impulse
```

## TLS Certificates

Impulse requires TLS certificates to serve QUIC/HTTP3 traffic. The service runs as the `impulse` user, so certificates must be readable by that user.

### Using Your Own Certificates

Copy your certificate and private key into the certs directory and set correct ownership:

```bash
# Copy certificates
sudo cp /path/to/fullchain.pem /etc/impulse/certs/fullchain.pem
sudo cp /path/to/privkey.pem   /etc/impulse/certs/privkey.pem

# Set ownership and permissions (root owns, impulse group can read)
sudo chown root:impulse /etc/impulse/certs/fullchain.pem /etc/impulse/certs/privkey.pem
sudo chmod 640 /etc/impulse/certs/fullchain.pem /etc/impulse/certs/privkey.pem
```

Then update `/etc/impulse/config.yaml` to point to these paths:

```yaml
listen:
  tls:
    cert: "/etc/impulse/certs/fullchain.pem"
    key:  "/etc/impulse/certs/privkey.pem"
```

### Using the Repo's Development Certificates

If you are building from source and want to use the included development certificates (located in `certs/` in the repo), copy them in the same way:

```bash
sudo cp certs/proxy-fullchain.pem /etc/impulse/certs/fullchain.pem
sudo cp certs/proxy-key-pkcs8.pem /etc/impulse/certs/privkey.pem

sudo chown root:impulse /etc/impulse/certs/fullchain.pem /etc/impulse/certs/privkey.pem
sudo chmod 640 /etc/impulse/certs/fullchain.pem /etc/impulse/certs/privkey.pem
```

Update `/etc/impulse/config.yaml`:

```yaml
listen:
  tls:
    cert: "/etc/impulse/certs/fullchain.pem"
    key:  "/etc/impulse/certs/privkey.pem"
```

> **Note:** The development certificates are signed by the repo's test CA (`certs/ca-cert.pem`). Do not use them in production.

### Generating Self-Signed Certificates

For quick local testing without the repo's dev certs:

```bash
openssl req -x509 -newkey rsa:4096 -nodes \
  -keyout /tmp/privkey.pem \
  -out /tmp/fullchain.pem \
  -days 365 \
  -subj "/CN=proxy.example.com"

sudo mv /tmp/fullchain.pem /etc/impulse/certs/fullchain.pem
sudo mv /tmp/privkey.pem   /etc/impulse/certs/privkey.pem
sudo chown root:impulse /etc/impulse/certs/fullchain.pem /etc/impulse/certs/privkey.pem
sudo chmod 640 /etc/impulse/certs/fullchain.pem /etc/impulse/certs/privkey.pem
```

For production certificates, see [TLS Configuration](../configuration/tls.md).

## Post-Installation Configuration

### Manual Setup (non-package installs)

If you installed from source or a tarball, set up the directories, user, and service manually:

```bash
# Create directories
sudo mkdir -p /etc/impulse/certs
sudo mkdir -p /var/log/impulse

# Create system user
sudo groupadd --system impulse
sudo useradd --system --gid impulse --no-create-home \
     --home-dir /etc/impulse --shell /usr/sbin/nologin \
     --comment "Impulse reverse proxy" impulse

# Set ownership
sudo chown -R impulse:impulse /etc/impulse /var/log/impulse
sudo chmod 750 /etc/impulse /etc/impulse/certs /var/log/impulse

# Copy default config
sudo install -m 0640 -o impulse -g impulse packaging/deb/debian/config.yaml /etc/impulse/config.yaml
```

Then place TLS certificates as described above, and install the systemd unit:

```bash
sudo install -m 0644 packaging/deb/debian/impulse.service /lib/systemd/system/impulse.service
sudo systemctl daemon-reload
sudo systemctl enable impulse.service
sudo systemctl start impulse.service
```

### Configuration File

Edit `/etc/impulse/config.yaml` to match your environment. Minimal working example:

```yaml
version: 1

listen:
  protocol: http3
  address: "0.0.0.0"
  port: 9889
  tls:
    cert: "/etc/impulse/certs/fullchain.pem"
    key:  "/etc/impulse/certs/privkey.pem"

upstream:
  default:
    load_balancing:
      type: round-robin
    route:
      path_prefix: "/"
    backends:
      - id: "backend1"
        address: "backend.internal.example:8443"
        weight: 100
        health_check:
          path: "/health"
          interval: 5000
          timeout_ms: 1000
          failure_threshold: 3
          success_threshold: 2
          cooldown_ms: 5000

log:
  level: info
  format: json
  file:
    enabled: true
    path: /var/log/impulse/impulse.log
```

See [Configuration Reference](../configuration/reference.md) for all options.

## First Run After Installation

Once the binary, certificates, and config are in place:

1. start Impulse with your config
2. verify the control API health endpoint
3. send one proxied request
4. move to the minimum-production checklist before serving real traffic

Recommended next pages:

- [Quickstart](../tutorials/quickstart.md)
- [Minimum Production](minimum-production.md)
- [Production Deployment](../deployment/production.md)

### Log Rotation

Configure log rotation for file-based logging. Create `/etc/logrotate.d/impulse`:

```
/var/log/impulse/*.log {
    daily
    rotate 14
    compress
    delaycompress
    missingok
    notifempty
    create 0640 impulse impulse
    sharedscripts
    postrotate
        systemctl restart impulse.service >/dev/null 2>&1 || true
    endscript
}
```

## Platform-Specific Notes

### Ubuntu/Debian

Install build dependencies before building from source:

```bash
sudo apt update
sudo apt install -y cmake build-essential pkg-config

# Install Rust if not present
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

### CentOS/RHEL 8+

```bash
sudo dnf groupinstall -y "Development Tools"
sudo dnf install -y cmake pkgconfig

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

### macOS

```bash
brew install cmake pkg-config rust
```

### Windows

> **Note:** Windows is not a supported runtime platform. The Impulse binary uses Unix-specific APIs (signals, `getuid`) that are not available on Windows. The instructions below may allow a build to succeed, but running Impulse on Windows in production is not supported.

1. Install Rust from [rustup.rs](https://rustup.rs/)
2. Install Visual Studio Build Tools with C++ support from [Microsoft](https://visualstudio.microsoft.com/visual-cpp-build-tools/)

Binary location after build: `target\release\impulse.exe`

## Docker Deployment

```dockerfile
FROM rust:1.85-slim as builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && \
    apt-get install -y ca-certificates && \
    rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/impulse /usr/bin/impulse
EXPOSE 9889/udp
CMD ["impulse", "--config", "/etc/impulse/config.yaml"]
```

```bash
docker run -d \
  --name impulse \
  -p 9889:9889/udp \
  -v /etc/impulse/config.yaml:/etc/impulse/config.yaml:ro \
  -v /etc/impulse/certs:/etc/impulse/certs:ro \
  impulse:latest
```

## Verification

```bash
# Check service status
sudo systemctl status impulse

# Validate configuration (runs startup validation then exits)
sudo -u impulse impulse --config /etc/impulse/config.yaml

# View logs
sudo journalctl -u impulse -f
# or if file logging is enabled:
sudo tail -f /var/log/impulse/impulse.log
```

## Troubleshooting

**`/etc/impulse/config.yaml` missing after `dpkg -i`:**
The package install may have been interrupted. Reinstall or manually restore the file:
```bash
sudo dpkg -i impulse_0.1.1-beta_amd64.deb
# or:
sudo install -m 0640 -o impulse -g impulse packaging/deb/debian/config.yaml /etc/impulse/config.yaml
```

**`Permission denied` on TLS key/cert:**
The `impulse` user cannot read the certificate files. Fix ownership and permissions:
```bash
sudo chown root:impulse /etc/impulse/certs/fullchain.pem /etc/impulse/certs/privkey.pem
sudo chmod 640 /etc/impulse/certs/fullchain.pem /etc/impulse/certs/privkey.pem
sudo systemctl restart impulse
```

**`Permission denied` when binding to port < 1024:**
Use a port > 1024, or grant the capability:
```bash
sudo setcap CAP_NET_BIND_SERVICE=+eip /usr/bin/impulse
```

**Build fails with linker errors:**
Ensure build tools are installed: `cmake`, `pkg-config`, C compiler. Update Rust: `rustup update`.

**Certificate errors on startup:**
Verify paths in config match actual file locations. Validate format:
```bash
openssl x509 -in /etc/impulse/certs/fullchain.pem -text -noout
```

## Next Steps

- [Configuration Reference](../configuration/reference.md) — Complete configuration options
- [TLS Setup Guide](../configuration/tls.md) — Production certificate management
- [Production Deployment](../deployment/production.md) — Production deployment best practices
- [Troubleshooting](../troubleshooting/common-issues.md) — Common issues and solutions
