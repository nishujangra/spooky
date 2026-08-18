# Terminology

This page standardizes the core terms used across the docs.

Use these definitions and preferred phrases consistently in product-reference pages.

| Term | Meaning |
| --- | --- |
| Listener | A downstream ingress socket and TLS identity definition. A listener owns an address, port, protocol, and certificate set. |
| Bootstrap TLS listener | The compatibility ingress path used for downstream HTTP/1.1 and HTTP/2 on the listener address/port. |
| Upstream | A named routing target consisting of route rules, optional TLS policy, load-balancing policy, and one or more backends. |
| Backend | A single origin endpoint inside an upstream. |
| Route | The match conditions that decide which upstream handles a request. |
| Admission | The shared request-policy gate that evaluates route-level rejection, scoped rate limits, quota, brownout, overload, and permit checks before backend execution. |
| Auth | Request-path authentication and authorization decisions, including local auth and external auth integration. |
| Quota | Contract-style request limiting based on explicit selectors and windows. Quota is separate from overload control. |
| Overload | Runtime protection behavior that sheds or delays work to keep the system stable under pressure. |
| Brownout | A specific overload mode in which non-core or lower-priority traffic is shed to preserve core traffic. |
| Control API | The privileged operator-facing admin HTTP surface. |
| Control plane | The broader operator-facing management layer around the Control API, runtime views, watchdog, metrics endpoint, and related admin services. |
| Audit | The operator-facing record of control API actions and runtime-management events. |
| Runtime generation | One active version of the normalized runtime state. New generations are produced during validated runtime activation and swap into service atomically. |
| Metrics endpoint | The Prometheus exposition endpoint. |
| Drain | The process of stopping new admissions while allowing existing work to complete or time out. |
| Cert reload | Reloading listener certificate material for new handshakes only. This is not full config reload. |

## Preferred Usage

Use these phrases consistently unless a page is quoting a metric name, config key, or code identifier.

| Prefer | Avoid when possible | Reason |
| --- | --- | --- |
| `listener` | `proxy listener`, `socket listener` | `listener` is the canonical runtime term. |
| `route` | `routing rule` for the main noun | `route` is shorter and already defined. |
| `upstream` | `upstream pool` as the default term | `upstream` is the canonical routing target; mention its backends when needed. |
| `backend` | `origin server` unless external comparison requires it | `backend` matches config, metrics, and runtime views. |
| `admission` | `admission control` everywhere | use `admission` as the general subsystem name; use `admission control` only when the longer phrase materially helps. |
| `auth` | mixed `authentication` / `authorization` wording for shared request-path decisions | `auth` is the shortest shared label for the request-path decision layer. |
| `quota` | `rate limiting` when the feature is specifically quota | quota has different semantics from scoped rate limiting. |
| `overload` | `rate limiting` or `quota` when the meaning is runtime protection | keep contract failure and runtime protection separate. |
| `brownout` | vague phrases like `degraded mode` when the specific overload mode is meant | brownout is a distinct runtime behavior. |
| `Control API` | `admin API` as the primary product term | `Control API` is the canonical product surface. |
| `control plane` | `control-plane services` for the general system every time | use `control plane` for the broader management layer; use `Control API` for the HTTP admin surface. |
| `audit` | `audit log` as the only term | `audit` covers the event stream and operator history more cleanly. |
| `runtime generation` | `generation` alone on first mention | spell out the full phrase on first mention for clarity. |

## Writing Style

Use these style rules across product-reference pages:

- Prefer short, direct sentences.
- Use the canonical term on first mention, then keep using the same term.
- Separate request-path policy concepts from runtime protection concepts.
- Use `Control API` for the admin HTTP surface and `control plane` for the broader operator layer.
- Use `upstream` for the named routing target and `backend` for individual endpoints inside it.
- Avoid mixing code names, config keys, and reader-facing terms in ordinary prose unless needed for precision.
