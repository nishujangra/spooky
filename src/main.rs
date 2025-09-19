// proxy http3 server QUIC + HTTP/3

use h3::server;
use quinn::{Endpoint, ServerConfig};
use std::{fs, net::SocketAddr, sync::Arc};

use bytes::Bytes;

use h3_quinn::Connection as H3QuinnConnection;

pub mod config;
pub mod utils;

use config::config::Config;
use utils::tls::load_tls;

fn read_config() -> Config {
    let filename: String = String::from("config.yaml");
    let text = fs::read_to_string(filename).unwrap();

    let data: Config = serde_yaml::from_str(&text).unwrap_or_else(|err| {
        eprintln!("Could not parse YAML file: {err}");
        std::process::exit(1);
    });

    return data;
}

#[tokio::main]
async fn main() {
    // Install default crypto provider for Rustls
    let _ = rustls::crypto::CryptoProvider::install_default(
        rustls::crypto::ring::default_provider()
    );

    let config_yaml = read_config();

    let (certs, key) = load_tls(
        &config_yaml.listen.tls.cert,
        &config_yaml.listen.tls.key
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
        quinn::crypto::rustls::QuicServerConfig::try_from(tls_config)
            .expect("Failed to create QUIC server config"),
    ));

    let addr: SocketAddr = format!("{}:{}", config_yaml.listen.address, config_yaml.listen.port)
        .parse()
        .expect("Invalid Listen address");

    server_config.transport = Arc::new(quinn::TransportConfig::default());

    let server_endpoint = Endpoint::server(server_config, addr).unwrap();

    println!("Proxy listening on 0.0.0.0:{}", config_yaml.listen.port);

    while let Some(connecting) = server_endpoint.accept().await {
        println!("Got a new connection request!");
        tokio::spawn(async move {
            match connecting.await {
                Ok(connection) => {
                    println!("New QUIC Connection: {:?}", connection.remote_address());

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

                },
                Err(e) => eprintln!("Connection failed: {}", e),
            }
        });
    }

}
