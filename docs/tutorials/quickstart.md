# Quickstart

This guide gets Impulse from zero to first successful traffic with the fewest moving parts possible.

Total time: about 5 minutes.

## Prerequisites

- **Rust 1.85+** (edition 2024) — `rustup update stable`
- **curl with HTTP/3 support** — the curl that ships with macOS does not include HTTP/3. Install one that does:
  ```bash
  brew install curl
  # then use $(brew --prefix curl)/bin/curl in the commands below, or put it first on PATH
  ```
- **Python 3** for the simplest local backend: `python3 --version`
- **UDP port 9889 free** — QUIC runs over UDP. Check with `lsof -iUDP:9889`.

## What You Will Run

This quickstart uses:

- a self-signed certificate for local TLS
- a small local HTTP backend on `127.0.0.1:8080`
- one catch-all upstream in Impulse
- one HTTP/3 request to confirm first traffic

## Step 1: Build

```bash
git clone https://github.com/Supernova-Labs-Org/spooky.git
cd spooky
cargo build --release
```

The binary lands at `target/release/spooky`.

## Step 2: Generate a Certificate

QUIC requires TLS 1.3. For local testing, a self-signed certificate works fine:

```bash
mkdir -p certs
openssl req -x509 -newkey rsa:4096 -nodes \
  -keyout certs/key.pem \
  -out certs/cert.pem \
  -days 365 \
  -subj "/CN=localhost"
```

Production: see [TLS Setup](../configuration/tls.md).

## Step 3: Start a Test Backend

Impulse supports:

- HTTP/2 upstream transport for `https://` backends
- HTTP/1.1 upstream transport for `http://` backends

For the fastest local test, use a simple HTTP/1.1 backend:

```bash
mkdir -p /tmp/spooky-demo
printf 'hello from backend\n' > /tmp/spooky-demo/index.html
cd /tmp/spooky-demo
python3 -m http.server 8080
```

Leave this running in its own terminal.

## Step 4: Write the Config

Create `config.yaml` in the repository root:

```yaml
version: 1                        # config schema version — must be 1

listen:
  protocol: http3                 # accept QUIC/HTTP/3 on this socket
  port: 9889                      # UDP port clients connect to
  address: "0.0.0.0"             # bind all interfaces; use 127.0.0.1 for loopback-only
  tls:
    cert: "certs/cert.pem"        # path to PEM-encoded certificate chain
    key: "certs/key.pem"          # path to PEM-encoded private key

upstream:
  default:                        # pool name — referenced internally; "default" catches all unmatched routes
    load_balancing:
      type: round-robin           # distribute requests evenly across backends in order
    route:
      path_prefix: "/"            # match every request path
    backends:
      - id: backend-1             # arbitrary label shown in logs
        address: "http://127.0.0.1:8080" # cleartext HTTP/1.1 backend for the local demo
        weight: 100               # relative share of traffic (only meaningful with multiple backends)

log:
  level: info                     # debug | info | warn | error
```

## Step 5: Start Impulse

```bash
./target/release/spooky --config config.yaml
```

You should see:

```
INFO spooky: loading config path="config.yaml"
INFO spooky: listening on 0.0.0.0:9889 protocol=http3
INFO spooky: upstream ready upstream=default backends=1
```

## Step 6: Verify HTTP/3

### 6a. Force HTTP/3 (confirms QUIC is working)

```bash
curl --http3-only -k https://localhost:9889/
```

`--http3-only` refuses to fall back to TCP. If this succeeds, QUIC is live.

Expected body:

```text
hello from backend
```

### 6b. Check the control API health endpoint

In another terminal:

```bash
curl -sk --http1.1 https://127.0.0.1:9902/health
```

Expected response:

```json
{"status":"ok", ...}
```

### 6c. Verify the Alt-Svc upgrade path (mimics browser behavior)

Browsers don't start with HTTP/3 — they discover it via the `Alt-Svc` response header on a regular HTTPS request, then switch on the next connection. Test that Impulse sends this header correctly:

```bash
curl -k -I https://localhost:9889/
```

Look for this line in the response headers:

```
alt-svc: h3=":9889"; ma=86400
```

`h3=":9889"` tells the client that HTTP/3 is available on port 9889. `ma=86400` is the max-age in seconds (24 hours) — how long the client should remember and prefer HTTP/3 for this origin.

If you see this header, Impulse is correctly advertising HTTP/3 to clients that don't yet support it or haven't upgraded yet.

## Common Issues

**`Error: Address already in use`** — something else is bound to UDP 9889. Find it with `lsof -iUDP:9889` and stop it, or change `port` in `config.yaml`.

**`Failed to connect to backend`** — the local backend is not running, or is on a different port. Confirm it is up with `curl http://127.0.0.1:8080/`.

**`Failed to load TLS certificate`** — the paths in `config.yaml` don't match where you generated the files. Both `certs/cert.pem` and `certs/key.pem` must exist relative to the working directory you launch Impulse from.

**curl falls back to HTTP/2 silently** — you're using the system curl, which lacks HTTP/3 support. Use `brew install curl` and invoke it with the full path, or check `curl --version` for `HTTP/3` in the features list.

## Next Steps

- [Docker](../getting-started/docker.md) — fastest container-based first run
- [Installation](../getting-started/installation.md) — install Impulse on a host
- [Configuration Reference](../configuration/reference.md) — exact config keys, defaults, and semantics
- [Minimum Production](../getting-started/minimum-production.md) — minimum safe production posture
- [Production Deployment](../deployment/production.md) — full deployment and hardening guide
- [Load Balancing Guide](../user-guide/load-balancing.md) — strategy selection and routing trade-offs
- [TLS Setup](../configuration/tls.md) — production certificates, rotation, and mTLS
