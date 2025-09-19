use quinn::{Endpoint, ServerConfig};
use std::{net::SocketAddr, sync::Arc};
use rustls;
use quinn::crypto::rustls::QuicServerConfig;

use crate::config::config::Config;

pub struct Server {
    pub endpoint: Endpoint,
    pub config: Config,
}

impl Server {
    pub async fn new(config: Config) -> Result<Self, Box<dyn std::error::Error>> {
        // Install default crypto provider for Rustls
        let _ = rustls::crypto::CryptoProvider::install_default(
            rustls::crypto::ring::default_provider()
        );

        let (certs, key) = crate::utils::tls::load_tls(
            &config.listen.tls.cert,
            &config.listen.tls.key
        );

        // Create TLS config with ALPN support and explicit crypto provider
        let crypto_provider = rustls::crypto::ring::default_provider();

        let mut tls_config = rustls::ServerConfig::builder_with_provider(crypto_provider.into())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("Failed to create TLS config");

        // Set ALPN protocols - HTTP/3 uses "h3"
        tls_config.alpn_protocols = vec![b"h3".to_vec()];

        let mut server_config = ServerConfig::with_crypto(Arc::new(
            QuicServerConfig::try_from(tls_config)
                .expect("Failed to create QUIC server config"),
        ));

        let addr: SocketAddr = format!("{}:{}", config.listen.address, config.listen.port)
            .parse()
            .expect("Invalid Listen address");

        server_config.transport = Arc::new(quinn::TransportConfig::default());

        let endpoint = Endpoint::server(server_config, addr)?;

        Ok(Server { endpoint, config })
    }

    pub async fn run(&self) {
        println!("Proxy listening on 0.0.0.0:{}", self.config.listen.port);

        while let Some(connecting) = self.endpoint.accept().await {
            println!("Got a new connection request!");
            println!("From: {}", connecting.remote_address());

            tokio::spawn(handle_connection(connecting));
        }
    }
}

async fn handle_connection(connecting: quinn::Incoming) {
    match connecting.await {
        Ok(connection) => {
            println!("New QUIC Connection: {:?}", connection.remote_address());
            process_connection(connection).await;
        }
        Err(e) => eprintln!("Connection failed: {}", e),
    }
}

async fn process_connection(connection: quinn::Connection) {
    use h3::{server};
    use bytes::Bytes;
    use h3_quinn::Connection as H3QuinnConnection;

    let h3_connection = H3QuinnConnection::new(connection);

    let mut h3_server: h3::server::Connection<_, Bytes> = server::builder()
        .max_field_section_size(8192)
        .build(h3_connection)
        .await
        .expect("Failed to build h3 server");

    loop {
        match h3_server.accept().await {
            Ok(Some(req_resolver)) => {
                match req_resolver.resolve_request().await {
                    Ok((req, mut _stream)) => {
                        println!("Got request: {:?}", req);

                        // let backend_addr: SocketAddr = "127.0.0.1:7777".parse().unwrap();
                        // let endpoint = Endpoint::client("[::]:0".parse().unwrap()).unwrap();

                        // let backend_connection = endpoint
                        //     .connect(backend_addr, "localhost")
                        //     .unwrap()
                        //     .await
                        //     .expect("connection to backend failed")

                        // let h3_backend = H3QuinnConnection::new(backend_connection);
                        // let (mut h3_client, driver) = client::builder().build(h3_backend).await.unwrap();
                        // tokio::spawn(driver); // run client driver

                        // // --- forward request to backend ---
                        // let (mut backend_resp, mut backend_body) =
                        //     h3_client.send_request(req, None).await.unwrap();

                        // // --- send backend response back to original client ---
                        // stream.send_response(backend_resp).await.unwrap();

                        // while let Some(chunk) = backend_body.recv_data().await.unwrap() {
                        //     stream.send_data(chunk).await.unwrap();
                        // }
                    
                        // stream.finish().await.unwrap();

                        println!("Here load balancing strategy and select backe and redirect traffic")
                    }
                    Err(e) => {
                        eprintln!("Failed to resolve request: {e:?}");
                        break;
                    }
                }
            }
            Ok(None) => {
                println!("Connection closed gracefully");
                break;
            }
            Err(e) => {
                println!("Connection closed: {e:?}");
                break;
            }
        }
    }
}
