use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub listen: Listen,
    pub backends: Vec<Backend>,

    #[serde(default = "get_default_load_balancing")]
    pub load_balancing: LoadBalancing,

    #[serde(default = "get_default_log")]
    pub log: Log
}

#[derive(Debug, Deserialize)]
pub struct Listen {
    #[serde(default = "get_default_protocol")]
    pub protocol: String,   // "http3"

    #[serde(default = "get_default_port")]
    pub port: u32,          // 9889

    #[serde(default = "get_default_address")]
    pub address: String,    // "0.0.0.0"
    pub tls: Tls,
}

#[derive(Debug, Deserialize)]
pub struct Tls {
    pub cert: String,       // "/path/to/cert"
    pub key: String,        // "/path/to/key"
}

#[derive(Debug, Deserialize)]
pub struct Backend {
    pub id: String,         // "backend1"
    pub address: String,    // "10.0.1.100:8080"

    #[serde(default = "get_default_weight")]
    pub weight: u32,        // 100
    pub health_check: HealthCheck,
}

#[derive(Debug, Deserialize)]
pub struct HealthCheck {
    #[serde(default = "get_default_path")]
    pub path: String,       // "/health"

    #[serde(default = "get_default_interval")]
    pub interval: String,   // "5s" (could later parse into Duration)
}

#[derive(Debug, Deserialize)]
pub struct LoadBalancing {
    #[serde(rename = "type")]
    pub lb_type: String,    // "weight-based", "least_connection", etc.
}

#[derive(Debug, Deserialize)]
pub struct Log {
    #[serde(default = "get_default_log_level")]
    pub level: String, // "info, warn, error"
}



// default values
fn get_default_protocol() -> String {
    String::from("http3")
}

fn get_default_port() -> u32 {
    9889
}

fn get_default_address() -> String {
    String::from("0.0.0.0")
}

fn get_default_weight() -> u32 {
    100
}

fn get_default_path() -> String {
    String::from("/health")
}

fn get_default_interval() -> String {
    String::from("5s")
}

fn get_default_log_level() -> String {
    String::from("info")
}

fn get_default_load_balancing() -> LoadBalancing {
    LoadBalancing { lb_type: String::from("weight-based") }
}

fn get_default_log() -> Log {
    Log { level: String::from("info") }
}