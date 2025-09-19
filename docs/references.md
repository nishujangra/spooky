# References

This is the list of references we have taken to build this project.

## Protocol RFCs

1. [RFC 9114 HTTP/3](https://www.rfc-editor.org/rfc/rfc9114.html)
2. [RFC 9000 QUIC: A UDP-based Multiplexed and secure transport](https://www.rfc-editor.org/rfc/rfc9000.html)

## Core Dependencies
1. [quinn](https://crates.io/crates/quinn) - QUIC transport implementation
2. [tonic](https://crates.io/crates/tonic) - gRPC over HTTP/2
3. [tokio](https://crates.io/crates/tokio) - Async runtime
4. [serde_yaml](https://crates.io/crates/serde_yaml) - YAML serialization
5. [anyhow](https://crates.io/crates/anyhow) - Error handling
6. [rustls](https://crates.io/crates/rustls) - TLS implementation
7. [h3](https://crates.io/crates/h3)
8. [h3_quinn](https://crates.io/crates/h3-quinn)
9. [http](https://crates.io/crates/http)

## Documentations
1. [Read YAML in Rust](https://rust.code-maven.com/yaml/) 
2. [ServerConfig](https://docs.rs/quinn/latest/quinn/struct.ServerConfig.html#method.with_single_cert)
3. [PrivateKeyDer](https://docs.rs/rustls-pki-types/1.12.0/rustls_pki_types/enum.PrivateKeyDer.html)
4. [PrivatePkcs8KeyDer](https://docs.rs/rustls-pki-types/latest/rustls_pki_types/struct.PrivatePkcs8KeyDer.html)
5. [CertificateDer](https://docs.rs/rustls-pki-types/1.12.0/rustls_pki_types/struct.CertificateDer.html)

## Load Balancing Algorithms
1. [Load Balancing Algorithms](https://kemptechnologies.com/load-balancer/load-balancing-algorithms-techniques/)
2. [Consistent Hashing](https://en.wikipedia.org/wiki/Consistent_hashing)
3. [Least Connections Algorithm](https://www.nginx.com/resources/glossary/least-connections-load-balancing/)
