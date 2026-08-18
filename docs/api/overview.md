# API Overview

This page summarizes the operator-facing programmatic surfaces. Use the linked reference pages for the canonical details.

## CLI Surface

Basic usage:

```bash
spooky --config /etc/spooky/config.yaml
```

Core options:

| Option | Meaning |
| --- | --- |
| `--config` / `-c` | Path to config file |
| `--version` / `-V` | Print version |
| `--help` / `-h` | Print usage |

## Control And Metrics Surfaces

Spooky exposes two main operator-facing HTTP surfaces when configured:

- a Prometheus metrics endpoint
- a control API for liveness, readiness, runtime visibility, staged activation, rollback, cert reload, and restart actions

### Surface Comparison

| Surface | Protocol | Main use | Start here |
| --- | --- | --- | --- |
| Metrics endpoint | HTTP `GET` | scrape, dashboarding, alerting, trend analysis | `GET /metrics` |
| Control API | HTTP/1.1 over TLS | runtime state, validate/preview/activate, rollback, cert reload, restart | `GET /health`, `GET /ready`, `GET /admin/runtime` |

### Common Operator Tasks

| Task | Endpoint | Example |
| --- | --- | --- |
| check that the process is alive | `GET /health` | `curl -k --http1.1 https://127.0.0.1:9902/health` |
| check that traffic should be admitted | `GET /ready` | `curl -k --http1.1 https://127.0.0.1:9902/ready` |
| inspect the active runtime generation and health state | `GET /admin/runtime` | `curl -k --http1.1 -H "Authorization: Bearer <token>" https://127.0.0.1:9902/admin/runtime` |
| inspect retained generations before rollback | `GET /admin/runtime/history` | `curl -k --http1.1 -H "Authorization: Bearer <token>" https://127.0.0.1:9902/admin/runtime/history` |
| validate a config before activation | `POST /admin/runtime/validate` | `curl -k --http1.1 -X POST -H "Authorization: Bearer <token>" -H "content-type: application/json" https://127.0.0.1:9902/admin/runtime/validate -d '{"config_path":"/etc/spooky/candidate.yaml"}'` |
| confirm Prometheus can scrape the process | `GET /metrics` | `curl -s http://127.0.0.1:9901/metrics` |

Use:

- [Metrics Reference](../reference/metrics-reference.md) for current metric families and first-alert guidance
- [Control API Reference](../reference/control-api-reference.md) for current endpoint behavior and security posture
- [Observability Operator Bundle](../operations/observability-bundle.md) for the packaged dashboards, alerts, and control-plane correlation workflow

## Configuration Surface

The canonical configuration docs live in:

- [Configuration Reference](../configuration/reference.md)
- [Configuration Examples](../configuration/examples.md)
- [TLS Setup](../configuration/tls.md)

## Important Scope Note

The control API applies configuration through a file-reload model, not a granular per-object API.

- it provides health, readiness, runtime inspection, restart actions, certificate reload, staged
  activation (`POST /admin/runtime/validate`, `/preview`, `/activate`), rollback
  (`POST /admin/runtime/rollback`), generation history (`GET /admin/runtime/history`), and the
  legacy full config reload shortcut (`POST /admin/runtime/reload`)
- config reload re-reads the config file and applies it live via an atomic runtime swap, including
  route, upstream, and backend changes — it is not a per-object mutation API (you edit the file and
  reload). `log.level` applies live; log format/file settings, tracing config, control-plane thread
  counts, and listener bind/removal changes still require a restart
- reloads default to the currently active runtime config source, which is the startup path until an
  alternate `config_path` is activated; a successful activation makes that file the new active source

### Minimal Control API Flow

```bash
# 1. validate a candidate
curl -k --http1.1 -X POST \
  -H "Authorization: Bearer <token>" \
  -H "content-type: application/json" \
  https://127.0.0.1:9902/admin/runtime/validate \
  -d '{"config_path":"/etc/spooky/candidate.yaml","requested_by":"ops","reason":"preflight"}'

# 2. preview the same candidate and record it in history
curl -k --http1.1 -X POST \
  -H "Authorization: Bearer <token>" \
  -H "content-type: application/json" \
  https://127.0.0.1:9902/admin/runtime/preview \
  -d '{"config_path":"/etc/spooky/candidate.yaml","requested_by":"ops","reason":"preview"}'

# 3. activate the candidate
curl -k --http1.1 -X POST \
  -H "Authorization: Bearer <token>" \
  -H "content-type: application/json" \
  https://127.0.0.1:9902/admin/runtime/activate \
  -d '{"config_path":"/etc/spooky/candidate.yaml","expected_generation":12,"requested_by":"ops","reason":"deploy"}'
```

## Related Pages

- [Metrics Reference](../reference/metrics-reference.md)
- [Control API Reference](../reference/control-api-reference.md)
- [Observability Operator Bundle](../operations/observability-bundle.md)
- [Operations Runbook](../operations/runbook.md)

## Configuration Validation

### Startup Validation

Configuration validation is performed automatically at startup before the QUIC listener is initialized. The validation process verifies:

- Configuration file format and syntax
- Required field presence
- Value type correctness
- File path existence (certificates, keys)
- Network address format validity

### Exit Codes

- `0`: Configuration validated successfully, normal operation
- `1`: Configuration validation failed or runtime error occurred

### Validation Output

**Valid Configuration**:
```
Configuration validation successful
Spooky startup phase=begin
Spooky listener topology listeners=1 packet_shards_per_worker=1 reuseport=true pin_workers=false
Listener 0 binds udp=0.0.0.0:9889 tcp_bootstrap=0.0.0.0:9889
```

**Invalid Configuration**:
```
Error loading config: <error details>
```

or

```
Configuration validation failed. Exiting...
```

## Error Codes

### HTTP Status Codes

Spooky may return the following HTTP status codes to clients:

- `200 OK`: Request successful (forwarded from backend)
- `400 Bad Request`: Malformed or invalid request
- `500 Internal Server Error`: Internal proxy error (e.g., TLS configuration issues)
- `502 Bad Gateway`: Backend server error
- `503 Service Unavailable`: Backend timeout, no healthy backends available, or upstream response body exceeds `max_response_body_bytes`

## Logging

### Log Format

Spooky uses the `env_logger` logging implementation with timestamped output. All log messages are written to standard output (stdout) with the following format:

```
[YYYY-MM-DD HH:MM:SS] [LEVEL] [module::path] message
```

### Log Output Examples

```
[2026-02-18 14:23:45] [INFO] [spooky::listener_group] Spooky startup phase=begin
[2026-02-18 14:23:45] [INFO] [spooky::listener_group] Spooky listener topology listeners=1 packet_shards_per_worker=1 reuseport=true pin_workers=false
[2026-02-18 14:23:45] [INFO] [spooky::listener_group] Listener 0 binds udp=0.0.0.0:9889 tcp_bootstrap=0.0.0.0:9889
[2026-02-18 14:23:45] [INFO] [spooky_edge::quic_listener] Runtime performance concurrency worker_threads=1 control_plane_threads=2 packet_shards_per_worker=1 reuseport=true pin_workers=false
[2026-02-18 14:23:45] [DEBUG] [spooky_edge::quic_listener] Certificate loaded successfully
[2026-02-18 14:23:50] [INFO] [spooky_edge::quic_listener] Length of data received: 1200
[2026-02-18 14:23:50] [DEBUG] [spooky_edge::quic_listener] Packet DCID (len=8): [00 01 02 03 04 05 06 07], type: Initial, active connections: 1
[2026-02-18 14:25:30] [INFO] [spooky_edge::quic_listener] Draining connections
[2026-02-18 14:25:35] [INFO] [spooky] Spooky shutdown complete
```

### Log Levels

Log verbosity is configured via the `log.level` configuration parameter. The following levels are available (ordered from most to least verbose):

| Level | Standard Equivalent | Use Case |
|-------|-------------------|----------|
| `whisper` | trace | Extremely detailed diagnostic information including packet hex dumps |
| `haunt` | debug | Detailed diagnostic information for troubleshooting |
| `spooky` | info | General informational messages about normal operation |
| `scream` | warn | Warning messages for potentially problematic situations |
| `poltergeist` | error | Error messages for failures and exceptions |
| `silence` | off | Disable all logging output |

Standard log level names (`trace`, `debug`, `info`, `warn`, `error`, `off`) are also supported for compatibility.

### Log Configuration

Configure logging in the configuration file:

```yaml
log:
  level: "spooky"  # or "haunt", "whisper", etc.
```

### Environment Variable Control

The `env_logger` implementation respects the `RUST_LOG` environment variable, which can be used to override configuration file settings or enable module-specific logging:

```bash
# Override global log level
RUST_LOG=debug spooky --config config.yaml

# Enable debug logging for specific modules
RUST_LOG=spooky_edge=debug,spooky_transport=info spooky --config config.yaml

# Trace all modules
RUST_LOG=trace spooky --config config.yaml
```

## Environment Variables

**Note**: Environment variable interpolation in configuration files is not currently supported. Configuration values must be provided literally in the YAML file.

For dynamic configuration, consider using external configuration management tools or templating the configuration file before loading.
