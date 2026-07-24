# Phase 0 — Frozen Operational Contract

Status: **snapshot of current behavior as of commit `5e32491` (branch `master`).**
No code changed. This is the deliverable for Phase 0 of `machine-hardening.md`: it
documents how startup, reload, drain, and shutdown behave *today* so later phases can
change semantics against a known baseline. All `file:line` references are relative to the
workspace root unless noted; edge-crate paths are under `crates/edge/src/`.

> Scope note: the plan's file layout (`quic_listener/startup/*`, `watchdog/*` as a
> `quic_listener` submodule, `drain*`, `shutdown*`) does not match the tree. The real
> layout is flat files under `crates/edge/src/quic_listener/`, a top-level
> `crates/edge/src/watchdog/`, and `crates/edge/src/runtime/`. The **process entry point
> lives outside the edge crate** in the `spooky` binary (`spooky/src/app.rs`,
> `spooky/src/listener_group.rs`); the edge crate provides building blocks, the binary
> orchestrates them.

---

## 1. Trace — Listener startup (boot → first active generation)

Entry: `main_entry` (`spooky/src/app.rs:33`).

1. `app.rs:59` init logger → `app.rs:65` init tracing → `app.rs:71` install panic hook.
2. `app.rs:75` validate config → `app.rs:79` normalize to `RuntimeConfig`.
3. `app.rs:90` early privileged-port check.
4. `app.rs:104` `configure_async_runtime` (fallback RT thread count, `quic_listener/async_runtime.rs:14`) → `app.rs:106` build dedicated `spooky-control-plane` Tokio runtime → `app.rs:125` `block_on(run)`.
5. **Generation 0 assembled** — `app.rs:139` `build_runtime_bundle` (`quic_listener/startup.rs:363`) → `build_shared_state` (`startup.rs:111`):
   - `startup.rs:174` load listener TLS material (`tls_runtime.rs:710`)
   - `startup.rs:184` duplicate-origin guard while building backend resolutions
   - `startup.rs:246-248` routing index, metrics, DNS resolver
   - `startup.rs:249,251` backend resolution store + lifecycle coordinator
   - `startup.rs:263` upstream transport pool; `startup.rs:274` per-upstream pools + inflight semaphores
   - `startup.rs:321` resilience runtime; `startup.rs:325` watchdog coordinator
   - `startup.rs:332` package into `SharedRuntimeState::from_parts`; `startup.rs:370` stamp `generation: 0`
6. **Publish** — `app.rs:148` `RuntimeBundleHandle::new` (`runtime/bundle.rs:90`). Gen 0 is now globally readable (`RwLock<Arc<RuntimeBundle>>`).
7. **Control-plane tasks** — `app.rs:153` `spawn_control_plane_tasks_with_runtime_bundle` (`quic_listener/control_plane.rs:35` → `:49`):
   - `control_plane.rs:52,63` watchdog expected-worker count set
   - `control_plane.rs:68` backend DNS refresh task; `:76` health-check task; `:85` watchdog service task (only if enabled, `:131`)
   - `control_plane.rs:56` metrics exporter started + eagerly bound (`metrics/service.rs:37`)
   - `control_plane.rs:57` control API server started + eagerly bound (`control_api/service.rs:62`; requires bootstrap TLS on primary listener, `service.rs:28`)
8. `app.rs:177` shutdown-signal watcher spawned (`wait_for_shutdown_signal`, `app.rs:233`, SIGTERM/Ctrl-C).
9. **Sockets bound + data-plane workers** — per listener `spawn_managed_listener_group` (`app.rs:186`, `spooky/src/listener_group.rs:113`):
   - `listener_group.rs:122` bootstrap TCP/TLS listener bound (`bootstrap/startup.rs:66`)
   - `listener_group.rs:138` UDP data-plane worker group; **UDP sockets bound** (`workers.rs:61` reuseport / `:66` single, actual `bind()` `startup.rs:598`). Workers are OS threads `spooky-data-plane-N` (`workers.rs:75,91`); each builds its own `QUICListener` via `initialize_listener_from_runtime` (`workers.rs:172`, `runtime_state.rs:315`)
10. `app.rs:202` startup banner → `app.rs:203` **privilege drop** (after binds, so privileged ports are claimed first).
11. `app.rs:206` supervisor loop: reap finished worker groups + reconcile topology every 100 ms.

**First active generation observable:** gen 0 is *readable* the instant the handle is
published (`app.rs:148`); it becomes *live to clients* progressively — control/metrics
endpoints when their eager binds succeed (`app.rs:153`), the data plane per-worker when
each worker enters `poll()` (`workers.rs:346` single / `:190` sharded) and its first
`poll_preamble` (`shutdown.rs:58`) calls `sync_runtime_bundle_if_needed` +
`watchdog.mark_poll_progress()`. That is the earliest point the generation is active
end-to-end.

Fatal startup errors call `std::process::exit` directly (`app.rs:144,159,198,258`), **not** panic.

---

## 2. Trace — Control API reload (intake → commit / reject)

Entry: `handle_control_api_runtime_reload` (`control_api/reload.rs:98`).

1. **Accept + gate** — TLS/TCP accept and connection-slot limit `control_api/service.rs:208` → `handle_control_api_request` (`control_api/http.rs:4`) → `gate_control_api_request` (`control_api/auth.rs:93`): route match (`auth.rs:41`) + auth (`auth.rs:64`). Reload route = `POST` on operator-configured `reload_path` (`auth.rs:56`). Auth is Bearer-token, constant-time compare via `subtle::ConstantTimeEq` (`auth.rs:131`); missing configured `auth_token` → fail-closed reject (`auth.rs:119`).
2. **Build plan** — `build_runtime_reload_plan` (`reload.rs:203`): re-read config from `current.startup().config_path` (`reload.rs:207`, `crates/config/src/loader.rs:7`) → validate (`reload.rs:208`, `crates/config/src/validator.rs:187`) → normalize (`reload.rs:210`) → **rebuild** entirely fresh `SharedRuntimeState` via `build_shared_state` (`reload.rs:212`) → stamp `current.generation()+1` (`reload.rs:220`).
3. **Validate compatibility** — `validate_runtime_reload_plan` (`reload.rs:233`) fans to four checkers (see §8).
4. **Commit vs reject** — the four ordered gates in `handle_control_api_runtime_reload` each early-return a rejection JSON; success falls through to commit. **Active generation is only mutated at the single `std::mem::replace`** (`bundle.rs:134`), reached only after all gates pass ⇒ any rejection leaves the live generation untouched (guaranteed).
5. **Install** — `apply_runtime_reload_plan` (`reload.rs:254`): spawn new generation's background tasks (`reload.rs:258`) → `RuntimeBundleHandle::replace` (`reload.rs:262`, `bundle.rs:121`) → retire old generation tasks with 5 s grace (`bundle.rs:136-140`) → 202 `{"reloaded": true, "generation": …, "path": …}` (`reload.rs:164`).

> `edge/.../quic_listener/validation.rs` is HTTP **request**-header validation for the data
> plane, **not** config-reload validation. Reload validation lives in `reload.rs` +
> `spooky_config`.

Post-commit non-rollback: `apply_live_log_level_reload` failure (`reload.rs:158`) is logged
but does **not** roll back the already-committed generation.

---

## 3. Trace — Generation swap (retain / drop / drain of old state)

A generation is an immutable `RuntimeBundle` (`runtime/bundle.rs:17`) holding `generation:
u64` + startup-owned state + `RuntimeConfig` + `Arc<SharedRuntimeState>`.
`SharedRuntimeState` (`runtime/shared_state.rs:5`) splits into `RuntimeSharedServices`
(`generation.rs:36` — TLS store, transport pool, backend lifecycle/resolution, DNS, metrics,
**watchdog**) and `RuntimeGenerationState` (`generation.rs:47` — listener configs, backend
endpoints/health, upstream pools/semaphores, routing index, resilience, task registry).

- **Swap primitive:** `RwLock<Arc<RuntimeBundle>>` (`bundle.rs:86`) with `std::mem::replace` under the write guard (`bundle.rs:134`). Readers `read()`+clone the inner `Arc` (`bundle.rs:96-101`), so each reader sees the whole old bundle or the whole new one — atomic per reader. **Not** arc-swap, **not** a watch channel.
- **Rebuilt, not moved:** everything in the new bundle is constructed fresh by `build_shared_state` — new transport pool, upstream pools, semaphores, routing index, resilience, metrics, TLS store. In-flight semaphore permits and warm pooled connections are **not** carried over.
- **Old-state retention:** the old `Arc<RuntimeBundle>` returned by `mem::replace` is dropped at end of `replace`, but its `SharedRuntimeState` stays alive as long as any data-plane worker still holds a clone from an earlier `current_view()`. Old-generation background tasks are aborted + waited up to **5 s** (`bundle.rs:140`, `tasks.rs:80`); on timeout it logs `"generation background tasks did not stop within …; continuing reload"` (`tasks.rs:69-77`) and proceeds. ⇒ **old + new generations overlap for up to ~5 s** on separate `Arc` graphs (no shared-mutable corruption).
- **Data-plane adoption (lazy, per worker):** at the top of every poll, `poll_preamble` (`shutdown.rs:58`) → `sync_runtime_bundle_if_needed` (`tls_runtime.rs:302`) compares cached generation vs `current_view()` (`tls_runtime.rs:318`); if changed it re-points every `Arc` field of the live `QUICListener` in place and rebuilds QUIC config (`tls_runtime.rs:333-370`). Existing `self.connections` are **left intact** — connections opened under the old generation keep running but forward using the new generation's pools/limits. **There is no per-connection drain on reload**; drain happens only on watchdog restart or process shutdown.
- **TLS-only reload:** listener TLS store swaps atomically under its own `RwLock` in `replace_listeners` (`runtime/tls/store.rs:71,96-98`).

---

## 4. Trace — Backend refresh / update (failure propagation)

Backend lifecycle is split across two coordinates that never merge into one authoritative
record:

- **Resolution state** (identity, authority, DNS addrs, refresh generation): `RuntimeBackendResolutionStore` (`runtime/backend/store.rs`) fronted by `BackendLifecycleCoordinator` (`runtime/backend/lifecycle.rs:120`).
- **Health / membership / placement:** lives in the `UpstreamPool`s (`Arc<RwLock<UpstreamPool>>` in `spooky_lb`), **not** the store. The store's per-backend `health`/`membership` are seeded once (`Unknown`/`Active`, `lifecycle.rs:55-56`) and never mutated by runtime paths; truth is reassembled on demand by `snapshot_inventory` merging pool state (`lifecycle.rs:146-229`).

**Three independent refresh triggers:** timer DNS refresh (`backend_resolution.rs:12`, mutates resolution generation only), timer active health checks (`health_check.rs:17`, mutates pool health only), passive request feedback (`lifecycle.rs:270`, pool health only).

**Failure propagation — all non-fatal / silent:**
- DNS lookup failure → `LookupFailed { retained_addrs, error }` (`lifecycle.rs:344`); empty answer → `EmptyAnswerRetained` (`lifecycle.rs:350`). Old addrs retained; loop continues. `warn!` logged (`lifecycle.rs:526,537`); a single generic metric `inc_backend_dns_refresh_failure()` (`lifecycle.rs:482`) does **not** distinguish empty vs hard failure.
- **Client rotation failure is fully silent** — `rotate_backend_client` `Err` collapses to `client_rotated = false`, not logged, refresh still reported success (`lifecycle.rs:386-396`); stale pooled connections persist until idle timeout.
- **Health state is not transferred across reload** — a new store/pool set resets every backend to seed `Unknown`; no code copies prior health. Silent, unreported.
- Partial pool rebuild does **not** exist — `UpstreamPool` build failure is all-or-nothing: fail-fast at startup (`startup.rs:276-281`), atomic reject at reload (`reload.rs:212-214` → 500, live generation untouched).
- Poisoned pool lock → health/feedback update silently dropped (`lifecycle.rs:278,317` `.write().ok()?`); `snapshot_inventory` silently omits a poisoned pool (`lifecycle.rs:168`), making its backends look `Removed`.

**Latent/unused surface:** `RuntimeBackendResolutionUpdate` (`update.rs`), `BackendLifecycleEvent`/`Mutation` variants `HealthUpdated`/`MembershipUpdated`/`SnapshotPublished`/`Noop` (`event.rs:17-45`) are defined but not driven by any runtime path (only `ResolutionUpdated` is produced, `store.rs:112`).

---

## 5. Trace — Watchdog interactions

The watchdog is a shared lock-free-ish `WatchdogCoordinator` + one async supervisor task
(`run_watchdog_service`) + participation by data-plane workers through the poll preamble.

- **Startup registration:** no per-component handshake. Single coordinator stored in `RuntimeSharedServices.watchdog` (`generation.rs:43`), threaded into every listener (`runtime/listener.rs:50`). Expected worker count set at bootstrap (`control_plane.rs:52-53,63-66`). Service task spawned in `spawn_watchdog_service` (`control_plane.rs:114`), short-circuits if disabled (`state.rs:24-28`, `control_plane.rs:131-133`); if no Tokio handle → `error!("Watchdog disabled: no Tokio runtime available")` and returns (`control_plane.rs:135-141`). Registered into the **generation's** task registry (`control_plane.rs:143-149`) ⇒ generation-scoped lifetime.
- **Generation observation / liveness:** heartbeat is a single relaxed `AtomicU64` timestamp `last_poll_progress_ms` set by `mark_poll_progress()` in every poll preamble (`coordinator.rs:59`, from `mod.rs:233/260/273`). Service computes `stalled = now − last_poll_progress > poll_stall_timeout_ms` (`service.rs:66-67`). The watchdog `Arc` is a **shared service** (survives generations); the watchdog **task** is generation-scoped and re-spawned per generation.
- **Drain/shutdown participation:** degraded windows past threshold → `request_restart(reason)` (`service.rs:114`, `coordinator.rs:76-105`, CAS-guarded + cooldown). Workers observe `should_enter_draining()` (`listener.rs:89-91`), log `warn!("Watchdog requested restart; entering draining mode")`, `start_draining()`; on completion `mark_worker_drained()` once per worker (guarded, `listener.rs:93-98`). Service waits for `workers_drained()` OR `drain_grace_ms` (`service.rs:125-130`), then runs the restart command.
- **Failure signaling:** three ORed pressures — `stalled`, `timeout_pressure`, `overload_pressure` (`service.rs:63-84`). `degraded_windows >= unhealthy_consecutive_windows` → action. Action is **out-of-process restart via configured command**, never in-process abort/panic. If `restart_command` unset → `warn!` + detection-only (`service.rs:96-102`). Otherwise spawns `tokio::process::Command` with `env_clear()` + `PATH` + `SPOOKY_WATCHDOG_REASON` (`service.rs:145-158`). On command failure → `error!` + restart stays pending, retried next loop (bounded by cooldown).

**Clock-skew note:** stall detection uses wall-clock `SystemTime` (`time.rs`, `unwrap_or(0)`) and is skew-sensitive; the drain-grace deadline uses monotonic `Instant` (`coordinator.rs:119`) and is skew-safe. Inconsistency worth freezing.

### Drain / shutdown entry points and idempotency

Two independent drain mechanisms share the same `QUICListener` primitives
(`quic_listener/shutdown.rs`): (1) process/worker shutdown via `Arc<AtomicBool>`
(`runtime_state.rs:306`), (2) watchdog-restart drain via the coordinator flag.

- **Single-worker shutdown:** `run_single_listener_worker` loops on `shutdown`, then `drain_with_active_polls()` (`workers.rs:345-350`).
- **Sharded shutdown ordering:** dispatcher exits (`workers.rs:222`) → `drop(shard_txs)` (`:298`) → shards see `Disconnected`, break, `drain_with_idle_polls()` (`:203-209`) → dispatcher `join`s shards (`:301-324`). Both the flag and channel-close terminate shards (belt-and-suspenders).
- **Idempotency:** `start_draining` early-returns if already draining (`shutdown.rs:11-18`); `drain_complete` is a safe repeatable query + `close_all_connections` (`shutdown.rs:20-42`); `finish_watchdog_drain` guarded so `mark_worker_drained` fires ≤ once per cycle (`shutdown.rs:93-98`); `request_restart` CAS-guarded; task-registry retirement sets `retired = true` and is safe to double-run (`tasks.rs:32-53`, `Drop` also aborts, `:101-105`). Per-listener `&mut self` exclusivity (one shard thread per listener) is the real protection.
- **In-flight drain + deadline:** during drain, new connection attempts dropped (`connection.rs:151-154`, `inc_ingress_draining_drop`); existing connections polled until `has_active_streams()` clears. Deadline `drain_timeout` from `timeout_policy.shutdown_drain` (`startup.rs:50`); on elapse force-close `close_all_connections` with QUIC reason `b"draining"` (`shutdown.rs:106-121`). **Drain loops are busy-spins** — `while !drain_complete() { poll() }` with no sleep/yield (`shutdown.rs:44-56`), spinning hot up to `drain_timeout`.
- Watchdog restart may execute with connections still active if `drain_grace_ms` elapses first (`service.rs:138-143`, `warn!("…drain grace elapsed; executing hook without full drain")`).

---

## 6. Invariants table (current contract)

| subsystem | current owner | who may mutate | when mutation allowed | current failure behavior | current fallback behavior |
|---|---|---|---|---|---|
| Runtime bundle (generation) | `RuntimeBundleHandle` (`bundle.rs:85`) | control-API reload path only, via `replace` (`bundle.rs:121`) | after all reload gates pass | reject before commit; on write-lock poison → `ProxyError` + abort staged tasks (`bundle.rs:127-132`) | **read side panics on poison** (`bundle.rs:100`) — asymmetric with write side |
| UDP listener sockets | startup-owned (`workers.rs`) | startup + supervisor topology reconcile (`listener_group.rs:192`) | startup; reload only for *new* listeners (existing bind change ⇒ restart-required, `reload.rs:281`) | preflight bind on reload (`reload.rs:303-321`); failure → 409 reject | none — restart required |
| Listener TLS store | shared service (`tls/store.rs`) | cert reload (`reload.rs:49`) + runtime reload rebuild | on reload / cert reload | write-lock poison → `ProxyError` (`store.rs:56,75`) | read-lock poison → silent empty/`None` (`store.rs:113,125`) |
| Control API endpoint | startup-bound, generation-config-driven | reload if bind changed (preflighted) | reload; bind-change preflighted (`reload.rs:361-368`) | preflight fail → 409 | none |
| Metrics exporter | startup-bound | reload if bind changed (preflighted `reload.rs:383-390`) | reload | preflight fail → 409 | none |
| Worker topology / threads | startup-owned | `performance.control_plane_threads` is restart-required (`reload.rs:416-470`) | startup only | change at reload → 409 restart-required | none |
| Backend resolution (DNS) | generation-owned store (`backend/store.rs`) | DNS refresh task | timer, if hostname backends exist | lookup/empty failure → retain old addrs (`lifecycle.rs:344-354`) | **silent retain**; generic failure metric only |
| Backend client rotation | transport pool | DNS refresh task after `Updated` | on resolution change | rotate `Err` → `client_rotated=false` | **silent**, stale conns until idle timeout (`lifecycle.rs:386-396`) |
| Backend health / membership | `UpstreamPool` (not store) | health-check task + request feedback | timer + live traffic | pool write-lock poison → drop update (`lifecycle.rs:278,317`) | **silent drop**; not transferred across reload (reset to `Unknown`) |
| Watchdog coordinator | shared service (`generation.rs:43`) | workers (`mark_*`) + service (`request_restart`/`complete_restart_cycle`) | continuous | mutex poison → `into_inner()` + `warn!` (`coordinator.rs:165`) | graceful recover |
| Watchdog service task | generation-owned (`tasks.rs`) | control plane spawn / retire | startup + per reload | restart command fail → `error!`, stays pending, retry | detection-only if `restart_command` unset |
| Generation background tasks | generation-owned registry (`tasks.rs`) | control plane | startup + per reload | retire timeout 5 s → `warn!` + continue (`tasks.rs:69-77`) | proceed regardless |
| Drain state (`draining`/`drain_start`) | per-`QUICListener` (`&mut self`) | owning worker/shard thread | during drain/shutdown | force-close on `drain_timeout` (`shutdown.rs:34-39`) | busy-spin until complete/deadline |

---

## 7. Panic-prone surfaces table (operational scope, non-test)

After excluding all `#[cfg(test)]` blocks / `tests.rs`, the operational control paths contain
only these. No `todo!`/`unimplemented!`/bare `.lock().unwrap()`/`.read().unwrap()`/
`.write().unwrap()` anywhere in non-test operational code.

| file | line | kind | context | trigger |
|---|---|---|---|---|
| `runtime/bundle.rs` | 100 | **panic (poison fallback)** | `.unwrap_or_else(\|_\| panic!("runtime bundle lock poisoned"))` in `current()` | runtime-condition — only if a prior panic poisoned the bundle `RwLock`; **hot read path** (every worker poll, every reload read, every backend snapshot). Asymmetric with `replace` which recovers gracefully. **Sharpest crash surface.** |
| `control_api/auth.rs` | 85 | unreachable | `Health \| Ready => unreachable!()` in `authorize_control_api_request` | impossible-by-construction — Health/Ready have `requires_authorization() == false` (`auth.rs:21`) so `auth.rs:69` returns early; only reachable if that predicate changes |
| `runtime/tasks.rs` | 23 | poison-fallback (graceful) | `Err(poisoned) => poisoned.into_inner()` in `register()` | runtime-condition — graceful recovery, no panic |
| `runtime/tasks.rs` | 35 | poison-fallback (graceful) | `Err(poisoned) => poisoned.into_inner()` in `begin_generation_retirement()` | runtime-condition — graceful recovery, no panic |
| `watchdog/coordinator.rs` | 165 | poison-fallback (graceful) | `Err(poisoned) => { warn!(…); poisoned.into_inner() }` in mutex helper | runtime-condition — graceful recovery + warn |

**Non-counted graceful lock sites** (degrade, don't panic): `health_check.rs:43`
(`match upstream_pool.read()`); `runtime/tls/store.rs:30,36,45,56,75,106,118`
(`.read().ok()` / `.write().map_err(…)`); all backend-module locks
(`store.rs:39,50,62,80`; `lifecycle.rs:168,264,278,317`). Saturating `try_into().unwrap_or(…)`
conversions (`startup.rs:58,302`; `backend_resolution.rs:36`; `health_check.rs:68`;
`watchdog/config.rs:44-59`) cannot panic.

**Lock-poisoning fallbacks in scope (Phase 0 exit-criterion list):**
1. `runtime/bundle.rs:100` — `current()` **panics** (the one to fix in Phase 5).
2. `runtime/bundle.rs:127-132` — `replace()` recovers gracefully → `ProxyError`.
3. `runtime/tasks.rs:23,35` — `into_inner()` graceful.
4. `watchdog/coordinator.rs:157-168` — `into_inner()` + warn, graceful.
5. `runtime/tls/store.rs:56,75` (write → `ProxyError`) and `:30-41,104-125` (read → silent `None`/empty).
6. Backend module (all silent-drop): `store.rs:39,50,62,80`; `lifecycle.rs:168,264,278,317`.

---

## 8. Reload rejection — every COMPUTE site and every FORMAT site

### Computed (decision to reject)
| site | condition | operator-triggerable? |
|---|---|---|
| `auth.rs:69` | auth failure | yes (missing/wrong token, or `auth_token` unset) |
| `auth.rs:97` | no matching route | yes (wrong path/method) |
| `reload.rs:103` | no runtime bundle handle | no (bootstrap-only state) |
| `reload.rs:106` | no current generation | no |
| `reload.rs:119-124` | `build_runtime_reload_plan` err (400 if starts with `"Configuration validation failed:"`/`"Runtime configuration normalization failed:"`, else 500) | yes (bad config on disk) |
| `reload.rs:135` | `validate_runtime_reload_plan` err → 409 | yes (incompatible change / preflight bind fail) |
| `reload.rs:281` | listener removed or bind changed | yes |
| `reload.rs:305,311` | QUIC listener preflight bind failed | yes (port conflict) |
| `reload.rs:322` | bootstrap TCP preflight failed | yes |
| `reload.rs:341` | no effective listeners for control-API TLS | yes |
| `reload.rs:354` | control-API TLS config missing | yes |
| `reload.rs:366` | control-API bind preflight failed (raw `probe_tcp_bind` string) | yes |
| `reload.rs:388` | metrics bind preflight failed (raw `probe_tcp_bind` string) | yes |
| `reload.rs:404` | startup-owned field changed (`log.file.*`, `log.format`, `observability.tracing.*`, `performance.control_plane_threads`; one per field, joined at `reload.rs:249`) | yes |
| `reload.rs:146-148` | `apply_runtime_reload_plan` err (e.g. lock poison) → 500 | no (commit-time) |
| `reload.rs:34,51` | cert-reload build/replace failure → 500 (separate `ReloadCerts` route) | yes |

### Formatted for operator (status + body)
| site | status | body |
|---|---|---|
| `auth.rs:77-80,88` | 401 | `{"reloaded": false, "error": "unauthorized"}` |
| `auth.rs:104-111` | 404 | `"not found\n"` (also reload `:104`) |
| `reload.rs:107-114` | 500 | `{"reloaded": false, "error": "runtime generation unavailable"}` |
| `reload.rs:126-133` | 400 / 500 | `{"reloaded": false, "error": <build-plan string>}` |
| `reload.rs:136-143` | 409 | `{"reloaded": false, "error": <compatibility reason string>}` (templates below) |
| `reload.rs:149-156` | 500 | `{"reloaded": false, "error": <ProxyError string, e.g. "runtime bundle lock poisoned">}` |
| `reload.rs:164-171` | 202 | `{"reloaded": true, "generation": …, "path": …}` (success) |
| `reload.rs:36-44,52-59` | 500 | `{"reloaded": false, "listener"?: …, "error": …}` (cert reload) |
| `render.rs:143-154` | fallback | on serialize/builder failure emits `{"error":"response"}` (loses intended status → default 200) |

409 reason-string templates (`reload.rs`):
- `"runtime reload rejected: listener '{}' was removed or its bind address changed; restart required"` (`:282`)
- `"runtime reload rejected: failed to preflight QUIC listener {}: {}"` (`:306`, `:312`)
- `"runtime reload rejected: failed to preflight bootstrap listener {}: {}"` (`:323`)
- `"runtime reload rejected: no effective listeners configured for control API TLS"` (`:342`)
- `"runtime reload rejected: control API TLS config missing for listener '{}'"` (`:355`)
- `"runtime reload rejected: {field} changed from {current:?} to {next:?}; restart required"` (`:405`)
- control-API / metrics bind preflight surface raw `"failed to bind {context} {bind}: {err}"` from `probe_tcp_bind` (`runtime_endpoint.rs:53`) at `:367` / `:389`.

---

## 9. Exit-criteria checklist

- [x] Every startup/reload/drain/shutdown entry point accounted for — §1, §2, §5.
  - startup: `app.rs:33`; reload: `reload.rs:98` (+ cert reload `reload.rs:84`); worker shutdown: `AtomicBool` at `runtime_state.rs:306` / `workers.rs:345,222`; bootstrap-listener shutdown: `bootstrap/listener.rs:100-116`; watchdog restart-drain: `coordinator.rs:76` / `listener.rs:89`; generation retirement: `bundle.rs:121`.
- [x] Every operator-visible reload rejection path listed — §8.
- [x] Every in-scope lock-poisoning fallback listed — §7 (list of 6).

## 10. Carry-forward for later phases (findings + resolution status)

Updated after Phases 1–10. `✅` = resolved; `↺` = intentionally deferred (see note).

1. ✅ **Poison asymmetry** — `bundle.rs` `current()` now recovers from lock poison via `PoisonError::into_inner()` with a documented safe-by-construction invariant; `replace()` still fails closed. (Phase 5)
2. ✅ **Silent backend fallbacks** — client-rotation failure is now an explicit `ClientRotationOutcome::Failed` with a `warn!` + exported `spooky_backend_client_rotation_failures_total` metric (Phase 4); every refresh outcome maps to a `BackendRefreshClassification` reporting traffic continuity (Phase 7). ↺ The DNS-retain and poisoned-pool-omission paths were classified as *acceptable explicit recovery* (they log + meter) and left as-is by design.
3. ✅ **Restart-required rules scattered** — centralized in `runtime::policy` (`RESOURCE_DOMAINS` + `ReloadCompatibilityAuthority`, Phase 2); wording standardized via structured `TransitionRejection` (Phase 8).
4. ✅ **Latent unused lifecycle enum surface** — the migration-era duplicate `TransitionRejection::resource_preparation_failed` constructor was removed (Phase 10). The `event.rs`/`update.rs` backend enums remain as the intended (if not-yet-driven) lifecycle vocabulary; the Phase-1 transition vocabulary (`TransitionPlan`, `RuntimeTransitionDecision`) is retained as the stable type layer the plan asked for and is unit-tested.
5. ↺ **Clock-skew stall detection** uses `SystemTime` (`watchdog/time.rs`). Not changed — the drain-grace deadline already uses monotonic `Instant`; converting the stall heartbeat is a watchdog-internal change outside the reload/drain/shutdown contract this plan hardened.
6. ↺ **Two hardcoded deadlines** — generation retirement 5 s and the drain busy-spin remain. The deterministic *lifecycle* is now modeled by `RuntimeLifecycleState` (Phase 6); making these deadlines configurable is a separate tuning change, not a correctness gap.
7. ↺ **`render.rs` fallback** drops intended status to 200 on serialize failure. Left as-is: it only triggers on failure to serialize internal structs (not operator input) and is out of the reload-rejection path Phase 8 standardized.

## 11. Final contract summary (post-refactor)

- **Ownership** is encoded in types: `StartupOwnedRuntimeState` / `RuntimeSharedServices` / `RuntimeGenerationState` each implement `OwnedRuntimeState` with an `OWNERSHIP` class; only generation-owned state is swappable (`runtime::generation`, Phase 3).
- **Reload compatibility** is answerable from one module: `runtime::policy::RESOURCE_DOMAINS` + `ReloadCompatibilityAuthority` (Phase 2).
- **Lifecycle** is a deterministic state machine: `RuntimeLifecycleState` (Starting→Running→Draining→ShuttingDown→Terminated) gates reload commits and makes drain/shutdown idempotent (Phase 6).
- **Failures** are typed and operator-actionable: `TransitionRejection` (reload), `BackendRefreshClassification` (backend), both stating whether the active runtime changed (Phases 7–8).
- **Panic surface** in operational paths is reduced to documented poison-recovery fallbacks only; no `unwrap`/`expect`/`panic!`/`unreachable!` remains in operator-controlled transition code (Phase 5, re-audited Phase 10).
- **Regression tests** cover live reload, restart-required rejection, generation preservation, drain idempotency, poison recovery, and backend-failure safety (Phase 9).
