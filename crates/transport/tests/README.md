## Transport Test Boundaries

The transport crate keeps protocol-hiding regressions as crate-local facade tests instead of a
separate `tests/regression/` subtree.

Keep tests in this directory when the bug is about:
- canonical transport facade request execution
- H1/H2 protocol hiding from callers
- unified client rotation semantics
- transport-boundary timeout ownership
- stable transport-side error and overload mapping

Representative regressions already pinned here:
- caller-visible protocol branching leaking out of the transport facade
- H1/H2 client rotation returning different observable contracts
- unknown-backend, overload, and send-failure mapping drifting across protocol implementations

Do not add edge request-path policy tests here. Admission, auth, retry, response normalization, and
listener/runtime behavior belong in edge or bridge suites unless the regression is truly at the
transport facade boundary.

Rule of thumb: if the caller should be able to treat transport as one backend-execution surface,
the regression belongs here.
