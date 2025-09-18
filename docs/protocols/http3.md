# HTTP/3 Overview

- HTTP semantics are widely used, traditionally over **HTTP/1.1** and **HTTP/2**.  
- **HTTP/3** runs over **QUIC**, a modern transport protocol.

---

## Prior Versions of HTTP
- **HTTP/1.1**:  
  - Text-based, whitespace-delimited messages (human-readable but parsing-heavy).  
  - No multiplexing → multiple TCP connections needed, harming congestion control.  

- **HTTP/2**:  
  - Introduced **binary framing** and **multiplexing**.  
  - Still TCP-based → packet loss or reordering stalls all streams (head-of-line blocking).  

### Delegation to QUIC
- QUIC provides:  
  - Stream multiplexing with independent reliability and flow control.  
  - Congestion control across the connection.  
  - Built-in **TLS 1.3** for security and faster setup.  
- **HTTP/3** builds on HTTP/2’s design but maps HTTP semantics over QUIC.  

---

## HTTP/3 Protocol Overview
- **Transport**: Uses QUIC with stream multiplexing and flow control.  
- **Communication**:  
  - Each HTTP/3 stream carries **frames** (e.g., `HEADERS`, `DATA`).  
  - A **control stream** manages connection-wide frames.  
- **Request/Response**:  
  - Each pair uses a single QUIC stream.  
  - Independent streams avoid blocking from packet loss.  
- **Server Push**: Supported via frames like `PUSH_PROMISE`, `MAX_PUSH_ID`, `CANCEL_PUSH`.  
- **Field Compression**:  
  - Replaces **HPACK** (HTTP/2) with **QPACK**, designed for QUIC’s unordered delivery.  

---

## Sample Headers

### HTTP/1.1 (Text-based headers)

```http
GET /index.html HTTP/1.1
Host: example.com
User-Agent: Mozilla/5.0
Accept: text/html
Connection: keep-alive
```

### HTTP/2

```http
(Frame Layer Example)

HEADERS Frame:
  :method = GET
  :path = /index.html
  :scheme = https
  :authority = example.com
  user-agent = Mozilla/5.0
  accept = text/html
```

### HTTP/3

```http
(QPACK Encoded Headers Example)

HEADERS Frame (QPACK-encoded):
  :method = GET
  :path = /index.html
  :scheme = https
  :authority = example.com
  user-agent = Mozilla/5.0
  accept = text/html
```

* Very similar to HTTP/2’s header format (uses pseudo-headers).
* Key difference: uses QPACK (not HPACK) because QUIC streams can arrive out of order.
* Runs on QUIC instead of TCP.

---

## HTTP/2 vs HTTP/3

| Feature               | **HTTP/2**                                | **HTTP/3**                         |
| --------------------- | ----------------------------------------- | ---------------------------------- |
| Transport             | TCP                                       | QUIC (over UDP)                    |
| Header Compression    | **HPACK**                                 | **QPACK**                          |
| Head-of-Line Blocking | Affects all streams due to TCP            | Avoided (independent QUIC streams) |
| Header Syntax         | Pseudo-headers (`:method`, `:path`, etc.) | Same syntax as HTTP/2              |
