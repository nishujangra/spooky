use serde::Deserialize;

use crate::config::default::{
    get_default_address, 
    get_default_interval, 
    get_default_load_balancing, 
    get_default_log, 
    get_default_log_level, 
    get_default_path, 
    get_default_port, 
    get_default_protocol, 
    get_default_weight
};

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub listen: Listen,
    pub backends: Vec<Backend>,

    #[serde(default = "get_default_load_balancing")]
    pub load_balancing: LoadBalancing,

    #[serde(default = "get_default_log")]
    pub log: Log
}

#[derive(Debug, Deserialize, Clone)]
pub struct Listen {
    #[serde(default = "get_default_protocol")]
    pub protocol: String,   // "http3"

    #[serde(default = "get_default_port")]
    pub port: u32,          // 9889

    #[serde(default = "get_default_address")]
    pub address: String,    // "0.0.0.0"
    pub tls: Tls,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Tls {
    pub cert: String,       // "/path/to/cert"
    pub key: String,        // "/path/to/key"
}

#[derive(Debug, Deserialize, Clone)]
pub struct Backend {
    pub id: String,         // "backend1"
    pub address: String,    // "10.0.1.100:8080"

    #[serde(default = "get_default_weight")]
    pub weight: u32,        // 100
    pub health_check: HealthCheck,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HealthCheck {
    #[serde(default = "get_default_path")]
    pub path: String,       // "/health"

    #[serde(default = "get_default_interval")]
    pub interval: String,   // "5s" (could later parse into Duration)
}

#[derive(Debug, Deserialize, Clone)]
pub struct LoadBalancing {
    #[serde(rename = "type")]
    pub lb_type: String,    // "weight-based", "least_connection", etc.
}

#[derive(Debug, Deserialize, Clone)]
pub struct Log {
    #[serde(default = "get_default_log_level")]
    pub level: String, // "info, warn, error"
}



impl Backend {
    pub fn is_healthy(&self) -> bool {
        true
    }
}