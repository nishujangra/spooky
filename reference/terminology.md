# Terminology

This page standardizes the core terms used across the docs.

| Term | Meaning |
| --- | --- |
| Listener | A downstream ingress socket and TLS identity definition. A listener owns an address, port, protocol, and certificate set. |
| Bootstrap TLS listener | The compatibility ingress path used for downstream HTTP/1.1 and HTTP/2 on the listener address/port. |
| Upstream | A named routing target consisting of route rules, optional TLS policy, load-balancing policy, and one or more backends. |
| Backend | A single origin endpoint inside an upstream pool. |
| Route | The match conditions that decide which upstream handles a request. |
| Control API | The privileged operator-facing admin HTTP surface. |
| Metrics endpoint | The Prometheus exposition endpoint. |
| Drain | The process of stopping new admissions while allowing existing work to complete or time out. |
| Cert reload | Reloading listener certificate material for new handshakes only. This is not full config reload. |
