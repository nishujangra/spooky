## Bridge Regression Boundaries

This directory owns request-shaping regressions for the canonical bridge surface.

Keep tests here when the bug is about:
- canonical request-builder inputs flowing into H1/H2 request shapes
- host policy or forwarded-header policy application
- WebSocket and upgrade request shaping
- H1/H2 request parity for the same logical bridge input

Representative regressions already covered here:
- forwarded-header and spoofed-header stripping parity
- canonical host rewrite versus preserve behavior
- WebSocket upgrade request shaping staying stable across H1/H2 builder paths

Do not force response-normalization regressions into this directory when the contract is clearer on
the unit-test side of the bridge crate. Response normalization is owned by the canonical normalizer
entrypoints and should stay near that implementation when the bug is not specifically about
cross-protocol request shaping.

Rule of thumb: request construction and ingress-to-upstream shaping regressions live here;
response-normalization and lower-level helper regressions stay next to the owning bridge module if
that gives a clearer contract.
