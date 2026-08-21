# Secret and Certificate Rotation

This page is the operator runbook for rotating listener certificates, upstream client certificates, and upstream CA trust material, and for reasoning about secret-backed config in general.

Read [Reload and Drain](reload-and-drain.md) first if you have not already — this page assumes you understand the generation model and the difference between `reload-certs` and generation activation.

## Which Path Applies

| Material | Path | Why |
| --- | --- | --- |
| Downstream listener cert/key, listener client-auth CA | `POST /admin/runtime/reload-certs` | Listener-scoped hot swap; does not touch runtime generation |
| Upstream client certificate/key (mTLS) | `validate` → `preview` → `activate` | Generation-owned; rebuilds the affected backend connection pool |
| Upstream CA bundle (`ca_file`/`ca_dir`) | `validate` → `preview` → `activate` | Generation-owned; same reasoning as client cert/key |
| `secrets.providers` registry shape | `validate` → `preview` → `activate` | Generation-owned config, not a listener concern |

Upstream TLS material is never rotated through `reload-certs`, even when the change is "just a certificate." `reload-certs` is intentionally narrow: it only reloads listener identity and listener client-auth trust material for new downstream handshakes. Upstream client identity and upstream trust roots always go through the normal activation path so the change gets a diff, a generation number, and rollback semantics.

## Secret References

Prefer file-backed secret references over plaintext values for anything sensitive:

```yaml
secrets:
  default_provider: local_filesystem
  providers:
    local_filesystem:
      kind: file
      base_dir: "/etc/spooky/secrets"

upstream:
  payments:
    tls:
      client_certificate_ref:
        ref: "file:///etc/spooky/secrets/upstream/payments-client.crt"
      client_key_ref:
        ref: "file:///etc/spooky/secrets/upstream/payments-client.key"
```

A `_ref` field and its plaintext sibling (`client_certificate` / `client_certificate_ref`, `client_key` / `client_key_ref`, `secret` / `secret_ref`, `client_secret` / `client_secret_ref`, `auth_token` / `auth_token_ref`, `token` / `token_ref`) are mutually exclusive — setting both fails validation. Supported ref schemes are `literal:<value>` and `file://<path>`.

Secret references resolve **eagerly during activation**, not lazily on first request. A missing file, unreadable file, empty file, or malformed PEM fails `validate`/`activate` before the candidate generation goes live — it never reaches request-serving code.

## Downstream Listener Certificate Rotation

1. Replace the cert/key files on disk. Prefer an atomic rename into place — see [Secret File Replacement Safety](#secret-file-replacement-safety) below.
2. Call:

   ```bash
   curl -k --http1.1 -X POST https://127.0.0.1:9902/admin/runtime/reload-certs \
     -H "Authorization: Bearer <token>"
   ```

   The path is whatever `observability.control_api.reload_certs_path` is configured to (default `/admin/runtime/reload-certs`).
3. Confirm the new cert in the runtime snapshot: `GET /admin/runtime` → `tls.listeners.<listener>` shows an updated `generation`, `last_loaded_at_unix_ms`, and `default_cert_not_after_unix_seconds`.
4. Confirm `spooky_control_plane_cert_reload_total{result="success"}` incremented and downstream cert-expiry metrics reflect the new material.

This does not rebuild the runtime generation, mutate route/policy state, or affect already-negotiated sessions — only new handshakes see the new certificate.

## Upstream Client Certificate Rotation (mTLS)

1. Write the new client cert/key to the staged secret path (atomic rename — see below).
2. Run the staged activation flow against the config, using the same reference (if rotating in place at the same path) or a new reference (if introducing a new path):

   ```bash
   curl -k --http1.1 -X POST https://127.0.0.1:9902/admin/runtime/validate \
     -H "Authorization: Bearer <token>" -H "content-type: application/json" \
     -d '{"requested_by":"ops","reason":"rotate payments client cert"}'

   curl -k --http1.1 -X POST https://127.0.0.1:9902/admin/runtime/activate \
     -H "Authorization: Bearer <token>" -H "content-type: application/json" \
     -d '{"expected_generation":<current>,"requested_by":"ops","reason":"rotate payments client cert"}'
   ```

3. Confirm the generation changed: `GET /admin/runtime/history` shows a new `entries` record, and its diff includes a `backend_policies` domain entry with `secret_material_changed: true`.
4. Confirm the connection pool rebuilt rather than mutated in place — a client identity change always produces a new pool instance for that backend, never a live in-place swap.
5. Confirm upstream mTLS handshakes succeed against the rotated identity, and `spooky_upstream_tls_failure_total` is not climbing for that upstream/backend pair.

**Same-path rotation is detected.** If the file at the same `file://` reference path changes content, activation recomputes the fingerprint and the diff still reports `secret_material_changed: true` even though the reference string itself did not change — you do not need to introduce a new reference to force detection.

## Upstream CA Rotation

Upstream CA rotation (`ca_file` / `ca_dir`) follows the exact same `validate` → `preview` → `activate` flow as client certificate rotation, since it is also generation-owned, not `reload-certs`-scoped.

For safe overlap during a CA transition:

1. Stage the new CA alongside the old one — either append the new CA to the existing `ca_file` bundle, or add it as an additional file in `ca_dir` (all PEM files in the directory are trusted).
2. Activate with both CAs trusted. Confirm upstream connections still succeed against backends serving certs from either CA.
3. Once all backends have rotated to certs signed by the new CA, remove the old CA and activate again.
4. Roll back (see below) if backend handshakes start failing at any step — do not proceed to the next step under active failures.

Do not remove the old CA in the same activation that introduces the new one unless you have already confirmed every backend has rotated — that removes your rollback safety margin.

## Secret File Replacement Safety

Prefer an atomic rename into place over in-place truncate-and-write for any file a `file://` secret reference points at:

```bash
# Write to a temp file in the same directory, then rename atomically.
install -m 0640 payments-client.crt.new /etc/spooky/secrets/upstream/payments-client.crt
```

`install`, `mv` within the same filesystem, or an equivalent atomic rename avoids a window where a reader observes a partially-written file. Truncate-and-write can produce a transient empty or malformed file that activation reads as `EmptySecret` or `MalformedPemCertificate` even though the final content would have been fine — an atomic rename avoids that window entirely.

## Rollback Expectations

Runtime rollback (`POST /admin/runtime/rollback`) restores a previously retained **runtime generation view** — the resolved policy state Spooky held for that generation. It does not, and cannot, restore external secret files.

Concretely:

- if you roll back to a generation that referenced `file:///etc/spooky/secrets/upstream/payments-client.crt`, and that file has since been deleted or overwritten, the rollback will re-resolve the reference against whatever is on disk **now**, not what was on disk when that generation was originally active.
- a rollback that re-reads a now-missing or now-different secret file can fail activation, or silently activate with different material than the original generation had, if the fingerprint at rollback time doesn't match what was recorded historically.
- do not treat retained runtime history as a backup of secret material. If you need to guarantee exact secret-content recovery, that is a secret-file backup/versioning problem, separate from runtime generation retention.

Before relying on rollback as your recovery path for a bad secret rotation, confirm the previous secret file content is still present and unchanged on disk.

## Failure Handling

When a secret or cert rotation does not behave as expected, inspect in this order:

1. **`GET /admin/runtime`** — check `tls.listeners`, `tls.upstreams`, and `secrets.material` for the affected scope. Each material item reports `source_kind`, a sanitized `reference`, `fingerprint`, `last_loaded_at_unix_ms`, `last_reload_status`, and (for certificates) `expiry_not_after_unix_seconds`. None of these ever include raw secret bytes.
2. **`GET /admin/runtime/history`** — confirm whether the activation attempt succeeded, and read the `rejected_changes` detail if it did not. A rejected activation leaves the active generation unchanged.
3. **Audit logs** — look for these action values:

   | Action | Meaning |
   | --- | --- |
   | `cert_reload_applied` | listener cert reload succeeded |
   | `secret_resolution_failed` | a secret reference failed to resolve during activation (missing file, bad permissions, malformed PEM, etc.) |
   | `upstream_mtls_material_changed` | an activation changed upstream client cert/key or CA fingerprint |
   | `upstream_mtls_material_invalid` | an activation was rejected because upstream TLS material was invalid |

4. **TLS and secret metrics**:

   | Metric | Use |
   | --- | --- |
   | `spooky_secret_reload_total{scope,result,reason}` | reload attempts by scope (`listeners`/`upstreams`) and outcome |
   | `spooky_secret_resolve_total{provider,result,reason}` | resolution attempts by provider and outcome |
   | `spooky_secret_last_success_unixtime{scope}` | staleness — how long since the last successful resolve for a scope |
   | `spooky_upstream_tls_failure_total{upstream,backend,phase,reason}` | live TLS/mTLS handshake failures against a rotated or misconfigured identity, by request phase |
   | `spooky_upstream_client_certificate_not_after_seconds{upstream}` | absolute expiry timestamp per upstream |
   | `spooky_upstream_client_certificate_days_remaining{upstream}` | days-remaining gauge for alerting ahead of expiry |
   | `spooky_control_plane_cert_reload_total{result,reason}` | listener cert reload outcomes |

None of the control-plane JSON, audit events, or metrics expose secret contents, private key bytes, or full provider URIs that could leak credentials — only sanitized scope, source kind, fingerprint, and timing/status metadata.

## Related Pages

- [Reload and Drain](reload-and-drain.md)
- [Runbook](runbook.md)
- [Control API Reference](../reference/control-api-reference.md)
- [TLS Configuration](../configuration/tls.md)
- [Metrics Reference](../reference/metrics-reference.md)
