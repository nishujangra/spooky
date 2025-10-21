# References

This is the list of references we have taken to build this project.

## Protocol RFCs

1. [RFC 9114 HTTP/3](https://www.rfc-editor.org/rfc/rfc9114.html)
2. [RFC 9000 QUIC: A UDP-based Multiplexed and secure transport](https://www.rfc-editor.org/rfc/rfc9000.html)

## Core Dependencies
1. [quinn](https://crates.io/crates/quinn/0.11.9) - QUIC transport implementation
2. [tokio](https://crates.io/crates/tokio) - Async runtime
3. [serde](https://crates.io/crates/serde) - Serialization framework
4. [serde_yaml](https://crates.io/crates/serde_yaml/0.9.34) - YAML serialization
5. [clap](https://crates.io/crates/clap) - Command line argument parser
6. [rustls](https://crates.io/crates/rustls) - TLS implementation
7. [rustls-pki-types](https://crates.io/crates/rustls-pki-types) - TLS certificate types
8. [h3](https://crates.io/crates/h3/0.0.8) - HTTP/3 implementation
9. [h3-quinn](https://crates.io/crates/h3-quinn/0.0.10) - HTTP/3 over QUIC
10. [http](https://crates.io/crates/http/1.3.1) - HTTP types and traits
11. [bytes](https://crates.io/crates/bytes) - Byte utilities
12. [rand](https://crates.io/crates/rand) - Random number generation
13. [log](https://crates.io/crates/log/0.4.28) - Logging facade
14. [env_logger](https://crates.io/crates/env_logger/0.11.3) - Environment-based logger

## Documentations
1. [serde_yaml Documentation](https://docs.rs/serde_yaml/) - YAML serialization in Rust
2. [quinn Documentation](https://docs.rs/quinn/) - QUIC transport protocol implementation
3. [rustls Documentation](https://docs.rs/rustls/) - TLS implementation in Rust
4. [h3 Documentation](https://docs.rs/h3/) - HTTP/3 implementation
5. [tokio Documentation](https://docs.rs/tokio/) - Asynchronous runtime for Rust
6. [clap Documentation](https://docs.rs/clap/) - Command line argument parser
7. [HTTP/3 Implementation in Rust: Performance Tuning for Global Web Services](http://markaicode.com/http3-rust-implementation-performance-tuning/)

## Load Balancing Algorithms
1. [Load Balancing Algorithms](https://kemptechnologies.com/load-balancer/load-balancing-algorithms-techniques/)
2. [Consistent Hashing](https://en.wikipedia.org/wiki/Consistent_hashing)
3. [Least Connections Algorithm](https://www.nginx.com/resources/glossary/least-connections-load-balancing/)
