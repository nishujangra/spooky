use quinn::{Endpoint, ServerConfig};
use std::{net::SocketAddr, sync::Arc};
use rustls;

use quinn::crypto::rustls::QuicServerConfig;
use log::{info, debug, trace };

use crate::config::config::{Config};
use crate::utils::tls::load_tls;



use super::Server;

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

            let server = self.clone(); // so tokio spawn task can own it

            tokio::spawn(async move {
                server.handle_connection(connecting).await;
            });
        }
    }
}