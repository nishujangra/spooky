## LB Test Boundaries

The load-balancing crate keeps its regression and contract coverage as crate-local integration
tests instead of a separate `tests/regression/` subtree.

Keep tests in this directory when the bug is about:
- strategy behavior such as round robin, random, consistent hash, least-connections, or
  latency-aware selection
- runtime-upstream to pool construction
- healthy-only selection semantics
- alternate-backend selection substrate behavior
- canonical request-key extraction owned by the balancing layer

Representative regressions already pinned here:
- round-robin sequencing drifting back to the first backend
- strategy normalization through the canonical runtime pool surface
- healthy-membership filtering for strategy picks

Do not add edge orchestration or backend lifecycle behavior here. If the regression requires live
runtime health transitions, DNS refresh, request feedback, or listener behavior, it belongs in the
edge lifecycle or request-path suites.

Rule of thumb: if the bug is about balancing substrate behavior independent of edge orchestration,
keep it here.
