use std::fs;

pub mod config;

// imports from models
use config::config::Config;

fn read_config() {
    let filename: String = String::from("config.yaml");
    let text = fs::read_to_string(filename).unwrap();

    let data: Config = serde_yaml::from_str(&text).unwrap_or_else(|err| {
        eprintln!("Could not parse YAML file: {err}");
        std::process::exit(1);
    });

    println!("{:#?}", data);
}

fn main() {
    println!("Hello, world!");

    read_config();
}
