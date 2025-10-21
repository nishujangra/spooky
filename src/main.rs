//! Spooky HTTP/3 Load Balancer - Main Entry Point
//! 
//! TODO: Add CLI argument parsing for config file path
//! TODO: Implement graceful shutdown signal handling
//! TODO: Add configuration validation and error reporting
//! TODO: Implement proper error handling for server initialization
//! TODO: Add health check endpoint for load balancer itself
//! TODO: Add metrics collection and monitoring endpoints
//! TODO: Implement configuration hot-reload capability
//! TODO: Add structured logging with request tracing
//! TODO: Add startup banner and version information
//! TODO: Implement proper process lifecycle management

use clap::{Parser};

// proxy http3 server QUIC + HTTP/3
use log::{info, debug, trace, error, LevelFilter};
use env_logger;

use std::{fs};

pub mod config;
pub mod utils;
pub mod lb;

pub mod proxy;
use config::config::Config;

use crate::utils::validate::validate;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {    
    // Sets a custom config file
    #[arg(short, long)]
    config: Option<String>,
}

/// Initialize a minimal logger early — so we see errors before main logger setup
fn init_early_logger() {
    let _ = env_logger::builder()
        .filter_level(LevelFilter::Error)
        .format_timestamp_secs()
        .try_init(); // ignore errors if already initialized
}

fn init_logger(log_level: &str) {
    let level = match log_level.to_lowercase().as_str() {
        "trace" => LevelFilter::Trace,
        "debug" => LevelFilter::Debug,
        "info" => LevelFilter::Info,
        "warn" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        "off" => LevelFilter::Off,
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

fn read_config(filename: &str) -> Config {
    // TODO: Add support for multiple config file formats (YAML, JSON, TOML)
    // TODO: Implement configuration schema validation
    // TODO: Add configuration file watching for hot-reload
    // TODO: Add fallback to default configuration if file not found
    // TODO: Implement configuration encryption/decryption for sensitive data
    
    let text = fs::read_to_string(filename).unwrap_or_else(|err| {
        error!("Failed to read config file: {}", err);
        std::process::exit(1);
    });

    let data: Config = serde_yaml::from_str(&text).unwrap_or_else(|err| {
        error!("Could not parse YAML file: {}", err);
        std::process::exit(1);
    });

    data
}

#[tokio::main]
async fn main() {
    // TODO: Add startup banner with version and build info
    

    // TODO: Implement signal handling for graceful shutdown (SIGTERM, SIGINT)
    // TODO: Add panic hook for proper error reporting
    // TODO: Implement proper error handling instead of expect() calls
    // TODO: Add startup health checks before accepting connections
    // TODO: Add metrics server startup
    // TODO: Implement proper process lifecycle management

    init_early_logger();

    // Parse CLI arguments
    let cli = Cli::parse();

    let config_path = cli.config.unwrap_or_else(|| "config.yaml".to_string());

    // Read configuration file
    let config_yaml = read_config(&config_path);

    // Initialize the Logger
    init_logger(&config_yaml.log.level);
    
    // Validate Configurations
    if validate(&config_yaml) == false {
        error!("Configuration validation failed. Exiting...");
        std::process::exit(1);
    }
    
    // Initialize logger with config log level
    info!("Starting Spooky HTTP/3 Proxy Server");
    info!("Log level set to: {}", config_yaml.log.level);
    debug!("Configuration: {:?}", config_yaml);

    let proxy_server = proxy::Server::new(config_yaml)
        .await
        .expect("Failed to create server");

    info!("Server initialized successfully");
    proxy_server.run().await;
}
