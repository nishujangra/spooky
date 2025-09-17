use quinn::{Endpoint, ServerConfig};
use std::{fs, net::SocketAddr, sync::Arc};

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
    let config_yaml = read_config();

    let (certs, key) = load_tls(
        &config_yaml.listen.tls.cert,
        &config_yaml.listen.tls.key
    );

    let mut server_config = ServerConfig::with_single_cert(
        certs, key
    ).expect("Failed to create server config");

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
                Ok(_connection) => println!("Connection established"),
                Err(e) => eprintln!("Connection failed: {}", e),
            }
        });
    }

}
