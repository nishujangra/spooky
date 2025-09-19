// proxy http3 server QUIC + HTTP/3

use std::fs;

pub mod config;
pub mod utils;

pub mod server;
use config::config::Config;

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

    let proxy_server = server::Server::new(config_yaml)
        .await
        .expect("Failed to create server");

    proxy_server.run().await;
}
