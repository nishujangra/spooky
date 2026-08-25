use super::*;

macro_rules! validation_error {
    ($($arg:tt)*) => {{
        let message = format!($($arg)*);
        super::record_validation_error(message.clone());
        log::error!("{}", message);
    }};
}

pub(super) fn validate_pem_certificates(path: &str, field_name: &str) -> bool {
    let pem = match std::fs::read(path) {
        Ok(pem) => pem,
        Err(err) => {
            validation_error!("Cannot open {} '{}': {}", field_name, path, err);
            return false;
        }
    };

    let certs = match CertificateDer::pem_slice_iter(&pem).collect::<Result<Vec<_>, _>>() {
        Ok(certs) => certs,
        Err(err) => {
            validation_error!(
                "Cannot parse PEM certificates from {} '{}': {}",
                field_name,
                path,
                err
            );
            return false;
        }
    };

    if certs.is_empty() {
        validation_error!(
            "{} '{}' does not contain any PEM certificate blocks",
            field_name,
            path
        );
        return false;
    }

    true
}

pub(super) fn validate_pem_private_key(path: &str, field_name: &str) -> bool {
    let pem = match std::fs::read(path) {
        Ok(pem) => pem,
        Err(err) => {
            validation_error!("Cannot open {} '{}': {}", field_name, path, err);
            return false;
        }
    };

    match PrivateKeyDer::from_pem_slice(&pem) {
        Ok(_) => true,
        Err(err) => {
            validation_error!(
                "Cannot parse PEM private key from {} '{}': {}",
                field_name,
                path,
                err
            );
            false
        }
    }
}

pub(super) fn validate_upstream_tls(field_prefix: &str, tls: &UpstreamTls) -> bool {
    if !tls.verify_certificates {
        warn!(
            "{}.verify_certificates=false: upstream TLS certificate verification is disabled; only use in trusted/development environments",
            field_prefix
        );
    }

    if let Some(ca_file) = tls.ca_file.as_ref() {
        if ca_file.trim().is_empty() {
            validation_error!("{}.ca_file cannot be empty when provided", field_prefix);
            return false;
        }
        if !validate_pem_certificates(ca_file, &format!("{}.ca_file", field_prefix)) {
            return false;
        }
    }

    if let Some(ca_dir) = tls.ca_dir.as_ref() {
        if ca_dir.trim().is_empty() {
            validation_error!("{}.ca_dir cannot be empty when provided", field_prefix);
            return false;
        }
        let metadata = match std::fs::metadata(ca_dir) {
            Ok(metadata) => metadata,
            Err(err) => {
                validation_error!("Cannot stat {}.ca_dir '{}': {}", field_prefix, ca_dir, err);
                return false;
            }
        };
        if !metadata.is_dir() {
            validation_error!("{}.ca_dir must be a directory: {}", field_prefix, ca_dir);
            return false;
        }
    }

    if !super::validate_secret_source_exclusivity(
        tls.client_certificate
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        tls.client_certificate_ref.as_ref(),
        &format!("{}.client_certificate", field_prefix),
        &format!("{}.client_certificate_ref", field_prefix),
    ) {
        return false;
    }

    if !super::validate_secret_source_exclusivity(
        tls.client_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        tls.client_key_ref.as_ref(),
        &format!("{}.client_key", field_prefix),
        &format!("{}.client_key_ref", field_prefix),
    ) {
        return false;
    }

    let has_client_certificate = tls
        .client_certificate
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || tls.client_certificate_ref.is_some();
    let has_client_key = tls
        .client_key
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || tls.client_key_ref.is_some();

    if has_client_certificate != has_client_key {
        validation_error!(
            "{}.client_certificate and {}.client_key must be configured as a complete mTLS pair",
            field_prefix,
            field_prefix
        );
        return false;
    }

    true
}

pub(super) fn validate_listen_config(listen: &Listen, field_prefix: &str) -> bool {
    if listen.protocol != "http3" {
        validation_error!(
            "{} protocol: expected 'http3', found '{}'",
            field_prefix,
            listen.protocol
        );
        return false;
    }

    if listen.address.is_empty() {
        validation_error!("{} address is empty", field_prefix);
        return false;
    }

    if listen.port == 0 {
        validation_error!(
            "Invalid {} port: {} (must be between 1 and 65535)",
            field_prefix,
            listen.port
        );
        return false;
    }

    let tls_prefix = format!("{}.tls", field_prefix);
    let legacy_cert = listen.tls.cert.trim();
    let legacy_key = listen.tls.key.trim();
    let has_legacy_cert_pair = !legacy_cert.is_empty() || !legacy_key.is_empty();
    let has_sni_certificates = !listen.tls.certificates.is_empty();

    if !has_legacy_cert_pair && !has_sni_certificates {
        validation_error!(
            "{} requires either cert/key or certificates entries",
            tls_prefix
        );
        return false;
    }

    if has_legacy_cert_pair {
        if legacy_cert.is_empty() || legacy_key.is_empty() {
            validation_error!(
                "{}.cert and {}.key must both be set when either is provided",
                tls_prefix,
                tls_prefix
            );
            return false;
        }
        if !validate_pem_certificates(legacy_cert, &format!("{}.cert", tls_prefix)) {
            return false;
        }
        if !validate_pem_private_key(legacy_key, &format!("{}.key", tls_prefix)) {
            return false;
        }
    }

    let mut seen_sni_names: HashMap<String, usize> = HashMap::new();
    for (idx, entry) in listen.tls.certificates.iter().enumerate() {
        let field_prefix = format!("{}.certificates[{idx}]", tls_prefix);
        let sni_name = match normalize_sni_server_name(&entry.server_name) {
            Some(sni) => sni,
            None => {
                validation_error!(
                    "{}.server_name '{}' is not a valid DNS hostname",
                    field_prefix,
                    entry.server_name
                );
                return false;
            }
        };

        if let Some(first_idx) = seen_sni_names.insert(sni_name.clone(), idx) {
            validation_error!(
                "{}.server_name '{}' duplicates {}.certificates[{}].server_name",
                field_prefix,
                entry.server_name,
                tls_prefix,
                first_idx
            );
            return false;
        }

        let cert = entry.cert.trim();
        if cert.is_empty() {
            validation_error!("{}.cert cannot be empty", field_prefix);
            return false;
        }
        if !validate_pem_certificates(cert, &format!("{}.cert", field_prefix)) {
            return false;
        }

        let key = entry.key.trim();
        if key.is_empty() {
            validation_error!("{}.key cannot be empty", field_prefix);
            return false;
        }
        if !validate_pem_private_key(key, &format!("{}.key", field_prefix)) {
            return false;
        }
    }

    if listen.tls.client_auth.require_client_cert && !listen.tls.client_auth.enabled {
        validation_error!(
            "{}.client_auth.require_client_cert requires client_auth.enabled=true",
            tls_prefix
        );
        return false;
    }

    if listen.tls.client_auth.enabled {
        let Some(ca_file) = listen.tls.client_auth.ca_file.as_ref() else {
            validation_error!(
                "{}.client_auth.ca_file is required when client_auth.enabled=true",
                tls_prefix
            );
            return false;
        };
        if ca_file.trim().is_empty() {
            validation_error!("{}.client_auth.ca_file cannot be empty", tls_prefix);
            return false;
        }
        if !validate_pem_certificates(ca_file, &format!("{}.client_auth.ca_file", tls_prefix)) {
            return false;
        }
    }

    true
}

pub(super) fn is_loopback_bind_address(raw: &str) -> bool {
    let normalized = raw.trim().trim_start_matches('[').trim_end_matches(']');
    if normalized.eq_ignore_ascii_case("localhost") {
        return true;
    }
    normalized
        .parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

pub(super) fn is_valid_http_token(value: &str) -> bool {
    !value.is_empty()
        && value.as_bytes().iter().all(|byte| {
            matches!(
                byte,
                b'0'..=b'9'
                    | b'a'..=b'z'
                    | b'A'..=b'Z'
                    | b'!'
                    | b'#'
                    | b'$'
                    | b'%'
                    | b'&'
                    | b'\''
                    | b'*'
                    | b'+'
                    | b'-'
                    | b'.'
                    | b'^'
                    | b'_'
                    | b'`'
                    | b'|'
                    | b'~'
            )
        })
}

pub(super) fn is_valid_http_url(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }

    let Ok(uri) = trimmed.parse::<http::Uri>() else {
        return false;
    };

    matches!(uri.scheme_str(), Some("http") | Some("https")) && uri.authority().is_some()
}

pub(super) fn is_valid_https_url(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }

    let Ok(uri) = trimmed.parse::<http::Uri>() else {
        return false;
    };

    uri.scheme_str() == Some("https") && uri.authority().is_some()
}

pub(super) fn is_valid_connect_authority(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return false;
    }

    if let Some(rest) = trimmed.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return false;
        };
        let host = &rest[..end];
        if host.is_empty() {
            return false;
        }
        let suffix = &rest[end + 1..];
        if !suffix.starts_with(':') || suffix.len() <= 1 {
            return false;
        }
        return suffix[1..].parse::<u16>().ok().is_some_and(|port| port > 0);
    }

    let Some((host, port)) = trimmed.rsplit_once(':') else {
        return false;
    };
    if host.is_empty() || host.contains(':') {
        return false;
    }
    port.parse::<u16>().ok().is_some_and(|value| value > 0)
}

pub(super) fn is_valid_request_key_spec(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }

    if matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "path"
            | "authority"
            | "method"
            | "cid"
            | "sticky-cid"
            | "peer_ip"
            | "client_ip"
            | "bearer_token"
    ) {
        return true;
    }

    let Some((source, key_name)) = trimmed.split_once(':') else {
        return false;
    };
    let source = source.trim();
    let key_name = key_name.trim();
    !key_name.is_empty()
        && matches!(
            source.to_ascii_lowercase().as_str(),
            "header" | "cookie" | "query"
        )
}

pub(super) fn normalize_route_host(raw: &str) -> String {
    let trimmed = raw.trim();
    let host = if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            &rest[..end]
        } else {
            trimmed
        }
    } else if let Some((candidate_host, candidate_port)) = trimmed.rsplit_once(':') {
        if !candidate_host.contains(':') && candidate_port.chars().all(|c| c.is_ascii_digit()) {
            candidate_host
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    host.trim_end_matches('.').to_ascii_lowercase()
}

pub(super) fn valid_route_host_pattern(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return false;
    }

    let normalized = normalize_route_host(trimmed);
    if normalized.is_empty()
        || normalized.contains('[')
        || normalized.contains(']')
        || normalized.contains('/')
        || normalized.contains('?')
        || normalized.contains('#')
    {
        return false;
    }

    match normalized.strip_prefix("*.") {
        Some(wildcard_suffix) => !wildcard_suffix.is_empty() && !wildcard_suffix.contains('*'),
        None => !normalized.contains('*'),
    }
}

pub(super) fn normalized_route_method(method: Option<&str>) -> Option<String> {
    method
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_uppercase())
}

pub(super) fn valid_static_host_header(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed == value
        && !trimmed.chars().any(|ch| ch.is_ascii_whitespace())
        && !trimmed.contains('/')
        && !trimmed.contains('?')
        && !trimmed.contains('#')
        && http::HeaderValue::from_str(trimmed).is_ok()
}

pub(super) fn normalize_sni_server_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.contains(':')
        || trimmed.contains('*')
        || trimmed.chars().any(char::is_whitespace)
    {
        return None;
    }
    let without_trailing_dot = trimmed.trim_end_matches('.');
    if without_trailing_dot.is_empty() {
        return None;
    }
    let ascii = idna::domain_to_ascii(without_trailing_dot).ok()?;
    if ascii.parse::<IpAddr>().is_ok() {
        return None;
    }
    Some(ascii.to_ascii_lowercase())
}
