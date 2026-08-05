# Operations Runbook

This page is the operator quick-reference for common high-signal failure modes and maintenance actions.

## Before You Touch Production

- keep a tested rollback path
- know whether the change requires cert reload or drain-and-restart
- know where metrics, logs, and control API endpoints are exposed
- keep a traffic-reduction plan ready before making invasive changes

## Scenario: Rising 503 Rate

Check:

- `spooky_overload_shed_by_reason_total`
- route latency metrics
- active connections and inflight pressure
- backend health state
- recent config or backend changes

Likely causes:

- global or scoped inflight limits reached
- route queue cap exceeded
- upstream or backend overload
- backend timeout surge

Immediate actions:

1. Determine whether the 503s are overload-generated or upstream-generated.
2. Reduce traffic or shed non-critical traffic first.
3. Verify backend health and recent latency changes.
4. Roll back the most recent risky config change if the spike correlates with change timing.

## Scenario: Handshake Failures Or Client Connection Failures

Check:

- downstream TLS metrics
- ALPN selection metrics
- certificate expiry/selection metrics
- listener cert/key presence and permissions

Likely causes:

- invalid or expired certificate material
- wrong SNI mapping
- missing client certificate in required-client-cert mode
- client-side protocol mismatch

Immediate actions:

1. Verify the listener is presenting the expected certificate.
2. Verify whether failures are concentrated on one hostname or all hostnames.
3. If only certificate material changed, use the certificate reload path when appropriate.
4. If listener routing or policy changed, prefer drain-and-restart with rollback readiness.

## Scenario: Backend Timeout Surge

Check:

- route latency percentiles
- backend timeout counters
- backend health transitions
- per-upstream and per-backend inflight pressure

Likely causes:

- unhealthy backend pool
- sudden backend latency regression
- connection establishment failures
- under-sized backend fleet

Immediate actions:

1. Confirm whether the issue is localized to one upstream or all traffic.
2. Remove or isolate failing backends if health signals are clear.
3. Reduce concurrency pressure if the proxy is amplifying backend collapse.
4. Roll back recent backend or network changes first, not just proxy config.

## Scenario: Control API Or Metrics Endpoint Unavailable

Check:

- bind address and port config
- local firewall rules
- listener startup logs
- whether endpoints are configured as required or optional

Immediate actions:

1. Confirm whether the process is healthy but only the admin plane is down.
2. If admin endpoints are `required: true`, treat startup failure as intentional protection.
3. If admin endpoints are `required: false`, decide whether to fail closed operationally and restart into a safer config.

## Scenario: Cert Rotation

Safe approach:

1. Place new cert and key material with correct permissions.
2. Validate hostname coverage and expiry before activation.
3. Use certificate reload for listener cert replacement.
4. Verify new handshakes present the new certificate.
5. Keep previous material until verification is complete.

## Scenario: Route Or Upstream Change

Current operational model:

- certificate-only changes can use cert reload
- route, upstream, timeout, and policy changes should be treated as drain-and-restart changes

Recommended sequence:

1. Validate config offline.
2. Stage on a canary node or bounded traffic slice.
3. Drain and restart one instance at a time.
4. Watch error rate, route latency, health transitions, and shed counters.
5. Expand only after the canary stays stable.

## Scenario: JWT Rejections Or Stale JWKS State

First, find out whether the problem is the tokens or the keys:

1. Check `GET /admin/runtime` and read `jwks.sources[]` and `auth.providers[]`.
2. Read `cache_state`. `fresh` or `stale` means keys are loading; `empty_unusable`
   means no usable keys and every JWT request is being rejected.
3. Compare `last_refresh_attempt_unix_seconds` with `last_refresh_success_unix_seconds`.
   A recent attempt with an old success means refreshes are failing — read
   `last_failure_reason`.
4. Read `auth.jwt_validation_failures[]` for the dominant rejection reason.

What the common reasons mean:

| Reason | Cause |
| --- | --- |
| `key_source_unavailable` | JWKS never loaded or aged past the staleness window |
| `missing_verification_key` | Token's `kid` is not in the cached set; check whether the issuer rotated early |
| `algorithm_not_allowed` | Token `alg` is outside `allowed_algorithms` |
| `issuer_mismatch` / `audience_mismatch` | Token is valid but issued for a different issuer or audience |
| `token_expired` | Ordinary client-side expiry, not a server problem |
| `ambiguous_verification_key` | Two configured keys resolve to the same `kid` |

Recovery:

- if the issuer rotated early, an unknown `kid` already triggers a rate-limited
  background refresh — wait one refresh interval before intervening
- if refreshes are failing, verify the endpoint is reachable over HTTPS from the
  proxy host; keys keep validating until `jwks_stale_if_error_secs` elapses
- if state is `empty_unusable`, treat it as an auth outage for that upstream
- avoid restarting a node with `require_ready` while the issuer is down: it will
  fail to boot rather than start degraded

### Rotation Cadence And Refresh Intervals

- keep `jwks_refresh_interval_secs` (default `300`) well below the issuer's rotation
  interval so new keys are cached before tokens signed with them arrive
- keep `jwks_cache_ttl_secs` (default `900`) at roughly three refresh intervals so a
  single failed refresh does not immediately mark the set stale
- `jwks_stale_if_error_secs` (default `3600`) is the outage budget: how long
  last-known-good keys keep working while refreshes fail. It also sets the overlap
  window during which a key dropped from the JWKS stays valid
- overlap old and new keys in the published JWKS for at least one full
  `jwks_cache_ttl_secs` when rotating

### Alerts Worth Adding

- `spooky_jwks_state{state="empty_unusable"} == 1` — auth outage for that upstream, page
- `spooky_jwks_age_seconds` above `jwks_cache_ttl_secs` — refreshes are not landing
- `rate(spooky_jwks_refresh_failure_total[15m]) > 0` sustained — issuer or network problem
- `rate(spooky_jwks_unknown_kid_total[5m])` rising — likely an unannounced rotation
- `rate(spooky_jwt_validation_failures_total{reason="key_source_unavailable"}[5m]) > 0` —
  requests are being rejected for key-availability reasons rather than bad tokens

## Scenario: Brownout Or Overload Triggering

Check:

- overload shed counters by reason
- brownout state transitions
- active connections
- inflight metrics versus configured caps

Actions:

1. Confirm whether the system is protecting itself correctly rather than failing unexpectedly.
2. Preserve core traffic first.
3. Reduce demand or increase backend capacity before simply widening limits.
4. Avoid increasing caps blindly without memory and latency validation.

## Scenario: Draining For Deploy Or Maintenance

1. Stop sending new traffic to the instance.
2. Trigger drain-aware restart workflow.
3. Watch for completion before hard termination whenever possible.
4. Use the configured forced-drain timeout only as a safety boundary, not as the primary shutdown path.

## After Any Incident

- record what metric or symptom first signaled the issue
- record whether the proxy was the root cause or the reflector of backend failure
- record what config or dependency changed
- add or tighten alerts and runbook steps for the same class of issue
