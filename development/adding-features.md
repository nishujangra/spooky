# Adding Features Safely

Use this workflow whenever a change adds or alters a feature.

## 1. Decide The Ownership Boundary

Before writing code, decide which crate owns the feature.

Typical split:

- config and validation: `crates/config`
- request lifecycle and runtime behavior: `crates/edge`
- request translation: `crates/bridge`
- upstream transport: `crates/transport`
- balancing and backend health: `crates/lb`

## 2. Update The Config Surface Carefully

If the feature is configurable:

1. add schema fields in `crates/config/src/config.rs`
2. add defaults in `crates/config/src/default.rs` when appropriate
3. add validation in `crates/config/src/validator.rs`
4. add runtime projection in `crates/config/src/runtime.rs`
5. document runtime precedence and caveats

## 3. Add Runtime Logic

- keep changes inside the narrowest responsible module
- avoid expanding central modules unless there is no smaller boundary available
- preserve the invariants documented in [Invariants](invariants.md)

## 4. Add Tests

- unit test the local logic
- add integration tests when the behavior is externally visible
- update benchmark coverage if the feature changes hot-path behavior materially

## 5. Update Metrics And Docs

- expose new operator-relevant behavior through metrics when appropriate
- update docs before considering the feature complete
- state whether the feature is production-ready, partial, or experimental
