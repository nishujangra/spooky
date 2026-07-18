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
- a control API for liveness, readiness, runtime visibility, full config reload, cert reload, and restart actions

Use:

- [Metrics Reference](../reference/metrics-reference.md) for current metric families and first-alert guidance
- [Control API Reference](../reference/control-api-reference.md) for current endpoint behavior and security posture

## Configuration Surface

The canonical configuration docs live in:

- [Configuration Reference](../configuration/reference.md)
- [Configuration Examples](../configuration/examples.md)
- [TLS Setup](../configuration/tls.md)

## Important Scope Note

The control API applies configuration through a file-reload model, not a granular per-object API.

- it provides health, readiness, runtime inspection, restart actions, certificate reload, and full
  config reload (`POST /admin/runtime/reload`)
- config reload re-reads the config file and applies it live via an atomic runtime swap, including
  route, upstream, and backend changes — it is not a per-object mutation API (you edit the file and
  reload), and startup-owned settings and listener bind/removal changes still require a restart

## Related Pages

- [Metrics Reference](../reference/metrics-reference.md)
- [Control API Reference](../reference/control-api-reference.md)
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
