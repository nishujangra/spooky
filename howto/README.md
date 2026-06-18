# How-To Guides

Step-by-step guides for common Spooky tasks.

| Guide | What it covers |
|-------|---------------|
| [01-certificates.md](01-certificates.md) | Setting up TLS certificates — mkcert (dev), OpenSSL self-signed, Let's Encrypt (production), multi-domain SNI, browser trust |
| [02-configuration.md](02-configuration.md) | Building a complete config file — listeners, upstreams, routing, load balancing, health checks, host policy, forwarded headers, performance tuning |
| [03-run.md](03-run.md) | Running Spooky — directly, as a systemd service, in Docker; startup sequence, health checks, graceful shutdown, troubleshooting |

## Quick path for first-time setup

1. **Generate certificates** → [01-certificates.md](01-certificates.md)
2. **Write your config** → [02-configuration.md](02-configuration.md) (or copy `config/config.reverse.yaml`)
3. **Start Spooky** → [03-run.md](03-run.md)
