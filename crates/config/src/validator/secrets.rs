use super::*;

macro_rules! validation_error {
    ($($arg:tt)*) => {{
        let message = format!($($arg)*);
        super::record_validation_error(message.clone());
        log::error!("{}", message);
    }};
}

pub(super) fn validate_secret_ref(secret_ref: &SecretRef, field_name: &str) -> bool {
    let reference = secret_ref.raw_value().trim();
    if reference.is_empty() {
        validation_error!("{field_name}.ref must be non-empty");
        return false;
    }

    match secret_ref.scheme() {
        Some("literal") => {
            let Some(value) = reference.strip_prefix("literal:") else {
                validation_error!("{field_name}.ref must use literal:<value> format");
                return false;
            };
            if value.trim().is_empty() {
                validation_error!("{field_name}.ref literal value must be non-empty");
                return false;
            }
        }
        Some("file") => {
            let Some(path) = reference.strip_prefix("file://") else {
                validation_error!("{field_name}.ref must use file://<path> format");
                return false;
            };
            if path.trim().is_empty() {
                validation_error!("{field_name}.ref file path must be non-empty");
                return false;
            }
        }
        Some(other) => {
            validation_error!(
                "{field_name}.ref uses unsupported secret scheme '{other}'; supported schemes are literal and file"
            );
            return false;
        }
        None => {
            validation_error!("{field_name}.ref must include a supported secret scheme prefix");
            return false;
        }
    }

    true
}

pub(super) fn validate_secret_source_exclusivity(
    literal_present: bool,
    secret_ref: Option<&SecretRef>,
    literal_field_name: &str,
    ref_field_name: &str,
) -> bool {
    if literal_present && secret_ref.is_some() {
        validation_error!("{literal_field_name} and {ref_field_name} cannot both be set");
        return false;
    }
    if let Some(secret_ref) = secret_ref
        && !validate_secret_ref(secret_ref, ref_field_name)
    {
        return false;
    }
    true
}

pub(super) fn validate_secrets_config(config: &Config) -> bool {
    if let Some(default_provider) = config.secrets.default_provider.as_deref() {
        if default_provider.trim().is_empty() {
            validation_error!("secrets.default_provider cannot be empty when provided");
            return false;
        }
        if !config.secrets.providers.contains_key(default_provider) {
            validation_error!(
                "secrets.default_provider '{}' must reference a configured provider",
                default_provider
            );
            return false;
        }
    }

    for (name, provider) in &config.secrets.providers {
        if name.trim().is_empty() {
            validation_error!("secrets.providers keys must be non-empty");
            return false;
        }
        match provider {
            SecretProvider::File { base_dir } => {
                if let Some(base_dir) = base_dir.as_deref()
                    && base_dir.trim().is_empty()
                {
                    validation_error!(
                        "secrets.providers.{}.base_dir cannot be empty when provided",
                        name
                    );
                    return false;
                }
            }
        }
    }

    true
}
