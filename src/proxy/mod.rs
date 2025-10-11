//! HTTP/3 Load Balancer Server Implementation
//! 
//! TODO: Implement proper backend server selection using configured strategy
//! TODO: Add health check monitoring for backend servers
//! TODO: Implement request forwarding to selected backend
//! TODO: Add response aggregation and error handling
//! TODO: Implement connection pooling for backend servers
//! TODO: Add metrics collection for load balancing decisions
//! TODO: Handle backend server failures and failover
//! TODO: Add request/response transformation capabilities
//! TODO: Implement graceful shutdown handling
//! TODO: Add circuit breaker pattern for unhealthy backends

use quinn::{Endpoint, ServerConfig};
use std::{net::SocketAddr, sync::Arc};
use rustls;

use rand::seq::SliceRandom; // for choose()
use rand::thread_rng;

use quinn::crypto::rustls::QuicServerConfig;
use log::{info, warn, debug, trace, error};

use crate::config::config::{Backend, Config, HealthCheck};
use crate::utils::tls::load_tls;

pub struct Server {
    pub endpoint: Endpoint,
    pub config: Config,
}

impl Server {
    pub async fn new(config: Config) -> Result<Self, Box<dyn std::error::Error>> {
        debug!("Initializing server with config: {:?}", config);
        
        // Install default crypto provider for Rustls
        trace!("Installing default crypto provider for Rustls");
        let _ = rustls::crypto::CryptoProvider::install_default(
            rustls::crypto::ring::default_provider()
        );

        debug!("Loading TLS certificates from: cert={}, key={}", 
               config.listen.tls.cert, config.listen.tls.key);
        let (certs, key) = load_tls(
            &config.listen.tls.cert,
            &config.listen.tls.key
        );

        // Create TLS config with ALPN support and explicit crypto provider
        trace!("Creating TLS configuration");
        let crypto_provider = rustls::crypto::ring::default_provider();

        let mut tls_config = rustls::ServerConfig::builder_with_provider(crypto_provider.into())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("Failed to create TLS config");

        // Set ALPN protocols - HTTP/3 uses "h3"
        debug!("Setting ALPN protocols to 'h3' for HTTP/3");
        tls_config.alpn_protocols = vec![b"h3".to_vec()];

        trace!("Creating QUIC server configuration");
        let mut server_config = ServerConfig::with_crypto(Arc::new(
            QuicServerConfig::try_from(tls_config)
                .expect("Failed to create QUIC server config"),
        ));

        let addr: SocketAddr = format!("{}:{}", config.listen.address, config.listen.port)
            .parse()
            .expect("Invalid Listen address");

        debug!("Server will listen on: {}", addr);
        server_config.transport = Arc::new(quinn::TransportConfig::default());

        let endpoint = Endpoint::server(server_config, addr)?;
        info!("Server endpoint created successfully");

        Ok(Server { endpoint, config })
    }

    pub async fn run(&self) {
        info!("Proxy listening on {}:{}", self.config.listen.address, self.config.listen.port);
        info!("Load balancing strategy: {}", self.config.load_balancing.lb_type);
        info!("Backend servers: {}", self.config.backends.len());

        while let Some(connecting) = self.endpoint.accept().await {
            debug!("New connection request from: {}", connecting.remote_address());
            tokio::spawn(handle_connection(connecting));
        }
    }
}

async fn handle_connection(connecting: quinn::Incoming) {
    match connecting.await {
        Ok(connection) => {
            info!("New QUIC Connection established: {}", connection.remote_address());
            process_connection(connection).await;
        }
        Err(e) => {
            error!("Connection failed: {}", e);
        }
    }
}

async fn process_connection(connection: quinn::Connection) {
    use h3::{server};
    use bytes::Bytes;
    use h3_quinn::Connection as H3QuinnConnection;

    trace!("Creating H3 connection from QUIC connection");
    let h3_connection = H3QuinnConnection::new(connection);

    let mut h3_server: h3::server::Connection<_, Bytes> = server::builder()
        .max_field_section_size(8192)
        .build(h3_connection)
        .await
        .expect("Failed to build h3 server");

    debug!("H3 server connection established, waiting for requests");

    loop {
        match h3_server.accept().await {
            Ok(Some(req_resolver)) => {
                match req_resolver.resolve_request().await {
                    Ok((req, mut _stream)) => {
                        info!("Received HTTP/3 request: {} {}", req.method(), req.uri());
                        debug!("Request headers: {:?}", req.headers());

                        // TODO: Use configured load balancing strategy instead of hardcoded random
                        // TODO: Get backends from config instead of hardcoded list
                        // TODO: Implement proper backend selection logic
                        // TODO: Add backend health status checking before selection
                        // TODO: Handle case when no healthy backends are available
                        
                        // choose backend server
                        // Pick random backend
                        let backends = vec![
                            Backend { 
                                id: "backend1".into(), 
                                address: "127.0.0.1:7001".into(), 
                                weight: 100, 
                                health_check: HealthCheck{
                                    path: "/health".into(),
                                    interval: "5s".into(),
                                } 
                            },
                            Backend { 
                                id: "backend2".into(), 
                                address: "127.0.0.1:7002".into(), 
                                weight: 100, 
                                health_check: HealthCheck{
                                    path: "/health".into(),
                                    interval: "5s".into(),
                                } 
                            },

                        ];

                        let mut rng = thread_rng();
                        if let Some(random_backend) = backends.choose(&mut rng) {
                            println!("Selected backend address: {}", random_backend.address);
                        } else {
                            println!("No backends available!");
                        }

                        // TODO: Implement actual request forwarding to selected backend
                        // TODO: Add HTTP/3 client connection to backend
                        // TODO: Forward request headers and body to backend
                        // TODO: Stream response back to client
                        // TODO: Handle backend connection failures
                        // TODO: Add timeout handling for backend requests
                        // TODO: Implement retry logic for failed requests
                        // TODO: Add request/response logging and metrics

                        // transfer load to that server
                        // give response

                        // // --- establish backend connection ---
                        // let backend_addr: SocketAddr = "127.0.0.1:7001".parse().unwrap();
                        // let endpoint = Endpoint::client("[::]:0".parse().unwrap()).unwrap();

                        // let quinn_conn = endpoint
                        //     .connect(backend_addr, "localhost")
                        //     .unwrap()
                        //     .await
                        //     .expect("connection to backend failed");


                        // let h3_backend = H3QuinnConnection::new(quinn_conn);
                        // let (mut h3_client, driver) = client::builder()
                        //     .max_field_section_size(8192)
                        //     .build(h3_backend)
                        //     .await
                        //     .unwrap();
                    },
                    Err(e) => {
                        error!("Error: {e:?}");
                        break;
                    }
                }
            }
            Ok(None) => {
                debug!("Connection closed gracefully");
                break;
            }
            Err(e) => {
                warn!("Connection closed with error: {e:?}");
                break;
            }
        }
    }
}
