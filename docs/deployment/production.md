# Production Deployment

> Spooky is beta software. Deploy it to production only with staged rollout, rollback readiness, and verified observability. Read [Production Readiness](../operations/production-readiness.md) first.

This guide covers the recommended production host layout, service model, security posture, rollout sequence, and change-management workflow for Spooky.

## Recommended Deployment Shape

Use Spooky as:

- an active-active edge pool
- behind a UDP-capable load balancer or traffic manager
- with one protected Control API surface per node
- with Prometheus scraping and centralized log collection in place before broad rollout

Recommended rollout shape:

1. one canary node or one bounded traffic slice
2. staged activation for runtime-managed config changes
3. cert reload for cert-only changes
4. drain-aware restart or node replacement for restart-required changes

## Minimum Host Baseline

Start with:

- Linux kernel 5.x or later
- 4 to 8 cores for a serious production node baseline
- 4 GB minimum memory, more for heavy concurrency or large-body workloads
- enough file descriptors and socket buffer headroom for expected traffic

Validate with your real traffic shape before treating these as final numbers.

## Directory And Account Layout

Recommended baseline:

```bash
sudo useradd --system --shell /usr/sbin/nologin \
  --home-dir /var/lib/spooky --create-home spooky

sudo mkdir -p /etc/spooky/certs /var/lib/spooky /var/log/spooky

sudo chown root:spooky /etc/spooky
sudo chmod 750 /etc/spooky

sudo chown root:spooky /etc/spooky/certs
sudo chmod 750 /etc/spooky/certs

sudo chown spooky:spooky /var/lib/spooky /var/log/spooky
sudo chmod 750 /var/lib/spooky /var/log/spooky
```

Recommended permissions:

- config files: readable by `root` and the `spooky` group
- private keys: readable only by the minimum required service identities
- writable paths: limited to runtime state and optional local logs

## Binary Installation

Install a release binary or a verified internal build into a stable path such as:

```bash
sudo install -m 755 -o root -g root spooky /usr/local/bin/spooky
```

Keep:

- the current production binary
- the previous known-good binary
- checksums or provenance metadata for the binary you deployed

## Host Tuning Baseline

Before rollout, set and verify:

- UDP and TCP buffer ceilings
- device backlog and packet budget
- file descriptor ceilings
- privileged-port bind strategy
- conntrack behavior, if present

Use [Host Tuning](../operations/host-tuning.md) for the tuning model and `scripts/sysctl-linux-network-tuning.sh` only as a starting helper.

Example `sysctl` baseline:

```bash
# /etc/sysctl.d/99-spooky.conf
net.core.rmem_max = 67108864
net.core.wmem_max = 67108864
net.core.rmem_default = 16777216
net.core.wmem_default = 16777216
net.core.netdev_max_backlog = 65536
fs.file-max = 2097152
```

Apply and verify:

```bash
sudo sysctl --system
sysctl net.core.rmem_max
sysctl fs.file-max
```

## Resource Limits

Recommended service-account limits:

```bash
# /etc/security/limits.d/spooky.conf
spooky soft nofile 1048576
spooky hard nofile 1048576
spooky soft nproc 16384
spooky hard nproc 16384
```

Also reflect the same intent in your `systemd` unit.

## Control API Posture

Treat the Control API as operator-only infrastructure.

Recommended posture:

- bind to loopback or an isolated admin network
- require TLS
- require explicit auth
- grant `viewer`, `operator`, and `admin` roles deliberately
- use `--http1.1` for all `curl` interactions

Do not expose the Control API broadly on the same network surface as public traffic.

## Certificate Management

Use a documented certificate lifecycle with:

- predictable source of truth
- verified SAN coverage
- expiry monitoring
- tested cert reload workflow

Before replacing a certificate, verify:

```bash
openssl x509 -noout -dates -in /etc/spooky/certs/fullchain.pem
openssl x509 -noout -text -in /etc/spooky/certs/fullchain.pem | grep -A1 "Subject Alternative Name"
openssl rsa -noout -modulus -in /etc/spooky/certs/privkey.pem | openssl md5
openssl x509 -noout -modulus -in /etc/spooky/certs/fullchain.pem | openssl md5
```

For cert-only updates, prefer:

```bash
curl -k --http1.1 -X POST \
  -H "Authorization: Bearer <operator-token>" \
  https://127.0.0.1:9902/admin/runtime/reload-certs
```

Use a full restart only when the change is not cert-only.

## Example Service Unit

Use `systemd` as a supervised service layer and keep change management in your rollout automation rather than depending on ad hoc shell access.

```ini
[Unit]
Description=Spooky edge runtime
Documentation=https://github.com/Supernova-Labs-Org/spooky
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=spooky
Group=spooky
ExecStart=/usr/local/bin/spooky --config /etc/spooky/config.yaml
Restart=always
RestartSec=5s
LimitNOFILE=1048576
LimitNPROC=16384
TasksMax=16384
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/spooky /var/log/spooky
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictNamespaces=true
RestrictRealtime=true
RestrictSUIDSGID=true
LockPersonality=true
StandardOutput=journal
StandardError=journal
SyslogIdentifier=spooky

[Install]
WantedBy=multi-user.target
```

Notes:

- Do not assume `systemctl reload` is your primary production change path.
- Prefer explicit Control API activation automation for runtime-managed changes.
- Use `systemctl restart` only for restart-required changes or binary replacement workflows.

## Change Management Model

### Runtime-managed config changes

Use:

1. config render or distribution
2. `POST /admin/runtime/validate`
3. `POST /admin/runtime/preview`
4. `POST /admin/runtime/activate`
5. runtime-history and metrics verification

### Certificate-only changes

Use:

1. write new cert material
2. verify permissions and expiry
3. `POST /admin/runtime/reload-certs`
4. verify new handshakes

### Restart-required changes

Use:

1. canary node or bounded slice
2. drain-aware restart or node replacement
3. post-restart verification
4. expand only after stable health and latency

## Activation Workflow Example

```bash
curl -k --http1.1 -X POST \
  -H "Authorization: Bearer <operator-token>" \
  -H "content-type: application/json" \
  -d '{"config_path":"/etc/spooky/config.yaml","requested_by":"ops","reason":"route change"}' \
  https://127.0.0.1:9902/admin/runtime/validate

curl -k --http1.1 -X POST \
  -H "Authorization: Bearer <operator-token>" \
  -H "content-type: application/json" \
  -d '{"config_path":"/etc/spooky/config.yaml","requested_by":"ops","reason":"route change"}' \
  https://127.0.0.1:9902/admin/runtime/preview

curl -k --http1.1 -X POST \
  -H "Authorization: Bearer <operator-token>" \
  -H "content-type: application/json" \
  -d '{"config_path":"/etc/spooky/config.yaml","expected_generation":12,"requested_by":"ops","reason":"route change"}' \
  https://127.0.0.1:9902/admin/runtime/activate
```

## Rollback Workflow Example

Choose a retained generation first:

```bash
curl -k --http1.1 \
  -H "Authorization: Bearer <viewer-or-operator-token>" \
  https://127.0.0.1:9902/admin/runtime/history
```

Then roll back:

```bash
curl -k --http1.1 -X POST \
  -H "Authorization: Bearer <operator-token>" \
  -H "content-type: application/json" \
  -d '{"target_generation":11,"expected_active_generation":12,"requested_by":"ops","reason":"rollback"}' \
  https://127.0.0.1:9902/admin/runtime/rollback
```

## Observability Before Traffic Expansion

Before broad rollout, verify:

- metrics endpoint is scraped successfully
- logs are shipping
- Control API health and readiness are reachable from the admin path
- dashboards for latency, overload, quota, backend health, and TLS are populated

Use the shipped observability package rather than inventing an unverified local query set.

See:

- [Observability Operator Bundle](../operations/observability-bundle.md)
- [Metrics Reference](../reference/metrics-reference.md)

## Rollout Procedure

### New config on an existing binary

1. Render the candidate config to disk.
2. Validate it before touching traffic.
3. Activate on one canary node or one bounded slice.
4. Watch latency, overload, backend health, quota outcomes, and auth outcomes.
5. Expand gradually.

### New binary

1. Keep the previous binary available.
2. Deploy to one canary node first.
3. Let the node rejoin only after health, readiness, and key dashboards stay stable.
4. Roll out node by node or slice by slice.

### Restart-required config change

1. Prepare the config and verify it is intentionally restart-required.
2. Use a drain-aware restart or node replacement workflow.
3. Keep rollback ready at the binary and config level.

## Security Checklist

Before production, confirm:

1. private keys are minimally readable
2. Control API is not broadly exposed
3. service account permissions are minimal
4. public ingress and admin-plane firewall rules are distinct
5. logs do not expose secrets or unnecessary request material

## Final Pre-Go-Live Checklist

Confirm all of the following:

- host tuning baseline applied and verified
- metrics and logs visible
- Control API auth tested
- cert reload tested
- runtime activation and rollback tested
- restart-required workflow tested
- canary rollout procedure documented
- incident owner and rollback owner clear

## Related Pages

- [Production Readiness](../operations/production-readiness.md)
- [Reload and Drain](../operations/reload-and-drain.md)
- [Validation](validation.md)
- [Deployment Patterns](../operations/deployment-patterns.md)
- [Runbook](../operations/runbook.md)
