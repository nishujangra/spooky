// proxy http3 server QUIC + HTTP/3
use log::{info, debug, trace, error, LevelFilter};
use env_logger;

use std::fs;

pub mod config;
pub mod utils;

pub mod server;
use config::config::Config;

fn init_logger(log_level: &str) {
    let level = match log_level.to_lowercase().as_str() {
        "trace" => LevelFilter::Trace,
        "debug" => LevelFilter::Debug,
        "info" => LevelFilter::Info,
        "warn" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        _ => {
            eprintln!("Invalid log level '{}', defaulting to 'info'", log_level);
            LevelFilter::Info
        }
    };

    env_logger::Builder::from_default_env()
        .filter_level(level)
        .format_timestamp_secs()
        .init();
}

fn read_config() -> Config {
    let filename: String = String::from("config.yaml");
    trace!("Reading configuration from: {}", filename);
    
    let text = fs::read_to_string(filename).unwrap_or_else(|err| {
        error!("Failed to read config file: {}", err);
        std::process::exit(1);
    });

    let data: Config = serde_yaml::from_str(&text).unwrap_or_else(|err| {
        error!("Could not parse YAML file: {}", err);
        std::process::exit(1);
    });

    debug!("Configuration loaded successfully");
    return data;
}

#[tokio::main]
async fn main() {
    // Read config first to get log level
    let config_yaml = read_config();
    
    // Initialize logger with config log level
    init_logger(&config_yaml.log.level);
    
    info!("Starting Spooky HTTP/3 Proxy Server");
    info!("Log level set to: {}", config_yaml.log.level);
    debug!("Configuration: {:?}", config_yaml);

    let proxy_server = server::Server::new(config_yaml)
        .await
        .expect("Failed to create server");

    info!("Server initialized successfully");
    proxy_server.run().await;
}
