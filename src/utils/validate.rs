use crate::config::config::Config;

// boilerplate function
pub fn validate(
    config: &Config
) -> bool {
    if config.listen.protocol != "http3"{
        return false;
    }

    if config.listen.tls.cert == "" || config.listen.tls.key == "" {
        return false;
    }

    true
}