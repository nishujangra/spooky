# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0-beta] - 2026-07-27

### Added

- Canonical observability vocabulary — reasons and event fields (admission outcomes, local reason enums, overload metric labels, backend health-failure reasons) now map to a single canonical set, with bounded connect-attempt label cardinality.
- Deterministic runtime lifecycle state machine that gates reload commits, driving drain and shutdown through explicit lifecycle states.
- Unified backend refresh classification with explicit traffic-continuity reporting, and canonical backend health-failure reason surfaced in the control-plane snapshot.
- Structured operator-facing reload rejections with preflight messages, backed by a typed operational-policy vocabulary for runtime ownership and reload rules.
- Live `log.level` changes applied on config reload via a runtime `set_log_level` on the logger.
- Fine-grained rejection and backend-failure kinds carried through terminal outcome mapping; bootstrap terminal outcomes observed and aligned with the request lifecycle model.

### Fixed

- Drain and shutdown now flow through the runtime lifecycle state machine instead of ad-hoc paths.
- Process-shared watchdog and DNS resolver are carried across reload instead of being rebuilt.
- Bundle-lock panic and control-API unreachable paths replaced with fail-safe recovery; backend client-rotation failure is now an explicit operator-visible outcome.
- Pre-response requests still reading the body now time out under the request-body bucket.
- Restored stateful round-robin backend selection, failover availability on circuit-open retries, and streaming guardrail failure semantics.
- WebSocket upgrade headers preserved in bootstrap `101` responses.
- Upstreams iterated in name order so route conflicts report deterministically.

### Changed

- Public API surface locked down across crates — crate-internal items demoted, `unreachable_pub` enabled, dead code removed, and public runtime bundle/listener/connection surfaces documented.
- Documentation realigned to the post-refactor architecture: edge runtime ownership, runtime generation and backend lifecycle, request lifecycle and transport boundaries, observability, control plane, and reload behavior.
- Integration and unit tests hardened with per-contract cases asserting QUIC/bootstrap parity, metric label vocabularies, retry/hedge accounting, and wire-shape stability.

## [0.3.1-beta] - 2026-06-27

### Added

- Full config hot reload via `POST /admin/runtime/reload` — atomically swaps the runtime bundle without restarting the process or dropping connections.
- Listener group reconciliation on reload — new listener groups are started and removed groups are retired gracefully per the incoming config.
- Live admin endpoint rebinding — control API and metrics endpoint addresses are updated in place on reload without requiring a restart.
- `RuntimeTaskRegistry` — generation-aware background task tracking; retired tasks drain on a configurable timeout when a new generation is activated.

### Fixed

- Hot reload now correctly rejects configs that remove a listener or change its bind address, returning `409 Conflict`.
- Hot reload now correctly rejects changes to startup-owned settings (log level, thread counts, listen address), returning `409 Conflict`.
- Control API and cert reload endpoints now target the live runtime bundle after a hot reload instead of the original startup bundle.
- Metrics endpoint, bootstrap listener, and control API settings are now refreshed from the live runtime bundle on each reload.

### Changed

- Large runtime modules split into focused submodules: `quic_listener/mod.rs` → `bootstrap_tls`, `control_api/`, `forwarding`, `metrics_endpoint`, `tls_runtime`; `spooky/main.rs` → `app`, `listener_group`; `config/validator.rs` → `helpers`; `config/runtime.rs` → `listeners`, `upstreams`.
- Integration tests extracted into per-subsystem modules (`h3_edge/`, `h3_bridge/protocol`, `lb/tests`, `control_api/tests`).

## [0.3.0-beta] - 2026-06-20

### Added

- HTTP/1.1 upstream transport — `http://` backends are now forwarded over a pooled HTTP/1.1 connection via new `H1Client` and `H1Pool` primitives.
- Scheme-aware dispatch in the data plane — backend scheme determines transport: `https://` uses HTTP/2, `http://` uses HTTP/1.1.
- Mixed HTTP/1.1 and HTTP/2 backend deployments supported within the same upstream pool.
- DNS refresh client rotation wired through `UpstreamTransportPool` for H1 backends, matching existing H2 behavior.
- Health checks routed through the scheme-aware transport pool — `http://` backends are now probed over HTTP/1.1.
- `TE: trailers` header added to H1 upstream requests to preserve trailer forwarding semantics.

### Fixed

- Config validator no longer applies TLS trust-store checks to `http://` upstreams — HTTP-only configs boot without requiring CA paths or TLS material.

### Changed

- Upstream transport layer unified under `UpstreamTransportPool`, which dispatches by `BackendTransportKind` (`Http1` or `H2`).

## [0.2.1-beta] - 2026-06-20

### Fixed

- `ProxyError::Pool` displayed as `"transport error: ..."` — disambiguated to `"pool error: ..."` so pool and transport errors have distinct display text in logs.
- Watchdog mutex poison is now logged and recovered instead of silently causing the coordinator to skip state updates — watchdog restart logic remains operational after a worker panic.
- OTLP tracing endpoint is now configurable via `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` or `OTEL_EXPORTER_OTLP_ENDPOINT` environment variables, with the resolved source logged at startup alongside the endpoint.
- `validate()` now returns a structured `ValidationError` instead of `bool`, making the first validation failure available to callers as a typed error value.
- `take_validation_error` clears the slot on read, preventing stale validation errors from leaking across test cases.
- Logger fallback error messages now include both the file path and directory path when log directory creation fails.

### Changed

- `validate_config` call site updated to handle the new `Result` return type from the validator.

## [0.2.0-beta] - 2026-06-18

### Added

**TLS**
- Live listener certificate reload — certificates can be reloaded without restarting the process.
- SNI-based server certificate selection with fallback for listener TLS, plus native QUIC SNI cert selection.
- Per-upstream TLS policy override — each upstream can now specify its own TLS settings independent of the global config.
- Certificate expiry telemetry — expiry timestamps exposed for monitoring.
- Downstream handshake telemetry — TLS handshake metrics on the listener path.
- Upstream TLS failure classification — failures are now categorized (cert, SNI mismatch, timeout, etc.) in logs and metrics.

**Routing**
- Wildcard host pattern matching in the route index with correct precedence over exact-host routes.
- Multi-listener support — independent listener worker groups can be spawned per listener entry.

**Forwarding**
- CONNECT tunnel support — HTTP CONNECT requests are validated, translated, and lifecycle-managed over H3.
- Response trailer forwarding over H3 — upstream response trailers are now relayed to the downstream client.
- Configurable `X-Forwarded-For` policy — choose append vs. overwrite semantics per deployment.
- Configurable Host header forwarding policy — preserve the original downstream `Host` or rewrite to the upstream authority.

**DNS**
- Periodic backend DNS refresh loop — backends are re-resolved at runtime without restart.
- Shared DNS resolver cache with atomic update semantics.
- Backend DNS refresh configuration (`performance.backend_dns_refresh_enabled`, `performance.backend_dns_refresh_interval_ms`).
- Backend connect and rotation telemetry — metrics for DNS refresh events, connection rotation, and backend selection.

**Config**
- Canonical runtime config model — a normalized intermediate config representation validated before startup.
- Cross-field normalization checks enforced at startup with classified error types.
- Runtime startup drives from the normalized config rather than raw YAML structs.

**Load Harness**
- H3 client timeout retry and reconnect controls.
- Config-gated inflight admission micro-wait.
- Matrix profile override and selection knobs.
- Improved worker model and ramp handling in load scenarios.

### Fixed

- `H2Client::default()` panicked on invalid TLS config — default construction no longer panics.
- Bootstrap response streaming lacked a running body-size cap — body size is now enforced incrementally.
- Hop-by-hop headers were not stripped from bootstrap responses.
- Ambiguous route conflicts (overlapping prefix + host combinations) are now rejected at startup.
- Backend hostnames are now validated more strictly at config load time.
- Upstream send/connect failures are classified into backend health states instead of being silently swallowed.
- SNI certificate hostnames containing whitespace are now rejected at config load.
- Route decision reasons were unstable for wildcard and trie-level routes.
- Authority/host normalization adapted to avoid unnecessary allocations on the hot path.
- Insecure upstream TLS (`verify_certificates: false`) now always emits a startup warning log.

### Changed

- Bootstrap forwarding policy unified into a single code path.
- Route precedence decisions made explicit in the routing layer.
- Backend health identity made explicit — health checks align with live backend resolution.
- Pooled clients are rotated when backend DNS changes are detected.
- Listener TLS material loading centralized.

## [0.1.1-beta] - 2026-05-28

### Added
- `upstream_tls.verify_certificates: false` — new config option to disable upstream TLS certificate verification, useful for backends with self-signed certs in development or trusted internal environments. Matches the opt-out behavior of Nginx (`proxy_ssl_verify off`) and Envoy (`ACCEPT_UNTRUSTED`). A warning is logged at startup when disabled.

### Fixed
- Upstream send errors now log the full error cause chain instead of the opaque `client error (Connect)`, making TLS failures (missing SAN, untrusted root, cert/SNI mismatch) immediately diagnosable from logs without requiring a packet trace.
- Validator no longer hard-rejects `upstream_tls.verify_certificates=false`; it now emits a warning and allows startup to continue.
- Debian package and systemd unit: TLS certificate files must be owned `root:spooky` with mode `640` so the `spooky` service user can read them. Documentation and all installation examples corrected accordingly.

### Changed
- Packaging layout cleanup: Debian assets moved under `packaging/deb/` (`make-deb.sh`, systemd unit, default config).
- Installation and Docker docs updated to match current packaging paths and runtime behavior.
- Debian package version bumped to `0.1.1-beta`.

## [0.1.0-beta] - 2026-05-12

Initial release of Spooky HTTP/3 edge proxy and load balancer.

### Core Features

**Protocol Support**
- HTTP/3 termination using quiche (RFC 9114)
- QUIC transport (RFC 9000)
- HTTP/2 backend connectivity (RFC 9113)
- TLS 1.3 with certificate chain loading (RFC 8446)
- TLS bootstrap ingress for HTTP/1.1 + HTTP/2 compatibility and Alt-Svc upgrade flow

**Routing and Load Balancing**
- Upstream pool architecture with per-upstream configuration
- Route matching based on path prefix and host headers with longest-match selection
- Method-aware route matching support (`route.method`)
- Multiple load balancing algorithms: random, round-robin, consistent hashing (64 replicas), least-connections, latency-aware, sticky-cid
- Configurable load-balancing key sourcing (`load_balancing.key`)
- Backend weight configuration for weighted load balancing

**Health Management**
- Active health checking with HTTP probes
- Configurable interval, timeout, failure/success thresholds, and cooldown
- Automatic backend removal and recovery

**Connection Management**
- Connection ID-based routing for QUIC packets
- Prefix matching for Short packets with extended DCIDs
- Peer-based fallback for connection migration scenarios
- Version negotiation packet handling
- Proper 0-RTT handling to prevent crypto failures
- Config-driven graceful shutdown drain timeout

**Ingress and Resilience**
- Sharded ingress dispatch — per-worker UDP sockets for parallel packet processing
- Global route-queue cap with `503 + Retry-After` shedding under overload
- LB fallback, health probe, and streaming timeout semantics
- Panic handling hardened for worker and control-plane tasks

**Bootstrap (HTTP/1.1 + HTTP/2 TCP Path)**
- Dual ingress: QUIC/HTTP3 and TCP/TLS bootstrap for browser compatibility
- Bootstrap path enforces LB strategy and health-aware backend resolution (parity with QUIC path)
- Bootstrap route-resolution parity with QUIC path (host/path/method decision flow)
- Bootstrap path enforces QUIC request policy pipeline (method/path/header policies)
- Bootstrap path enforces downstream mTLS policy
- Bootstrap header sanitization and RFC 7239-compliant IPv6 normalization in `Forwarded`
- Bootstrap connection limiter and per-connection timeout guard
- Bootstrap backend request/response streaming support with deterministic unsupported-upgrade behavior for WebSocket

**Configuration**
- YAML-based configuration with comprehensive validation at startup
- Per-upstream load balancing strategy and embedded routing rules
- Packet shard ingress controls
- `performance.control_plane_threads` applied to startup runtime configuration
- Upstream TLS verification enforced by default; cleartext backends require explicit opt-in
- Downstream mTLS support via `listen.tls.client_auth.*`

**Observability**
- Structured JSON logging with standard and spooky-themed log levels
- Request/response metrics: total requests, successes, failures, timeouts
- Backend selection and health transition logging
- QUIC connection error classification and deduplication

**Control API**
- Bearer token authentication with constant-time comparison
- Metrics endpoint, health and ready probes
- TLS-enabled control-plane listener path support and startup hardening

**Operational**
- Debian package with systemd unit, system user/group, and config at `/etc/spooky/config.yaml`
- Docker packaging with compose bootstrap and operator smoke-test scripts
- CLI with `--config` flag
- Streaming request/response handling with bounded queues and hard body caps
- Deterministic cap-breach behavior via HTTP errors (`413`/`503`) under pressure
- Concurrent connection handling (10,000+ connections tested)
- Per-backend concurrency limiting (64 max in-flight requests)

### Known Limitations

1. No dynamic backend discovery (service discovery remains static config-driven).
2. No configuration hot reload (restart-based config apply model).
3. Project is pre-GA and still requires extended soak/failure-mode hardening for broad production rollout.

See [roadmap](docs/roadmap.md) for planned improvements.

---

## Version Numbering

- **Major version** (X.0.0): Breaking changes to configuration or API
- **Minor version** (0.X.0): New features, non-breaking changes
- **Patch version** (0.0.X): Bug fixes, documentation updates

## Contributing

See [contributing guide](CONTRIBUTING.md) for development guidelines.

## License

GNU General Public License v3.0 (GPLv3) — see [LICENSE.md](LICENSE.md)
