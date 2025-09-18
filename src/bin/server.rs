use h3::server;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use quinn::{Endpoint, ServerConfig};
use std::{fs, net::SocketAddr};

use bytes::Bytes;

// use http::Response;

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
async fn main(){
    let cert_path = "./certs/cert.der";
    let key_path = "./certs/key.der";

    let (certs, key) = load_tls(
        &cert_path,
        &key_path
    ); 

    // #[warn(unused_mut)]
    let server_config = ServerConfig::with_single_cert(
        certs, key
    ).expect("Failed to create server config");

    let addr: SocketAddr = format!("{}:{}", "127.0.0.1", "7777")
        .parse()
        .expect("Invalid Listen address");

    let server_endpoint = Endpoint::server(server_config, addr).unwrap();

    println!("Sever running on {addr}");

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
                        .expect("Failed to build h3 client");

                    while let Some(mut req_resolver) = h3_server.accept().await.unwrap() {
                        let (req,mut stream) = req_resolver.resolve_request().await.unwrap();
                        println!("Got request: {:?}", req);
                    }
                },
                Err(err) => eprintln!("Connection failed: {err:?}"),
            }
        });
    }
}