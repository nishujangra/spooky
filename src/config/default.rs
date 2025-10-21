use crate::config::config::{LoadBalancing, Log};

// default values
pub fn get_default_protocol() -> String {
    String::from("http3")
}

pub fn get_default_port() -> u32 {
    9889
}

pub fn get_default_address() -> String {
    String::from("0.0.0.0")
}

pub fn get_default_weight() -> u32 {
    100
}

pub fn get_default_path() -> String {
    String::from("/health")
}

pub fn get_default_interval() -> String {
    String::from("5s")
}

pub fn get_default_log_level() -> String {
    String::from("info")
}

pub fn get_default_load_balancing() -> LoadBalancing {
    LoadBalancing { lb_type: String::from("weight-based") }
}

pub fn get_default_log() -> Log {
    Log { level: String::from("info") }
}