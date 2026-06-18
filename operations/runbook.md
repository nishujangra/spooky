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
