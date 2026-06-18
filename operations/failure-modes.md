# Failure Modes And Status Codes

This page documents the major operator-visible failure classes and how they typically surface.

## 400

Typical reason:

- malformed or unsupported request semantics

Examples:

- upgrade-style request semantics on paths that do not support them
- invalid request shape after validation

## 403

Typical reason:

- policy denied the request path

## 405

Typical reason:

- method policy blocked the request

## 408

Typical reason:

- client request body stalled beyond the configured idle timeout

## 413

Typical reason:

- request body exceeded configured request-body limits

## 500-Class Upstream Responses

Typical reason:

- upstream returned a genuine 5xx response

Interpretation:

- this is usually a backend behavior signal, not necessarily a proxy-generated fault

## 502

Typical reason:

- upstream transport or bridge-level failure before a valid backend response could be served

## 503

Typical reasons:

- overload shedding
- queue-cap overflow
- upstream timeout
- response-body cap breach
- temporary backend unavailability

Interpretation:

- 503 in Spooky can mean either self-protection or upstream failure insulation
- always inspect the body text, logs, and overload metrics before assuming one cause

## Silent Drop Behaviors

Some invalid or non-actionable traffic is dropped rather than converted into a rich HTTP response.

Examples include:

- malformed packet cases before a request lifecycle exists
- new connection attempts during draining
- packets for unknown connections in certain QUIC states

## Stream Reset Versus HTTP Error Response

Spooky deliberately distinguishes between:

- returning a terminal HTTP response such as 408, 413, or 503
- resetting a stream when transport semantics or teardown behavior require it

This distinction matters for clients and should be considered during incident analysis.
