# Development

This markdown file is about the development of this project.

## What we are doing?

We are using rust for its speed and will built this load balancer on the HTTP3/QUIC protocols
 


## Folder Structure

```text
spooky/
├── Cargo.toml              # Main manifest
├── Cargo.lock
├── config.yaml             # Main configuration file
├── config.sample.yaml      # Sample configuration
├── certs/                  # TLS certificates
│   ├── cert.der
│   └── key.der
│
├── src/
│   ├── main.rs             # Main entry point
│   ├── bin/
│   │   └── server.rs       # Server binary entry point
│   ├── config/             # Configuration management
│   │   ├── mod.rs
│   │   ├── config.rs       # Configuration structures
│   │   ├── default.rs      # Default configuration values
│   │   └── validator.rs    # Configuration validation logic
│   ├── proxy/              # HTTP/3 server implementation
│   │   └── mod.rs          # Main proxy server logic
│   ├── lb/                 # Load balancing strategies
│   │   ├── mod.rs
│   │   └── random.rs       # Random selection strategy (basic implementation)
│   ├── utils/              # Utility functions
│   │   ├── mod.rs
│   │   └── tls.rs          # TLS certificate loading utilities
│   └── health/             # Health checking (planned)
│       ├── mod.rs
│       ├── checker.rs
│       └── monitor.rs
│
├── docs/                   # Project documentation
│   ├── development.md      # This file
│   ├── roadmap.md
│   ├── protocols/
│   │   ├── http3.md
│   │   └── quic.md
│   ├── dev/
│   │   └── h3-server.md
│   └── gen-cert.md         # Certificate generation guide
│
├── target/                 # Build artifacts
├── LICENSE.md
├── README.md
└── COPYRIGHT.md
```

### Current Implementation Status

**✅ Implemented:**
- Basic HTTP/3 server with QUIC ✅
- Configuration management (YAML) ✅
- TLS certificate handling ✅
- Random load balancing strategy (basic implementation) ✅
- Logging infrastructure ✅
- CLI argument parsing ✅
- Configuration validation ✅

**🔄 In Progress:**
- Load balancing strategy implementation (backend selection logic) 🔄
- Backend server management and configuration 🔄
- Request forwarding logic to backend servers 🔄
- QUIC client connections for backend communication 🔄

**📋 Planned:**
- Additional load balancing strategies (round-robin, least connections, etc.)
- Health checking system for backend servers
- Metrics and monitoring endpoints
- Configuration hot-reload capability
- Graceful shutdown handling
- Integration tests
- Performance benchmarks
- Circuit breaker pattern for unhealthy backends
- Request/response transformation capabilities
