# Operations Runbook

This page is the operator quick-reference for common production changes and high-signal failure scenarios.

## Before You Touch Production

Confirm all of the following:

- rollback target is known
- binary rollback path is known
- you know whether the change is runtime-managed, cert-only, or restart-required
- Control API, metrics, and logs are reachable from the operator path
- a traffic-reduction or instance-removal plan is ready

## Standard Change Workflows

### Runtime-managed config change

Use this for routes, upstreams, backends, timeouts, resilience, quota, and similar live-reloadable runtime domains.

1. Render the candidate config.
2. `POST /admin/runtime/validate`
3. `POST /admin/runtime/preview`
4. `POST /admin/runtime/activate`
5. Verify the active generation and runtime history.
6. Watch latency, backend health, overload, quota, and auth outcomes before expanding rollout.

### Certificate-only change (listener)

1. Write new cert material with correct permissions.
2. Verify expiry and SAN coverage.
3. `POST /admin/runtime/reload-certs`
4. Confirm new handshakes present the new certificate.
5. Keep the previous cert material until verification is complete.

### Upstream client certificate or CA rotation

Upstream mTLS client cert/key and upstream CA changes are generation-owned, not `reload-certs`-scoped. Use the runtime-managed config change workflow above (`validate` → `preview` → `activate`). See [Secret and Certificate Rotation](secret-and-cert-rotation.md) for the full flow, overlap strategy for CA transitions, and rollback caveats.

### Restart-required change or binary upgrade

1. Remove or drain one node from traffic.
2. Apply the change or replace the binary.
3. Restart the node.
4. Verify health, readiness, metrics, and key dashboards.
5. Reintroduce the node only after it is stable.
6. Repeat node by node or slice by slice.

## Scenario: Rising 503 Rate

Check:

- `spooky_overload_shed_by_reason_total`
- quota backend health and quota outcome series
- route latency and backend timeout signals
- active connections and inflight pressure
- recent config, backend, or deploy changes

Likely causes:

- overload self-protection
- backend timeout surge
- temporary backend unavailability
- fail-closed quota backend failure

Immediate actions:

1. Decide whether the 503s are overload, quota-backend, or upstream-failure related.
2. Reduce demand or remove non-critical traffic first.
3. Verify backend health and recent latency changes.
4. Roll back the most recent risky change if the spike correlates strongly with it.

## Scenario: Rising 429 Rate

Check:

- quota policy outcome metrics
- matched policy identifiers
- quota backend health
- recent traffic-shape changes by tenant, token, client, or route

Interpretation:

- this is contract enforcement, not overload
- do not widen inflight or brownout settings to fix a quota-contract problem

Immediate actions:

1. Confirm the affected selector dimensions.
2. Confirm whether burst or sustained windows are being exhausted.
3. Verify whether the rise is expected traffic growth, abuse, or mis-sized quota policy.

## Scenario: Handshake Failures Or Client Connection Failures

Check:

- downstream TLS and handshake metrics
- certificate-expiry and certificate-selection signals
- ALPN-related behavior
- listener certificate paths and permissions

Likely causes:

- expired or wrong certificate material
- wrong hostname coverage
- missing client certificate where required
- client protocol mismatch

Immediate actions:

1. Verify the listener presents the expected certificate.
2. Determine whether the failures are isolated to one listener or hostname.
3. If only cert material changed, use cert reload.
4. If the issue involves listener bind or startup-owned changes, use the restart path.

## Scenario: Backend Timeout Surge

Check:

- route latency percentiles
- backend timeout counters
- backend health transitions
- per-upstream and per-backend inflight pressure

Likely causes:

- unhealthy backend pool
- backend latency regression
- network or TLS establishment issues
- too much concurrency against a weak backend tier

Immediate actions:

1. Confirm whether the issue is local to one upstream or broad.
2. Remove or isolate obviously failing backends if health signals are clear.
3. Reduce concurrency pressure if the proxy is amplifying backend collapse.
4. Roll back recent backend or network changes before widening limits.

## Scenario: Control API Or Metrics Endpoint Unavailable

Check:

- bind address and port settings
- local firewall rules
- startup logs
- whether the endpoints are required in the config

Immediate actions:

1. Determine whether the data plane is healthy but the admin plane is down.
2. If the admin surface is required and failed to bind, treat that as intentional startup protection.
3. If the admin surface is optional and unavailable, decide whether to restart into a safer posture.

## Scenario: JWT Rejections Or Stale JWKS State

Find out whether the problem is the tokens or the key source:

1. Read `GET /admin/runtime` and inspect the JWT and JWKS state.
2. Check cache freshness and refresh timestamps.
3. Read the dominant JWT validation failure reasons.
4. Confirm whether the issue is a rotated `kid`, stale JWKS state, issuer mismatch, or token expiry.

What common reasons usually mean:

| Reason | Meaning |
|---|---|
| `key_source_unavailable` | no usable verification key source |
| `missing_verification_key` | token `kid` not present in the cached key set |
| `algorithm_not_allowed` | token algorithm not allowed by config |
| `issuer_mismatch` or `audience_mismatch` | token valid for some other audience or issuer |
| `token_expired` | normal token expiry |

Recovery:

- wait one refresh interval if an issuer rotated early
- verify HTTPS reachability to the JWKS endpoint from the proxy host
- treat `empty_unusable` as an auth outage for that upstream
- avoid relying on restart as the primary fix for an upstream issuer outage

## Scenario: Brownout Or Overload Triggering

Check:

- overload shed counters by reason
- brownout activation state
- active connections
- inflight pressure versus configured caps

Actions:

1. Confirm whether self-protection is behaving correctly.
2. Preserve core traffic first.
3. Reduce demand or add backend capacity before simply widening limits.
4. Avoid increasing caps blindly without memory and tail-latency validation.

## Scenario: Drain For Deploy Or Maintenance

1. Stop routing new traffic to the node if your traffic manager supports it.
2. Use the drain-aware restart workflow.
3. Watch drain progress and readiness.
4. Use the configured forced-drain timeout only as a safety boundary.

If drain repeatedly times out:

- inspect long-lived streams
- inspect shutdown drain timeout
- inspect watchdog drain-grace configuration
- inspect whether traffic removal from the upstream load balancer is happening soon enough

## After Any Incident

Record:

- the first signal that surfaced the issue
- whether the proxy was the root cause or a reflector of backend failure
- what changed immediately beforehand
- whether the failure was quota, overload, auth, transport, or upstream application behavior
- what alert, dashboard, or runbook step should be tightened before the next occurrence
