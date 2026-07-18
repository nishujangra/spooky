# Deployment Validation for Spooky

This guide covers how to validate a Spooky configuration change or binary upgrade before it touches production traffic. Target audience: SREs and platform engineers preparing a deployment of Spooky v0.1.1-beta or later.

The goal is to catch problems at each stage of deployment, not after the restart.

---

## Config Validation Before Restart

Spooky validates its configuration at startup and exits with a non-zero code if the config is invalid. This gives you a free dry-run: start the process against the new config on a non-production host (or in a pre-deploy step on the same host with a different port), watch for the startup log line, then stop it.

```bash
spooky --config /etc/spooky/config-new.yaml
```

If the process reaches a log line matching `listening on ...`, the configuration is structurally valid and all externally referenced files (TLS certificates, key files, CA bundles) were found and could be opened. At that point, stop the process immediately:

```
Ctrl-C
```

You are not doing a real deployment yet. This is purely a parse-and-load check.

**What startup validation catches:**

- YAML schema errors (unknown keys, wrong types, missing required fields)
- Missing or unreadable certificate and key files
- Syntactically invalid listener addresses (bad IP, out-of-range port)
- Duplicate upstream pool names (Spooky rejects ambiguous configs)
- Invalid field values (e.g., a timeout expressed as a negative integer, an unsupported TLS version string)

**What startup validation does NOT catch:**

- **Backend reachability.** Spooky probes backends after startup, not during config parsing. A config that lists unreachable backends will pass startup validation cleanly.
- **Certificate expiry.** Spooky loads the certificate file and checks that it is parseable PEM; it does not validate `notAfter`. An expired cert will load without error.
- **Behavioral correctness.** A valid config can still produce wrong routing. Overlapping route prefixes, a backend pointed at the wrong port, or a timeout set to an unintended value all pass validation.

Run this check in CI on every config change. It is fast (sub-second) and catches the majority of deployment-blocking errors before any process restarts.

---

## Verifying Backend Reachability

After starting Spooky against the new config (in a staging environment, or as a canary instance before routing real traffic), verify that all upstream backends have been probed and are healthy before you shift traffic to the instance.

### 1. Poll /admin/runtime

The `/admin/runtime` endpoint returns the current runtime state of every upstream pool, including per-backend health status. Query it within the first 30 seconds of startup:

```bash
curl -sk https://127.0.0.1:9902/admin/runtime | jq .
```

A healthy response looks like this:

```json
{
  "upstreams": {
    "api-pool": {
      "backends": [
        {
          "address": "10.0.1.10:8080",
          "healthy": true,
          "consecutive_failures": 0,
          "last_probe_ms": 4
        },
        {
          "address": "10.0.1.11:8080",
          "healthy": true,
          "consecutive_failures": 0,
          "last_probe_ms": 6
        }
      ],
      "healthy_count": 2,
      "total_count": 2
    },
    "static-pool": {
      "backends": [
        {
          "address": "10.0.2.20:80",
          "healthy": true,
          "consecutive_failures": 0,
          "last_probe_ms": 3
        }
      ],
      "healthy_count": 1,
      "total_count": 1
    }
  }
}
```

Before routing traffic, every backend should show `"healthy": true` and `"consecutive_failures": 0`. If any backend shows `healthy: false`, do not route traffic to this instance until you understand why.

A pool where `healthy_count` is less than `total_count` indicates partial degradation. Spooky will route to the remaining healthy backends, but the pool is operating below capacity. Determine whether that is acceptable before proceeding.

### 2. Watch health check logs at debug level

During the first 30 seconds after startup, run Spooky at debug log level (or tail its logs if already running with structured output) and watch for health probe results. The log level is set in the config file via `log.level: debug` (there is no `SPOOKY_LOG`/`RUST_LOG` environment variable):

```bash
# set `log.level: debug` in config-new.yaml, then:
spooky --config /etc/spooky/config-new.yaml 2>&1 | grep -i "health\|probe\|backend"
```

You are looking for probe success messages for every backend. Any repeated probe failure for a backend that should be reachable is a signal to stop and investigate before the instance takes traffic.

### 3. Manual backend health check

If `/admin/runtime` shows a backend as unhealthy, verify reachability independently from the same host. This rules out Spooky misconfiguration versus a genuinely unreachable backend:

```bash
# Replace with your backend's actual health check path
curl -v http://10.0.1.10:8080/healthz
```

If this also fails, the problem is backend availability or network, not the Spooky config. If this succeeds but `/admin/runtime` still shows the backend as unhealthy, the issue is in Spooky's probe configuration (wrong path, wrong timeout, TLS mismatch on the backend connection).

---

## Pre-Deploy Checklist for Config Changes

Run through this checklist for every config change before restarting Spooky in production. Each item is phrased as what to verify and how to verify it.

**1. Run startup validation against the new config on a non-production host.**

```bash
spooky --config /etc/spooky/config-new.yaml
# Wait for "listening on ..." log line, then Ctrl-C
echo "Exit code: $?"
# Expected: 130 (Ctrl-C SIGINT), not 1 or 2
```

If the process exits with code 1 or 2 before the listening line, the config is invalid. Read the error output carefully — Spooky emits the field path that caused the failure.

**2. Diff the config change and confirm each difference is intentional.**

```bash
diff /etc/spooky/config.yaml /etc/spooky/config-new.yaml
```

Review every line that changed. Common sources of unintended changes: YAML serializers that reorder keys, editor auto-formatting that changes indentation, copy-paste errors in backend addresses or timeout values. If you cannot account for every line in the diff, do not deploy.

**3. Verify TLS certificate paths exist and are readable by the spooky user.**

For each `tls.cert` and `tls.key` path in the new config:

```bash
ls -la /etc/spooky/certs/example.com.crt
ls -la /etc/spooky/certs/example.com.key
```

Confirm the file owner and permissions allow the `spooky` system user to read them. A common issue is a cert renewal that writes new files with root-only permissions.

Also confirm cert validity dates have not already expired:

```bash
openssl x509 -in /etc/spooky/certs/example.com.crt -noout -dates
```

**4. If backends changed: confirm new backends answer the configured health check path before routing traffic.**

For each new or modified backend address:

```bash
curl -f http://<new-backend-address>:<port>/<health-check-path>
```

Do this from the host that will run Spooky, not from your workstation. Network path, DNS resolution, and firewall rules may differ.

**5. If upstream pools changed: confirm route prefix overlaps are intentional.**

Spooky uses longest-prefix matching: a request to `/api/v2/users` will match a route for `/api/v2/` before a route for `/api/`. This is usually correct, but misconfiguration is easy.

List all route prefixes in the new config and sort them:

```bash
grep -E '^\s+prefix:' /etc/spooky/config-new.yaml | awk '{print $2}' | sort
```

Look for cases where one prefix is a prefix of another and verify the routing intent is correct. If two prefixes are identical, Spooky will reject the config at startup. If a shorter prefix is unintentionally catching traffic meant for a longer one, the config is valid but behaviorally wrong — startup validation will not catch this.

**6. Confirm the control API is bound to loopback in the new config.**

The `/admin/runtime` endpoint exposes internal runtime state. It must not be accessible from the network. Verify the control API listen address:

```bash
grep -A5 'control_api\|admin' /etc/spooky/config-new.yaml
```

The bind address must be `127.0.0.1` or `[::1]`, never `0.0.0.0` or `::`. If it is bound to all interfaces, the endpoint is publicly reachable.

**7. Confirm the metrics endpoint is accessible from your monitoring system before restarting.**

If the metrics endpoint address or port changed in the new config, verify that your Prometheus scrape target can reach it before you restart the production instance. A missed scrape during a deploy is recoverable; a missed scrape for 30 minutes while you debug firewall rules is not.

From the Prometheus host or scrape node:

```bash
curl -s http://<spooky-host>:<metrics-port>/metrics | head -5
```

Expected output begins with `# HELP` lines. If this fails, correct the network path or adjust the bind address before deploying.

---

## Canary Rollout Procedure

For high-risk config changes (new upstream pools, changed TLS configuration, significant routing changes), validate against a fraction of production traffic before full rollout.

### 1. Run two Spooky instances on the same host

Run the new-config instance on a different port from the production instance. Both instances can share the same binary:

```bash
# Production instance (already running)
# Config: /etc/spooky/config.yaml, port 443

# Canary instance — the CLI only accepts --config/-c; set the bind address in the config file.
# Use config-new.yaml with `listen.address`/`listen.port` set to the canary port (e.g. 8443).
spooky --config /etc/spooky/config-new.yaml
```

Alternatively, use a separate unit file if running under systemd:

```bash
systemctl start spooky-canary
```

### 2. Route 5-10% of traffic to the canary instance

Use your upstream load balancer or weighted DNS to send a small fraction of traffic to the canary instance. The exact mechanism depends on your infrastructure:

- **HAProxy / Nginx upstream:** adjust `weight` on the canary backend to 5 out of 100
- **AWS ALB / GCP GLB:** use weighted target groups / backend services
- **DNS-based:** lower TTL and set a 90/10 A-record split

Keep the canary at low traffic weight until you have confirmed it behaves correctly.

### 3. Compare metrics between old and canary instances

Use the Prometheus queries below to compare the two instances side by side. Substitute `instance="<host>:<port>"` with the actual scrape labels for each instance.

**Request success rate (should be equal between instances):**

```promql
rate(spooky_requests_success{instance="host:9090"}[5m])
/
rate(spooky_requests_total{instance="host:9090"}[5m])
```

Run the same query for the canary instance and compare. A lower success rate on the canary indicates the new config is causing failures.

**p99 request latency (compare between instances):**

```promql
histogram_quantile(0.99,
  rate(spooky_upstream_request_latency_ms_bucket{instance="host:9901"}[5m])
)
```

This histogram is in **milliseconds** (buckets are `_ms`), and carries `upstream`/`outcome`/`le`
labels. A p99 latency increase on the canary instance (but not the production instance) points to
backend latency regression or a timeout misconfiguration in the new config.

**Backend health (compare health-check success/failure between instances):**

Spooky does not export a per-backend boolean health gauge. Use the health-check counters instead:

```promql
rate(spooky_health_checks_failure{instance="host:9901"}[5m])
```

A rising health-check failure rate on the canary but not the production instance confirms a backend
reachability or config problem specific to the new config. (Live per-backend health is also visible
in the `/admin/runtime` snapshot.)

### 4. Promote or roll back

If all three metrics are comparable between instances after 10-15 minutes at canary weight, increase the canary to 50%, watch for another 10 minutes, then promote fully by restarting the production instance with the new config.

If any metric diverges unfavorably, route all traffic back to the production instance and stop the canary. The production instance has been running throughout, so no rollback is required on the production side.

---

## What to Watch After Deploy

After restarting Spooky with a new config or new binary, watch the following five signals for the first 30 minutes. Set up dashboard panels or alert inhibitions before the restart so you can observe cleanly.

**1. `spooky_requests_success` rate**

```promql
rate(spooky_requests_success[2m])
```

This should match the pre-deploy baseline within 1-2 minutes of restart (after QUIC connections re-establish). A sustained drop below baseline indicates requests are failing. Compare against the same window from yesterday or the previous week to account for traffic volume changes.

**2. `spooky_backend_errors` rate**

```promql
rate(spooky_backend_errors[2m])
```

Any sudden increase after restart indicates Spooky is reaching backends but backends are returning errors. This points to a routing misconfiguration (requests sent to wrong backend), a backend environment mismatch, or an application-level problem triggered by the new routing.

**3. `spooky_backend_timeouts` rate**

```promql
rate(spooky_backend_timeouts[2m])
```

A timeout spike after restart is a strong signal that backend addresses changed to something unreachable, or that connection timeout values in the config are now too low for the backend's actual response time. Distinguish from backend errors: errors indicate a response was received (and was a failure); timeouts indicate no timely response at all.

**4. Backend health state via /admin/runtime**

Poll this endpoint repeatedly in the first few minutes after restart:

```bash
watch -n 5 'curl -sk https://127.0.0.1:9902/admin/runtime | jq ".upstreams | to_entries[] | {pool: .key, healthy: .value.healthy_count, total: .value.total_count}"'
```

Any pool where `healthy` is less than `total` after 60 seconds of uptime should be investigated. If a backend was healthy before the restart and is now unhealthy, the new config is likely pointing to a wrong address or using a different TLS mode that the backend does not expect.

**5. Process RSS memory**

```promql
process_resident_memory_bytes{job="spooky"}
```

After startup, RSS should stabilize within 2-3 minutes. Memory that grows linearly over the 30-minute observation window (without a corresponding linear increase in active connections) may indicate a resource leak introduced by the new version or config. This is a low-probability event on a minor release but important to catch early.

If any of these signals deviates from baseline and you cannot identify an innocent cause (expected traffic increase, known backend maintenance), roll back immediately rather than waiting to understand the root cause. The production instance can be restored in seconds; debugging under live traffic takes longer.

---

## Upgrade Procedure (Spooky Version Upgrade)

Follow these steps when upgrading the Spooky binary. This procedure applies to all version upgrades, including patch releases.

**1. Download the new binary.**

Download the release artifact for your platform from the Spooky release page and place it in a staging location:

```bash
curl -Lo /usr/local/bin/spooky-new https://github.com/supernova-labs/spooky/releases/download/v<VERSION>/spooky-linux-x86_64
chmod +x /usr/local/bin/spooky-new
```

Do not overwrite the running binary yet.

**2. Verify the binary version.**

```bash
/usr/local/bin/spooky-new --version
```

Confirm the output matches the intended release version. If it does not, you have the wrong artifact.

**3. Run startup validation with the existing config against the new binary.**

```bash
/usr/local/bin/spooky-new --config /etc/spooky/config.yaml
# Wait for "listening on ..." log line, then Ctrl-C
```

This confirms that the new binary accepts your existing configuration. New versions occasionally rename config fields or tighten validation rules. If the new binary rejects a config that the current binary accepts, read the error output and update the config before proceeding.

**4. Review the changelog for the new version — check "Breaking Changes" first.**

Read the release notes for every version between your current version and the target version (inclusive). Pay specific attention to:

- **Breaking changes:** any config field renames, removed options, or changed defaults that require config updates before the upgrade will work
- **Behavior changes:** changes to routing logic, health check behavior, or TLS handling that affect correctness without being strictly breaking
- **Deprecation notices:** fields that will be removed in a future version (update the config now to avoid a forced change later)

Do not skip the changelog even for patch releases. Security fixes sometimes require changed defaults.

**5. Replace the binary.**

```bash
cp /usr/local/bin/spooky-new /usr/local/bin/spooky
```

Use `cp`, not `mv`, to keep a clean audit trail in your package manager or deployment tooling. Verify the replacement:

```bash
/usr/local/bin/spooky --version
```

**6. Restart via systemd.**

```bash
systemctl restart spooky
```

Do not use `systemctl stop` followed by `systemctl start` — this creates an unnecessary gap in availability. `systemctl restart` performs a clean stop-then-start in sequence.

Check that systemd considers the service active:

```bash
systemctl status spooky
```

Expected: `Active: active (running)`. If the service enters a failed state, check `journalctl -u spooky -n 50` for the startup error.

**7. Confirm /health returns 200 within 5 seconds of restart.**

```bash
for i in $(seq 1 10); do
  STATUS=$(curl -o /dev/null -sk -w "%{http_code}" https://127.0.0.1:9902/health)
  echo "$(date +%T) /health: $STATUS"
  [ "$STATUS" = "200" ] && break
  sleep 0.5
done
```

If `/health` does not return 200 within 5 seconds, the process has either not started successfully or is taking unusually long to initialize. Check systemd status and logs before proceeding.

**8. Watch metrics for 10 minutes before considering the upgrade complete.**

Apply the same five-signal checklist from the [What to Watch After Deploy](#what-to-watch-after-deploy) section above. Version upgrades introduce more unknowns than config-only changes, so extend the observation window to 10 minutes minimum before marking the deploy complete.

If you observe any unexpected metric change during this window, you have two rollback options:

- **Binary-only rollback:** the previous binary is still at its original path if you followed step 5 above. Restore it with `cp /usr/local/bin/spooky-old /usr/local/bin/spooky && systemctl restart spooky`.
- **Full rollback:** if a config change accompanied the upgrade, restore both the config and the binary before restarting.

After a successful upgrade, remove the staging binary and update your deployment tooling to record the new version:

```bash
rm /usr/local/bin/spooky-new
```
