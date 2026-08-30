use std::collections::HashMap;

use super::*;
use crate::validator::{
    helpers::{
        is_valid_http_token, is_valid_http_url, is_valid_https_or_loopback_http_url,
        is_valid_https_url,
    },
    secrets::validate_secret_source_exclusivity,
};

macro_rules! validation_error {
    ($($arg:tt)*) => {{
        let message = format!($($arg)*);
        super::record_validation_error(message.clone());
        log::error!("{}", message);
    }};
}

pub(super) fn validate_external_auth_headers(
    upstream_name: &str,
    field_prefix: &str,
    request_headers: &[crate::config::ExternalAuthRequestHeader],
    response_header_allowlist: &[String],
) -> bool {
    let mut seen_request_headers = std::collections::HashSet::new();
    for (idx, header) in request_headers.iter().enumerate() {
        let header_name = header.name.trim();
        if header_name.is_empty() {
            validation_error!(
                "upstream '{}' {}.request_headers[{}].name must be non-empty",
                upstream_name,
                field_prefix,
                idx
            );
            return false;
        }
        if http::header::HeaderName::from_bytes(header_name.as_bytes()).is_err() {
            validation_error!(
                "upstream '{}' {}.request_headers[{}].name must be a valid HTTP header name",
                upstream_name,
                field_prefix,
                idx
            );
            return false;
        }
        if http::HeaderValue::from_str(header.value.as_str()).is_err() {
            validation_error!(
                "upstream '{}' {}.request_headers[{}].value must be a valid HTTP header value",
                upstream_name,
                field_prefix,
                idx
            );
            return false;
        }
        let normalized_name = header_name.to_ascii_lowercase();
        if !seen_request_headers.insert(normalized_name) {
            validation_error!(
                "upstream '{}' {}.request_headers contains duplicate header names",
                upstream_name,
                field_prefix
            );
            return false;
        }
    }

    let mut seen_allowed_headers = std::collections::HashSet::new();
    for (idx, header_name) in response_header_allowlist.iter().enumerate() {
        let header_name = header_name.trim();
        if header_name.is_empty() {
            validation_error!(
                "upstream '{}' {}.response_header_allowlist[{}] must be non-empty",
                upstream_name,
                field_prefix,
                idx
            );
            return false;
        }
        if http::header::HeaderName::from_bytes(header_name.as_bytes()).is_err() {
            validation_error!(
                "upstream '{}' {}.response_header_allowlist[{}] must be a valid HTTP header name",
                upstream_name,
                field_prefix,
                idx
            );
            return false;
        }
        let normalized_name = header_name.to_ascii_lowercase();
        if !seen_allowed_headers.insert(normalized_name) {
            validation_error!(
                "upstream '{}' {}.response_header_allowlist contains duplicate header names",
                upstream_name,
                field_prefix
            );
            return false;
        }
    }

    true
}

pub(super) fn validate_upstream_auth(
    upstream_name: &str,
    upstream: &crate::config::Upstream,
) -> bool {
    if let Some(api_key) = upstream.auth.api_key.as_ref() {
        let header_name = api_key.header_name.trim();
        if header_name.is_empty() {
            validation_error!(
                "upstream '{}' auth.api_key.header_name must be non-empty",
                upstream_name
            );
            return false;
        }
        if !is_valid_http_token(header_name) {
            validation_error!(
                "upstream '{}' auth.api_key.header_name must be a valid HTTP header name",
                upstream_name
            );
            return false;
        }
        if api_key.keys.is_empty() || api_key.keys.iter().any(|value| value.trim().is_empty()) {
            validation_error!(
                "upstream '{}' auth.api_key.keys must contain at least one non-empty key",
                upstream_name
            );
            return false;
        }
        let mut seen_api_keys = std::collections::HashSet::new();
        for key in &api_key.keys {
            if !seen_api_keys.insert(key.trim().to_string()) {
                validation_error!(
                    "upstream '{}' auth.api_key.keys contains duplicate values",
                    upstream_name
                );
                return false;
            }
        }
    }

    if let Some(external_auth) = upstream.auth.external_auth.as_ref() {
        if upstream.auth.api_key.is_some() || upstream.auth.jwt.is_some() {
            validation_error!(
                "upstream '{}' auth.external_auth cannot be combined with auth.api_key or auth.jwt in v1",
                upstream_name
            );
            return false;
        }
        if !upstream.auth.required_scopes.is_empty() || !upstream.auth.required_roles.is_empty() {
            validation_error!(
                "upstream '{}' auth.external_auth cannot be combined with auth.required_scopes or auth.required_roles in v1",
                upstream_name
            );
            return false;
        }

        match external_auth {
            ExternalAuth::Http {
                endpoint,
                request_headers,
                response_header_allowlist,
                timeout_ms,
                ..
            } => {
                if !is_valid_http_url(endpoint) {
                    validation_error!(
                        "upstream '{}' auth.external_auth.http.endpoint must be an absolute http(s) URL",
                        upstream_name
                    );
                    return false;
                }
                if !validate_external_auth_headers(
                    upstream_name,
                    "auth.external_auth.http",
                    request_headers,
                    response_header_allowlist,
                ) {
                    return false;
                }
                if *timeout_ms == 0 {
                    validation_error!(
                        "upstream '{}' auth.external_auth.http.timeout_ms must be greater than 0",
                        upstream_name
                    );
                    return false;
                }
            }
            ExternalAuth::Oidc {
                discovery_url,
                issuer_url,
                client_id,
                client_secret,
                client_secret_ref,
                audience,
                scopes,
                request_headers,
                response_header_allowlist,
                timeout_ms,
                ..
            } => {
                let has_discovery_url = discovery_url
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty());
                let has_issuer_url = issuer_url
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty());
                if !has_discovery_url && !has_issuer_url {
                    validation_error!(
                        "upstream '{}' auth.external_auth.oidc requires discovery_url or issuer_url",
                        upstream_name
                    );
                    return false;
                }
                if let Some(discovery_url) = discovery_url.as_deref()
                    && !discovery_url.trim().is_empty()
                    && !is_valid_https_or_loopback_http_url(discovery_url)
                {
                    validation_error!(
                        "upstream '{}' auth.external_auth.oidc.discovery_url must be an absolute https URL or loopback http URL",
                        upstream_name
                    );
                    return false;
                }
                if let Some(issuer_url) = issuer_url.as_deref()
                    && !issuer_url.trim().is_empty()
                    && !is_valid_https_or_loopback_http_url(issuer_url)
                {
                    validation_error!(
                        "upstream '{}' auth.external_auth.oidc.issuer_url must be an absolute https URL or loopback http URL",
                        upstream_name
                    );
                    return false;
                }
                if client_id.trim().is_empty() {
                    validation_error!(
                        "upstream '{}' auth.external_auth.oidc.client_id must be non-empty",
                        upstream_name
                    );
                    return false;
                }
                if !validate_secret_source_exclusivity(
                    client_secret
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty()),
                    client_secret_ref.as_ref(),
                    &format!(
                        "upstream '{}' auth.external_auth.oidc.client_secret",
                        upstream_name
                    ),
                    &format!(
                        "upstream '{}' auth.external_auth.oidc.client_secret_ref",
                        upstream_name
                    ),
                ) {
                    return false;
                }
                if client_secret
                    .as_deref()
                    .is_some_and(|value| value.trim().is_empty())
                {
                    validation_error!(
                        "upstream '{}' auth.external_auth.oidc.client_secret must be non-empty when provided",
                        upstream_name
                    );
                    return false;
                }
                if audience
                    .as_deref()
                    .is_some_and(|value| value.trim().is_empty())
                {
                    validation_error!(
                        "upstream '{}' auth.external_auth.oidc.audience must be non-empty when provided",
                        upstream_name
                    );
                    return false;
                }
                if scopes.iter().any(|scope| scope.trim().is_empty()) {
                    validation_error!(
                        "upstream '{}' auth.external_auth.oidc.scopes must not contain empty values",
                        upstream_name
                    );
                    return false;
                }
                if !validate_external_auth_headers(
                    upstream_name,
                    "auth.external_auth.oidc",
                    request_headers,
                    response_header_allowlist,
                ) {
                    return false;
                }
                if !response_header_allowlist.is_empty() {
                    validation_error!(
                        "upstream '{}' auth.external_auth.oidc.response_header_allowlist is not supported in v1",
                        upstream_name
                    );
                    return false;
                }
                if *timeout_ms == 0 {
                    validation_error!(
                        "upstream '{}' auth.external_auth.oidc.timeout_ms must be greater than 0",
                        upstream_name
                    );
                    return false;
                }
            }
        }
    }

    if let Some(jwt) = upstream.auth.jwt.as_ref() {
        let has_hs256 = jwt
            .allowed_algorithms
            .iter()
            .any(|alg| matches!(alg, crate::config::JwtAlgorithm::Hs256));
        let has_asymmetric_alg = jwt.allowed_algorithms.iter().any(|alg| {
            matches!(
                alg,
                crate::config::JwtAlgorithm::Rs256 | crate::config::JwtAlgorithm::Es256
            )
        });

        if jwt.allowed_algorithms.is_empty() {
            validation_error!(
                "upstream '{}' auth.jwt.allowed_algorithms must contain at least one algorithm",
                upstream_name
            );
            return false;
        }
        if !validate_secret_source_exclusivity(
            !jwt.secret.trim().is_empty(),
            jwt.secret_ref.as_ref(),
            &format!("upstream '{}' auth.jwt.secret", upstream_name),
            &format!("upstream '{}' auth.jwt.secret_ref", upstream_name),
        ) {
            return false;
        }
        let has_hs256_secret_source = !jwt.secret.trim().is_empty() || jwt.secret_ref.is_some();
        if has_hs256 && !has_hs256_secret_source {
            validation_error!(
                "upstream '{}' auth.jwt.secret or auth.jwt.secret_ref must be configured when HS256 is enabled",
                upstream_name
            );
            return false;
        }
        if has_hs256_secret_source && !has_hs256 {
            validation_error!(
                "upstream '{}' auth.jwt.secret/auth.jwt.secret_ref requires auth.jwt.allowed_algorithms to include HS256",
                upstream_name
            );
            return false;
        }
        if jwt
            .issuer
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            validation_error!(
                "upstream '{}' auth.jwt.issuer must be non-empty when provided",
                upstream_name
            );
            return false;
        }
        if jwt
            .audience
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            validation_error!(
                "upstream '{}' auth.jwt.audience must be non-empty when provided",
                upstream_name
            );
            return false;
        }
        if jwt.issuer.is_some() && jwt.issuers.is_some() {
            validation_error!(
                "upstream '{}' auth.jwt.issuer and auth.jwt.issuers cannot both be set",
                upstream_name
            );
            return false;
        }
        if jwt.audience.is_some() && jwt.audiences.is_some() {
            validation_error!(
                "upstream '{}' auth.jwt.audience and auth.jwt.audiences cannot both be set",
                upstream_name
            );
            return false;
        }
        if let Some(issuers) = jwt.issuers.as_ref()
            && (issuers.is_empty() || issuers.iter().any(|value| value.trim().is_empty()))
        {
            validation_error!(
                "upstream '{}' auth.jwt.issuers must be non-empty and must not contain empty values",
                upstream_name
            );
            return false;
        }
        if let Some(audiences) = jwt.audiences.as_ref()
            && (audiences.is_empty() || audiences.iter().any(|value| value.trim().is_empty()))
        {
            validation_error!(
                "upstream '{}' auth.jwt.audiences must be non-empty and must not contain empty values",
                upstream_name
            );
            return false;
        }
        if jwt
            .jwks_url
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            validation_error!(
                "upstream '{}' auth.jwt.jwks_url must be non-empty when provided",
                upstream_name
            );
            return false;
        }
        if let Some(jwks_url) = jwt.jwks_url.as_deref()
            && !jwks_url.trim().is_empty()
            && !is_valid_https_url(jwks_url)
        {
            validation_error!(
                "upstream '{}' auth.jwt.jwks_url must be an absolute https URL",
                upstream_name
            );
            return false;
        }
        if jwt.jwks_url.is_some() && !has_asymmetric_alg {
            validation_error!(
                "upstream '{}' auth.jwt.jwks_url requires auth.jwt.allowed_algorithms to include RS256 or ES256",
                upstream_name
            );
            return false;
        }
        if !jwt.static_keys.is_empty() && !has_asymmetric_alg {
            validation_error!(
                "upstream '{}' auth.jwt.static_keys require auth.jwt.allowed_algorithms to include RS256 or ES256",
                upstream_name
            );
            return false;
        }
        if jwt.jwks_url.is_some() {
            if jwt.jwks_refresh_interval_secs == 0 {
                validation_error!(
                    "upstream '{}' auth.jwt.jwks_refresh_interval_secs must be greater than 0",
                    upstream_name
                );
                return false;
            }
            if jwt.jwks_request_timeout_ms == 0 {
                validation_error!(
                    "upstream '{}' auth.jwt.jwks_request_timeout_ms must be greater than 0",
                    upstream_name
                );
                return false;
            }
            if jwt.jwks_cache_ttl_secs == 0 {
                validation_error!(
                    "upstream '{}' auth.jwt.jwks_cache_ttl_secs must be greater than 0",
                    upstream_name
                );
                return false;
            }
        }
        let mut jwt_key_fingerprints: HashMap<String, String> = HashMap::new();
        for (index, key) in jwt.static_keys.iter().enumerate() {
            match key {
                JwtVerificationKey::Pem {
                    kid,
                    alg,
                    public_key_pem,
                    ..
                } => {
                    if kid.as_deref().is_some_and(|value| value.trim().is_empty()) {
                        validation_error!(
                            "upstream '{}' auth.jwt.static_keys[{}].kid must be non-empty when provided",
                            upstream_name,
                            index
                        );
                        return false;
                    }
                    if public_key_pem.trim().is_empty() {
                        validation_error!(
                            "upstream '{}' auth.jwt.static_keys[{}].public_key_pem must be non-empty",
                            upstream_name,
                            index
                        );
                        return false;
                    }
                    if let Some(kid) = kid
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        let fingerprint = format!("pem:{:?}:{}", alg, public_key_pem.trim());
                        if let Some(existing) =
                            jwt_key_fingerprints.insert(kid.to_string(), fingerprint.clone())
                            && existing != fingerprint
                        {
                            validation_error!(
                                "upstream '{}' auth.jwt.static_keys contains conflicting entries for kid '{}'",
                                upstream_name,
                                kid
                            );
                            return false;
                        }
                    }
                }
                JwtVerificationKey::Jwk { kid, jwk, alg } => {
                    if kid.as_deref().is_some_and(|value| value.trim().is_empty()) {
                        validation_error!(
                            "upstream '{}' auth.jwt.static_keys[{}].kid must be non-empty when provided",
                            upstream_name,
                            index
                        );
                        return false;
                    }
                    if jwk.trim().is_empty() {
                        validation_error!(
                            "upstream '{}' auth.jwt.static_keys[{}].jwk must be non-empty",
                            upstream_name,
                            index
                        );
                        return false;
                    }
                    if let Some(kid) = kid
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        let fingerprint = format!("jwk:{:?}:{}", alg, jwk.trim());
                        if let Some(existing) =
                            jwt_key_fingerprints.insert(kid.to_string(), fingerprint.clone())
                            && existing != fingerprint
                        {
                            validation_error!(
                                "upstream '{}' auth.jwt.static_keys contains conflicting entries for kid '{}'",
                                upstream_name,
                                kid
                            );
                            return false;
                        }
                    }
                }
            }
        }
        if !has_hs256_secret_source && jwt.static_keys.is_empty() && jwt.jwks_url.is_none() {
            validation_error!(
                "upstream '{}' auth.jwt must configure at least one key source",
                upstream_name
            );
            return false;
        }
    }
    if upstream
        .auth
        .required_scopes
        .iter()
        .any(|value| value.trim().is_empty())
    {
        validation_error!(
            "upstream '{}' auth.required_scopes must not contain empty values",
            upstream_name
        );
        return false;
    }
    if upstream
        .auth
        .required_roles
        .iter()
        .any(|value| value.trim().is_empty())
    {
        validation_error!(
            "upstream '{}' auth.required_roles must not contain empty values",
            upstream_name
        );
        return false;
    }
    if (!upstream.auth.required_scopes.is_empty() || !upstream.auth.required_roles.is_empty())
        && upstream.auth.jwt.is_none()
    {
        validation_error!(
            "upstream '{}' auth.required_scopes/auth.required_roles require auth.jwt",
            upstream_name
        );
        return false;
    }

    true
}
