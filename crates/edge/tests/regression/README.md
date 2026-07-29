## Edge Regression Boundaries

This directory is for stable edge behavior regressions that are easiest to express through the
crate's public or externally visible contracts without booting the full listener runtime.

Keep tests here when the bug is about:
- stable hash or request-key derivation contracts
- Prometheus exposition shape and metric rendering contracts
- other edge-visible output that is best asserted without a live QUIC/bootstrap harness

Representative regressions already pinned elsewhere in the edge test tree:
- request-body guardrails returning `413` instead of accidentally surfacing `503`
- long streamed responses surviving total-timeout semantics after forward progress
- control-plane runtime-swap snapshots exposing the active listener label
- backend failure and partial-outage availability behavior through lifecycle-aware integration suites

Do not add tests here when the bug depends on runtime orchestration, live reload, backend health,
bootstrap-vs-QUIC parity, or full request execution. Those belong in the dedicated integration
files under `crates/edge/tests/`, where the runtime harness already exists:
- `h3_bridge.rs`
- `bootstrap_quic_parity.rs`
- `runtime_swap.rs`
- `backend_failure_and_recovery.rs`
- related support-backed integration suites

Rule of thumb: if the regression is about a live listener, backend lifecycle, or end-to-end request
path, keep it in the integration suite that already owns that contract. If it is about a stable
edge-facing output contract, keep it here.
