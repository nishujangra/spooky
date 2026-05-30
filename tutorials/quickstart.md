# Quickstart

Spooky is an HTTP/3 (QUIC) edge reverse proxy. Clients connect over HTTP/3; Spooky forwards to your existing HTTP/2 backends unchanged. This guide gets a working proxy running locally and confirms the full upgrade path — including the `Alt-Svc` header that tells browsers to switch to HTTP/3.

Total time: about 5 minutes.

## Prerequisites

- **Rust 1.85+** (edition 2024) — `rustup update stable`
- **curl with HTTP/3 support** — the curl that ships with macOS does not include HTTP/3. Install one that does:
  ```bash
  brew install curl
  # then use $(brew --prefix curl)/bin/curl in the commands below, or put it first on PATH
  ```
  Alternatively, use Spooky's own `h2_backend` test client (shown in Step 3) to confirm connectivity without curl.
- **UDP port 9889 free** — QUIC runs over UDP. Check with `lsof -iUDP:9889`.

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

Spooky requires HTTP/2 backends. HTTP/1.1-only upstreams are not supported.

Use the bundled test backend, which speaks HTTP/2 out of the box:

```bash
cargo run --bin h2_backend -- --port 8080
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
        address: "127.0.0.1:8080" # where to forward — must be an HTTP/2 endpoint
        weight: 100               # relative share of traffic (only meaningful with multiple backends)

log:
  level: info                     # debug | info | warn | error
```

## Step 5: Start Spooky

```bash
./target/release/spooky --config config.yaml
```

You should see:

```
INFO spooky: loading config path="config.yaml"
INFO spooky: listening on 0.0.0.0:9889 protocol=http3
INFO spooky: upstream pool ready pool=default backends=1
```

## Step 6: Verify HTTP/3

### 6a. Force HTTP/3 (confirms QUIC is working)

```bash
curl --http3-only -k https://localhost:9889/
```

`--http3-only` refuses to fall back to TCP. If this succeeds, QUIC is live.

### 6b. Verify the Alt-Svc upgrade path (mimics browser behavior)

Browsers don't start with HTTP/3 — they discover it via the `Alt-Svc` response header on a regular HTTPS request, then switch on the next connection. Test that Spooky sends this header correctly:

```bash
curl -k -I https://localhost:9889/
```

Look for this line in the response headers:

```
alt-svc: h3=":9889"; ma=86400
```

`h3=":9889"` tells the client that HTTP/3 is available on port 9889. `ma=86400` is the max-age in seconds (24 hours) — how long the client should remember and prefer HTTP/3 for this origin.

If you see this header, Spooky is correctly advertising HTTP/3 to clients that don't yet support it or haven't upgraded yet.

## Common Issues

**`Error: Address already in use`** — something else is bound to UDP 9889. Find it with `lsof -iUDP:9889` and stop it, or change `port` in `config.yaml`.

**`Failed to connect to backend`** — the h2_backend process isn't running, or is on a different port. Confirm it's up with `curl -k --http2 https://localhost:8080/` (expect a response, not a connection refused).

**`Failed to load TLS certificate`** — the paths in `config.yaml` don't match where you generated the files. Both `certs/cert.pem` and `certs/key.pem` must exist relative to the working directory you launch Spooky from.

**curl falls back to HTTP/2 silently** — you're using the system curl, which lacks HTTP/3 support. Use `brew install curl` and invoke it with the full path, or check `curl --version` for `HTTP/3` in the features list.

## What to Read Next

- **[Configuration Reference](../configuration/reference.md)** — every config field, its type, default value, and valid range.
- **[Load Balancing Guide](../user-guide/load-balancing.md)** — when to use round-robin vs. least-connections vs. random, and how weights interact.
- **[TLS Setup](../configuration/tls.md)** — production certificates with Let's Encrypt, cert rotation, and mTLS.
- **[Production Deployment](../deployment/production.md)** — systemd unit file, resource limits, metrics endpoints, and hardening checklist.
