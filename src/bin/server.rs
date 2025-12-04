use h3::server;
use http::Response;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use quinn::{Endpoint, ServerConfig};
use std::{env, fs, net::SocketAddr, sync::Arc};

use bytes::Bytes;

use h3_quinn::Connection as H3QuinnConnection;

pub fn load_tls(
    cert_path: &str,
    key_path: &str,
) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let cert_bytes = fs::read(cert_path).expect("Failed to read cert file");
    let key_bytes = fs::read(key_path).expect("Failed to read key file");

    let certs = vec![CertificateDer::from(cert_bytes)];
    let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(key_bytes));

    (certs, key)
}

#[tokio::main]
async fn main() {
    // Read port from CLI
    let args: Vec<String> = env::args().collect();

    let mut port = 8000; // default port = 8000

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--port" {
            if let Some(p) = iter.next() {
                port = p.parse::<u16>().expect("Port must be a number");
            }
        }
    }

    // Install default crypto provider for Rustls
    let _ = rustls::crypto::CryptoProvider::install_default(
        rustls::crypto::ring::default_provider()
    );

    let cert_path = "./certs/cert.der";
    let key_path = "./certs/key.der";

    let (certs, key) = load_tls(&cert_path, &key_path);

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

    let server_config = ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(tls_config)
            .expect("Failed to create QUIC server config"),
    ));

    let addr: SocketAddr = format!("{}:{}", "127.0.0.1", port) // port read from the cli
        .parse()
        .expect("Invalid Listen address");

    let server_endpoint = Endpoint::server(server_config, addr).unwrap();

    println!("Server running on {addr}");

    while let Some(connecting) = server_endpoint.accept().await {
        tokio::spawn(async move {
            match connecting.await {
                Ok(new_connection) => {
                    println!("New connection established");
                    let h3_connection = H3QuinnConnection::new(new_connection);

                    let mut h3_server: h3::server::Connection<_, Bytes> = server::builder()
                        .max_field_section_size(8192)
                        .build(h3_connection)
                        .await
                        .expect("Failed to build h3 server");

                    loop {
                        match h3_server.accept().await {
                            Ok(Some(req_resolver)) => {
                                match req_resolver.resolve_request().await {
                                    Ok((req, mut stream)) => {
                                        println!("Got request: {:?}", req);

                                        let response = Response::builder()
                                            .status(200)
                                            .header("content-type", "text/plain")
                                            .body(())
                                            .unwrap();

                                        if let Err(e) = stream.send_response(response).await {
                                            eprintln!("Failed to send response: {e:?}");
                                            break;
                                        }

                                        if let Err(e) = stream
                                            .send_data(Bytes::from("Hello from HTTP/3 server!"))
                                            .await
                                        {
                                            eprintln!("Failed to send data: {e:?}");
                                            break;
                                        }

                                        if let Err(e) = stream.finish().await {
                                            eprintln!("Failed to finish stream: {e:?}");
                                            break;
                                        }
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
                Err(err) => eprintln!("Connection failed: {err:?}"),
            }
        });
    }
}