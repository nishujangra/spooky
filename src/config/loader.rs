use std::fs;

use crate::config::config::Config;

pub fn read_config(filename: &str) -> Result<Config, String> {
    // TODO: Add support for multiple config file formats (YAML, JSON, TOML)
    // TODO: Add configuration file watching for hot-reload
    // TODO: Implement configuration encryption/decryption for sensitive data

    let text = fs::read_to_string(filename)
        .map_err(|err| format!("Failed to read config file '{}': {}", filename, err))?;

    let data: Config = serde_yaml::from_str(&text)
        .map_err(|err| format!("Could not parse YAML file '{}': {}", filename, err))?;

    Ok(data)
}