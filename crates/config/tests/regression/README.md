## Config Regression Boundaries

This directory is the regression suite for raw-config to runtime-config lowering.

Keep tests here when the bug is about:
- listener precedence and listener normalization
- timeout normalization and validation
- transport, auth, admission, rate-limit, backend, TLS, or load-balancing runtime shaping
- reloadable versus startup-owned config interpretation

Representative regressions already covered here:
- `listen` versus `listeners[]` precedence
- timeout ordering and nonzero validation
- backend health-check zero-value rejection
- runtime shaping for auth, admission, rate limits, and alternate-backend policy

These tests should stay on the canonical public lowering path:
- `RuntimeConfig::from_config`

Do not move crate-internal helper behavior here if the contract is clearer beside the owning
runtime interpreter module. Internal shaping details that require private helpers should stay as
unit tests under `crates/config/src/runtime/`.

Rule of thumb: if the regression is visible in the runtime config produced from raw config, keep it
here. If it only exists because of a private helper or intermediate normalization detail, test it
next to the owning module.
