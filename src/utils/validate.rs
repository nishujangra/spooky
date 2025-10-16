use crate::config::config::Config;
use log::{error, info};

pub fn validate(config: &Config) -> bool {
    info!("Starting configuration validation...");

    // --- Validate protocol ---
    if config.listen.protocol != "http3" {
        error!("Invalid protocol: expected 'http3', found '{}'", config.listen.protocol);
        return false;
    }

    // --- Validate listen address ---
    if config.listen.address.is_empty() {
        error!("Listen address is empty");
        return false;
    }

    // --- Validate listen port ---
    if config.listen.port == 0 || config.listen.port > 65535 {
        error!(
            "Invalid listen port: {} (must be between 1 and 65535)",
            config.listen.port
        );
        return false;
    }

    // --- Validate TLS certs ---
    if config.listen.tls.cert.is_empty() {
        error!("TLS certificate path is missing");
        return false;
    }

    if config.listen.tls.key.is_empty() {
        error!("TLS key path is missing");
        return false;
    }

    // --- Validate backends ---
    if config.backends.is_empty() {
        error!("No backends configured");
        return false;
    }

    for backend in &config.backends {
        if backend.id.is_empty() {
            error!("Backend id is missing");
            return false;
        }

        if backend.address.is_empty() {
            error!("Backend address is missing for backend id '{}'", backend.id);
            return false;
        }

        if backend.weight == 0 {
            error!("Backend weight is invalid (0) for backend id '{}'", backend.id);
            return false;
        }
    }

    info!("Configuration validation passed successfully");
    
    true
}
