use std::{collections::HashMap, time::Duration};

use super::{config_invalid, normalize_optional_string};
use crate::runtime::RuntimeConfigError;
use crate::validator::{is_valid_https_or_loopback_http_url, is_valid_https_url};

fn normalize_string_vec(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn normalize_nonempty_string_vec(
    field_name: &str,
    values: &[String],
) -> Result<Vec<String>, RuntimeConfigError> {
    let normalized = normalize_string_vec(values);
    if normalized.len() != values.len() {
        return Err(config_invalid(format!(
            "{field_name} must not contain empty values"
        )));
    }
    Ok(normalized)
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeApiKeyAuth {
    pub header_name: String,
    pub keys: Vec<String>,
}

impl RuntimeApiKeyAuth {
    pub(crate) fn normalize(
        api_key: &crate::config::ApiKeyAuth,
        upstream_name: &str,
    ) -> Result<Self, RuntimeConfigError> {
        let header_name = api_key.header_name.trim();
        if header_name.is_empty() {
            return Err(config_invalid(format!(
                "upstream '{upstream_name}' auth.api_key.header_name must be non-empty"
            )));
        }
        let keys = normalize_nonempty_string_vec(
            &format!("upstream '{upstream_name}' auth.api_key.keys"),
            &api_key.keys,
        )?;
        Ok(Self {
            header_name: header_name.to_string(),
            keys,
        })
    }

    #[cfg(test)]
    pub(crate) fn as_config(&self) -> crate::config::ApiKeyAuth {
        crate::config::ApiKeyAuth {
            header_name: self.header_name.clone(),
            keys: self.keys.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeJwtAuth {
    pub secret: String,
    pub issuer: Option<String>,
    pub audience: Option<String>,
    pub issuers: Vec<String>,
    pub audiences: Vec<String>,
    pub allowed_algorithms: Vec<crate::config::JwtAlgorithm>,
    pub require_kid: bool,
    pub static_keys: Vec<RuntimeJwtVerificationKey>,
    pub jwks_url: Option<String>,
    pub jwks_refresh_interval: Duration,
    pub jwks_request_timeout: Duration,
    pub jwks_cache_ttl: Duration,
    pub jwks_stale_if_error: Duration,
    pub jwks_startup_behavior: crate::config::JwksStartupBehavior,
    pub clock_skew: Duration,
}

impl Default for RuntimeJwtAuth {
    fn default() -> Self {
        let defaults = crate::config::JwtAuth::default();
        Self {
            secret: defaults.secret,
            issuer: defaults.issuer,
            audience: defaults.audience,
            issuers: Vec::new(),
            audiences: Vec::new(),
            allowed_algorithms: defaults.allowed_algorithms,
            require_kid: defaults.require_kid,
            static_keys: Vec::new(),
            jwks_url: defaults.jwks_url,
            jwks_refresh_interval: Duration::from_secs(defaults.jwks_refresh_interval_secs),
            jwks_request_timeout: Duration::from_millis(defaults.jwks_request_timeout_ms),
            jwks_cache_ttl: Duration::from_secs(defaults.jwks_cache_ttl_secs),
            jwks_stale_if_error: Duration::from_secs(defaults.jwks_stale_if_error_secs),
            jwks_startup_behavior: defaults.jwks_startup_behavior,
            clock_skew: Duration::from_secs(defaults.clock_skew_secs),
        }
    }
}

impl RuntimeJwtAuth {
    pub(crate) fn normalize(
        jwt: &crate::config::JwtAuth,
        upstream_name: &str,
    ) -> Result<Self, RuntimeConfigError> {
        let secret = jwt.secret.trim();
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

        if has_hs256 && secret.is_empty() {
            return Err(config_invalid(format!(
                "upstream '{upstream_name}' auth.jwt.secret must be non-empty"
            )));
        }
        if !secret.is_empty() && !has_hs256 {
            return Err(config_invalid(format!(
                "upstream '{upstream_name}' auth.jwt.secret requires auth.jwt.allowed_algorithms to include HS256"
            )));
        }

        let issuer = normalize_optional_string(jwt.issuer.as_deref());
        let audience = normalize_optional_string(jwt.audience.as_deref());
        let issuers = normalize_optional_string_vec(
            jwt.issuers.as_deref(),
            &format!("upstream '{upstream_name}' auth.jwt.issuers"),
        )?;
        let audiences = normalize_optional_string_vec(
            jwt.audiences.as_deref(),
            &format!("upstream '{upstream_name}' auth.jwt.audiences"),
        )?;

        if issuer.is_some() && !issuers.is_empty() {
            return Err(config_invalid(format!(
                "upstream '{upstream_name}' auth.jwt.issuer and auth.jwt.issuers cannot both be set"
            )));
        }
        if audience.is_some() && !audiences.is_empty() {
            return Err(config_invalid(format!(
                "upstream '{upstream_name}' auth.jwt.audience and auth.jwt.audiences cannot both be set"
            )));
        }

        let allowed_algorithms = normalize_algorithms(&jwt.allowed_algorithms, upstream_name)?;
        let static_keys =
            normalize_static_keys(&jwt.static_keys, upstream_name, has_asymmetric_alg)?;
        let jwks_url = normalize_optional_string(jwt.jwks_url.as_deref());
        if let Some(jwks_url) = jwks_url.as_deref()
            && !is_valid_https_url(jwks_url)
        {
            return Err(config_invalid(format!(
                "upstream '{upstream_name}' auth.jwt.jwks_url must be an absolute https URL"
            )));
        }

        if jwks_url.is_some() && !has_asymmetric_alg {
            return Err(config_invalid(format!(
                "upstream '{upstream_name}' auth.jwt.jwks_url requires auth.jwt.allowed_algorithms to include RS256 or ES256"
            )));
        }
        if !static_keys.is_empty() && !has_asymmetric_alg {
            return Err(config_invalid(format!(
                "upstream '{upstream_name}' auth.jwt.static_keys require auth.jwt.allowed_algorithms to include RS256 or ES256"
            )));
        }
        if secret.is_empty() && static_keys.is_empty() && jwks_url.is_none() {
            return Err(config_invalid(format!(
                "upstream '{upstream_name}' auth.jwt must configure at least one key source"
            )));
        }

        Ok(Self {
            secret: secret.to_string(),
            issuer,
            audience,
            issuers,
            audiences,
            allowed_algorithms,
            require_kid: jwt.require_kid,
            static_keys,
            jwks_url,
            jwks_refresh_interval: Duration::from_secs(jwt.jwks_refresh_interval_secs),
            jwks_request_timeout: Duration::from_millis(jwt.jwks_request_timeout_ms),
            jwks_cache_ttl: Duration::from_secs(jwt.jwks_cache_ttl_secs),
            jwks_stale_if_error: Duration::from_secs(jwt.jwks_stale_if_error_secs),
            jwks_startup_behavior: jwt.jwks_startup_behavior.clone(),
            clock_skew: Duration::from_secs(jwt.clock_skew_secs),
        })
    }

    #[cfg(test)]
    pub(crate) fn as_config(&self) -> crate::config::JwtAuth {
        crate::config::JwtAuth {
            secret: self.secret.clone(),
            secret_ref: None,
            issuer: self.issuer.clone(),
            audience: self.audience.clone(),
            issuers: (!self.issuers.is_empty()).then_some(self.issuers.clone()),
            audiences: (!self.audiences.is_empty()).then_some(self.audiences.clone()),
            allowed_algorithms: self.allowed_algorithms.clone(),
            require_kid: self.require_kid,
            static_keys: self
                .static_keys
                .iter()
                .map(RuntimeJwtVerificationKey::as_config)
                .collect(),
            jwks_url: self.jwks_url.clone(),
            jwks_refresh_interval_secs: self.jwks_refresh_interval.as_secs(),
            jwks_request_timeout_ms: self.jwks_request_timeout.as_millis() as u64,
            jwks_cache_ttl_secs: self.jwks_cache_ttl.as_secs(),
            jwks_stale_if_error_secs: self.jwks_stale_if_error.as_secs(),
            jwks_startup_behavior: self.jwks_startup_behavior.clone(),
            clock_skew_secs: self.clock_skew.as_secs(),
        }
    }
}

fn normalize_optional_string_vec(
    values: Option<&[String]>,
    field_name: &str,
) -> Result<Vec<String>, RuntimeConfigError> {
    match values {
        Some(values) => normalize_nonempty_string_vec(field_name, values),
        None => Ok(Vec::new()),
    }
}

fn normalize_algorithms(
    algorithms: &[crate::config::JwtAlgorithm],
    upstream_name: &str,
) -> Result<Vec<crate::config::JwtAlgorithm>, RuntimeConfigError> {
    if algorithms.is_empty() {
        return Err(config_invalid(format!(
            "upstream '{upstream_name}' auth.jwt.allowed_algorithms must contain at least one algorithm"
        )));
    }

    let mut normalized = Vec::with_capacity(algorithms.len());
    for algorithm in algorithms {
        if !normalized.contains(algorithm) {
            normalized.push(*algorithm);
        }
    }
    Ok(normalized)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeJwtVerificationKey {
    Pem {
        kid: Option<String>,
        alg: Option<crate::config::JwtAlgorithm>,
        public_key_pem: String,
    },
    Jwk {
        kid: Option<String>,
        alg: Option<crate::config::JwtAlgorithm>,
        jwk: String,
    },
}

impl RuntimeJwtVerificationKey {
    fn normalize(
        key: &crate::config::JwtVerificationKey,
        field_name: &str,
    ) -> Result<Self, RuntimeConfigError> {
        match key {
            crate::config::JwtVerificationKey::Pem {
                kid,
                alg,
                public_key_pem,
            } => {
                let public_key_pem = public_key_pem.trim();
                if public_key_pem.is_empty() {
                    return Err(config_invalid(format!(
                        "{field_name}.public_key_pem must be non-empty"
                    )));
                }
                Ok(Self::Pem {
                    kid: normalize_optional_string(kid.as_deref()),
                    alg: *alg,
                    public_key_pem: public_key_pem.to_string(),
                })
            }
            crate::config::JwtVerificationKey::Jwk { kid, alg, jwk } => {
                let jwk = jwk.trim();
                if jwk.is_empty() {
                    return Err(config_invalid(format!(
                        "{field_name}.jwk must be non-empty"
                    )));
                }
                Ok(Self::Jwk {
                    kid: normalize_optional_string(kid.as_deref()),
                    alg: *alg,
                    jwk: jwk.to_string(),
                })
            }
        }
    }

    fn kid(&self) -> Option<&str> {
        match self {
            Self::Pem { kid, .. } | Self::Jwk { kid, .. } => kid.as_deref(),
        }
    }

    fn material_fingerprint(&self) -> String {
        match self {
            Self::Pem {
                alg,
                public_key_pem,
                ..
            } => format!("pem:{alg:?}:{public_key_pem}"),
            Self::Jwk { alg, jwk, .. } => format!("jwk:{alg:?}:{jwk}"),
        }
    }

    #[cfg(test)]
    fn as_config(&self) -> crate::config::JwtVerificationKey {
        match self {
            Self::Pem {
                kid,
                alg,
                public_key_pem,
            } => crate::config::JwtVerificationKey::Pem {
                kid: kid.clone(),
                alg: *alg,
                public_key_pem: public_key_pem.clone(),
            },
            Self::Jwk { kid, alg, jwk } => crate::config::JwtVerificationKey::Jwk {
                kid: kid.clone(),
                alg: *alg,
                jwk: jwk.clone(),
            },
        }
    }
}

fn normalize_static_keys(
    keys: &[crate::config::JwtVerificationKey],
    upstream_name: &str,
    has_asymmetric_alg: bool,
) -> Result<Vec<RuntimeJwtVerificationKey>, RuntimeConfigError> {
    if !keys.is_empty() && !has_asymmetric_alg {
        return Err(config_invalid(format!(
            "upstream '{upstream_name}' auth.jwt.static_keys require auth.jwt.allowed_algorithms to include RS256 or ES256"
        )));
    }

    let normalized = keys
        .iter()
        .enumerate()
        .map(|(index, key)| {
            RuntimeJwtVerificationKey::normalize(
                key,
                &format!("upstream '{upstream_name}' auth.jwt.static_keys[{index}]"),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut fingerprints_by_kid: HashMap<String, String> = HashMap::new();
    for key in &normalized {
        let Some(kid) = key.kid() else {
            continue;
        };
        let fingerprint = key.material_fingerprint();
        if let Some(existing) = fingerprints_by_kid.insert(kid.to_string(), fingerprint.clone())
            && existing != fingerprint
        {
            return Err(config_invalid(format!(
                "upstream '{upstream_name}' auth.jwt.static_keys contains conflicting entries for kid '{kid}'"
            )));
        }
    }

    Ok(normalized)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RuntimeExternalAuthFailureMode {
    FailOpen,
    #[default]
    FailClosed,
}

impl RuntimeExternalAuthFailureMode {
    pub(crate) fn from_config(mode: crate::config::ExternalAuthFailureMode) -> Self {
        match mode {
            crate::config::ExternalAuthFailureMode::FailOpen => Self::FailOpen,
            crate::config::ExternalAuthFailureMode::FailClosed => Self::FailClosed,
        }
    }

    #[cfg(test)]
    pub(crate) fn as_config(self) -> crate::config::ExternalAuthFailureMode {
        match self {
            Self::FailOpen => crate::config::ExternalAuthFailureMode::FailOpen,
            Self::FailClosed => crate::config::ExternalAuthFailureMode::FailClosed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeExternalAuthRequestHeader {
    pub name: String,
    pub value: String,
}

impl RuntimeExternalAuthRequestHeader {
    fn normalize(
        header: &crate::config::ExternalAuthRequestHeader,
        field_name: &str,
    ) -> Result<Self, RuntimeConfigError> {
        let name = header.name.trim();
        if name.is_empty() {
            return Err(config_invalid(format!(
                "{field_name}.name must be non-empty"
            )));
        }
        http::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            config_invalid(format!(
                "{field_name}.name must be a valid HTTP header name"
            ))
        })?;

        Ok(Self {
            name: name.to_string(),
            value: header.value.clone(),
        })
    }

    fn normalize_many(
        headers: &[crate::config::ExternalAuthRequestHeader],
        field_name: &str,
    ) -> Result<Vec<Self>, RuntimeConfigError> {
        headers
            .iter()
            .enumerate()
            .map(|(index, header)| Self::normalize(header, &format!("{field_name}[{index}]")))
            .collect()
    }

    #[cfg(test)]
    fn as_config(&self) -> crate::config::ExternalAuthRequestHeader {
        crate::config::ExternalAuthRequestHeader {
            name: self.name.clone(),
            value: self.value.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeExternalAuth {
    Http {
        endpoint: String,
        request_headers: Vec<RuntimeExternalAuthRequestHeader>,
        response_header_allowlist: Vec<String>,
        timeout: Duration,
        failure_mode: RuntimeExternalAuthFailureMode,
    },
    Oidc {
        discovery_url: Option<String>,
        issuer_url: Option<String>,
        client_id: String,
        client_secret: Option<String>,
        audience: Option<String>,
        scopes: Vec<String>,
        request_headers: Vec<RuntimeExternalAuthRequestHeader>,
        response_header_allowlist: Vec<String>,
        timeout: Duration,
        failure_mode: RuntimeExternalAuthFailureMode,
    },
}

impl RuntimeExternalAuth {
    fn normalize(
        external_auth: &crate::config::ExternalAuth,
        upstream_name: &str,
    ) -> Result<Self, RuntimeConfigError> {
        match external_auth {
            crate::config::ExternalAuth::Http {
                endpoint,
                request_headers,
                response_header_allowlist,
                timeout_ms,
                failure_mode,
            } => {
                if !is_valid_https_or_loopback_http_url(endpoint) {
                    return Err(config_invalid(format!(
                        "upstream '{upstream_name}' auth.external_auth.http.endpoint must be an absolute https URL or loopback http URL"
                    )));
                }
                if *timeout_ms == 0 {
                    return Err(config_invalid(format!(
                        "upstream '{upstream_name}' auth.external_auth.http.timeout_ms must be greater than 0"
                    )));
                }
                Ok(Self::Http {
                    endpoint: endpoint.clone(),
                    request_headers: RuntimeExternalAuthRequestHeader::normalize_many(
                        request_headers,
                        &format!(
                            "upstream '{upstream_name}' auth.external_auth.http.request_headers"
                        ),
                    )?,
                    response_header_allowlist: normalize_nonempty_string_vec(
                        &format!(
                            "upstream '{upstream_name}' auth.external_auth.http.response_header_allowlist"
                        ),
                        response_header_allowlist,
                    )?,
                    timeout: Duration::from_millis(*timeout_ms),
                    failure_mode: RuntimeExternalAuthFailureMode::from_config(*failure_mode),
                })
            }
            crate::config::ExternalAuth::Oidc {
                discovery_url,
                issuer_url,
                client_id,
                client_secret,
                client_secret_ref: _,
                audience,
                scopes,
                request_headers,
                response_header_allowlist,
                timeout_ms,
                failure_mode,
            } => {
                if *timeout_ms == 0 {
                    return Err(config_invalid(format!(
                        "upstream '{upstream_name}' auth.external_auth.oidc.timeout_ms must be greater than 0"
                    )));
                }
                if let Some(discovery_url) = discovery_url.as_deref()
                    && !discovery_url.trim().is_empty()
                    && !is_valid_https_or_loopback_http_url(discovery_url)
                {
                    return Err(config_invalid(format!(
                        "upstream '{upstream_name}' auth.external_auth.oidc.discovery_url must be an absolute https URL or loopback http URL"
                    )));
                }
                if let Some(issuer_url) = issuer_url.as_deref()
                    && !issuer_url.trim().is_empty()
                    && !is_valid_https_or_loopback_http_url(issuer_url)
                {
                    return Err(config_invalid(format!(
                        "upstream '{upstream_name}' auth.external_auth.oidc.issuer_url must be an absolute https URL or loopback http URL"
                    )));
                }
                let client_id = client_id.trim();
                if client_id.is_empty() {
                    return Err(config_invalid(format!(
                        "upstream '{upstream_name}' auth.external_auth.oidc.client_id must be non-empty"
                    )));
                }
                Ok(Self::Oidc {
                    discovery_url: normalize_optional_string(discovery_url.as_deref()),
                    issuer_url: normalize_optional_string(issuer_url.as_deref()),
                    client_id: client_id.to_string(),
                    client_secret: normalize_optional_string(client_secret.as_deref()),
                    audience: normalize_optional_string(audience.as_deref()),
                    scopes: normalize_nonempty_string_vec(
                        &format!("upstream '{upstream_name}' auth.external_auth.oidc.scopes"),
                        scopes,
                    )?,
                    request_headers: RuntimeExternalAuthRequestHeader::normalize_many(
                        request_headers,
                        &format!(
                            "upstream '{upstream_name}' auth.external_auth.oidc.request_headers"
                        ),
                    )?,
                    response_header_allowlist: normalize_nonempty_string_vec(
                        &format!(
                            "upstream '{upstream_name}' auth.external_auth.oidc.response_header_allowlist"
                        ),
                        response_header_allowlist,
                    )?,
                    timeout: Duration::from_millis(*timeout_ms),
                    failure_mode: RuntimeExternalAuthFailureMode::from_config(*failure_mode),
                })
            }
        }
    }

    #[cfg(test)]
    fn as_config(&self) -> crate::config::ExternalAuth {
        match self {
            Self::Http {
                endpoint,
                request_headers,
                response_header_allowlist,
                timeout,
                failure_mode,
            } => crate::config::ExternalAuth::Http {
                endpoint: endpoint.clone(),
                request_headers: request_headers.iter().map(Self::header_as_config).collect(),
                response_header_allowlist: response_header_allowlist.clone(),
                timeout_ms: timeout.as_millis().try_into().unwrap_or(u64::MAX),
                failure_mode: failure_mode.as_config(),
            },
            Self::Oidc {
                discovery_url,
                issuer_url,
                client_id,
                client_secret,
                audience,
                scopes,
                request_headers,
                response_header_allowlist,
                timeout,
                failure_mode,
            } => crate::config::ExternalAuth::Oidc {
                discovery_url: discovery_url.clone(),
                issuer_url: issuer_url.clone(),
                client_id: client_id.clone(),
                client_secret: client_secret.clone(),
                client_secret_ref: None,
                audience: audience.clone(),
                scopes: scopes.clone(),
                request_headers: request_headers.iter().map(Self::header_as_config).collect(),
                response_header_allowlist: response_header_allowlist.clone(),
                timeout_ms: timeout.as_millis().try_into().unwrap_or(u64::MAX),
                failure_mode: failure_mode.as_config(),
            },
        }
    }

    #[cfg(test)]
    fn header_as_config(
        header: &RuntimeExternalAuthRequestHeader,
    ) -> crate::config::ExternalAuthRequestHeader {
        header.as_config()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeAuthPolicy {
    pub api_key: Option<RuntimeApiKeyAuth>,
    pub jwt: Option<RuntimeJwtAuth>,
    pub external_auth: Option<RuntimeExternalAuth>,
    pub required_scopes: Vec<String>,
    pub required_roles: Vec<String>,
}

impl RuntimeAuthPolicy {
    pub(crate) fn normalize(
        auth: &crate::config::RouteAuth,
        upstream_name: &str,
    ) -> Result<Self, RuntimeConfigError> {
        Ok(Self {
            api_key: auth
                .api_key
                .as_ref()
                .map(|api_key| RuntimeApiKeyAuth::normalize(api_key, upstream_name))
                .transpose()?,
            jwt: auth
                .jwt
                .as_ref()
                .map(|jwt| RuntimeJwtAuth::normalize(jwt, upstream_name))
                .transpose()?,
            external_auth: auth
                .external_auth
                .as_ref()
                .map(|external_auth| RuntimeExternalAuth::normalize(external_auth, upstream_name))
                .transpose()?,
            required_scopes: normalize_nonempty_string_vec(
                &format!("upstream '{upstream_name}' auth.required_scopes"),
                &auth.required_scopes,
            )?,
            required_roles: normalize_nonempty_string_vec(
                &format!("upstream '{upstream_name}' auth.required_roles"),
                &auth.required_roles,
            )?,
        })
    }

    #[cfg(test)]
    pub(crate) fn as_config(&self) -> crate::config::RouteAuth {
        crate::config::RouteAuth {
            api_key: self.api_key.as_ref().map(RuntimeApiKeyAuth::as_config),
            jwt: self.jwt.as_ref().map(RuntimeJwtAuth::as_config),
            external_auth: self
                .external_auth
                .as_ref()
                .map(RuntimeExternalAuth::as_config),
            required_scopes: self.required_scopes.clone(),
            required_roles: self.required_roles.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ApiKeyAuth, ExternalAuth, ExternalAuthFailureMode, ExternalAuthRequestHeader, JwtAuth,
        RouteAuth,
    };

    fn assert_config_invalid(err: RuntimeConfigError, expected: impl AsRef<str>) {
        let expected = expected.as_ref();
        assert_eq!(err.category(), "config_invalid");
        assert_eq!(err.to_string(), format!("config_invalid: {expected}"));
    }

    #[test]
    fn api_key_auth_normalization_trims_header_and_keys() {
        let api_key = ApiKeyAuth {
            header_name: "  x-api-key  ".to_string(),
            keys: vec![" primary ".to_string(), "backup".to_string()],
        };

        let normalized = RuntimeApiKeyAuth::normalize(&api_key, "payments").expect("api key auth");

        assert_eq!(normalized.header_name, "x-api-key");
        assert_eq!(normalized.keys, vec!["primary", "backup"]);
    }

    #[test]
    fn jwt_auth_normalization_trims_optional_fields_and_converts_clock_skew() {
        let jwt = JwtAuth {
            secret: "  signing-secret  ".to_string(),
            issuer: Some("  issuer.example  ".to_string()),
            audience: Some("  payments-api  ".to_string()),
            clock_skew_secs: 45,
            ..JwtAuth::default()
        };

        let normalized = RuntimeJwtAuth::normalize(&jwt, "payments").expect("jwt auth");

        assert_eq!(normalized.secret, "signing-secret");
        assert_eq!(normalized.issuer.as_deref(), Some("issuer.example"));
        assert_eq!(normalized.audience.as_deref(), Some("payments-api"));
        assert_eq!(normalized.clock_skew, Duration::from_secs(45));
    }

    #[test]
    fn jwt_auth_normalization_rejects_non_https_jwks_url() {
        let jwt = JwtAuth {
            secret: String::new(),
            allowed_algorithms: vec![crate::config::JwtAlgorithm::Rs256],
            jwks_url: Some("http://issuer.example/.well-known/jwks.json".to_string()),
            ..JwtAuth::default()
        };

        let err =
            RuntimeJwtAuth::normalize(&jwt, "payments").expect_err("non-https jwks url must fail");

        assert_config_invalid(
            err,
            "upstream 'payments' auth.jwt.jwks_url must be an absolute https URL",
        );
    }

    #[test]
    fn external_http_auth_normalization_preserves_failure_mode_and_request_headers() {
        let external_auth = ExternalAuth::Http {
            endpoint: "http://127.0.0.1:8080/check".to_string(),
            request_headers: vec![ExternalAuthRequestHeader {
                name: "  x-tenant-id  ".to_string(),
                value: "{route.tenant}".to_string(),
            }],
            response_header_allowlist: vec![" x-auth-user ".to_string(), "x-auth-role".to_string()],
            timeout_ms: 2_500,
            failure_mode: ExternalAuthFailureMode::FailOpen,
        };

        let normalized =
            RuntimeExternalAuth::normalize(&external_auth, "payments").expect("external auth");

        assert_eq!(
            normalized,
            RuntimeExternalAuth::Http {
                endpoint: "http://127.0.0.1:8080/check".to_string(),
                request_headers: vec![RuntimeExternalAuthRequestHeader {
                    name: "x-tenant-id".to_string(),
                    value: "{route.tenant}".to_string(),
                }],
                response_header_allowlist: vec![
                    "x-auth-user".to_string(),
                    "x-auth-role".to_string()
                ],
                timeout: Duration::from_millis(2_500),
                failure_mode: RuntimeExternalAuthFailureMode::FailOpen,
            }
        );
    }

    #[test]
    fn external_oidc_auth_normalization_shapes_client_and_scope_fields() {
        let external_auth = ExternalAuth::Oidc {
            discovery_url: Some(" https://issuer.example/.well-known/openid-configuration ".into()),
            issuer_url: Some(" https://issuer.example ".into()),
            client_id: "  impulse-edge  ".to_string(),
            client_secret: Some("  secret-value  ".to_string()),
            client_secret_ref: None,
            audience: Some("  payments-api  ".to_string()),
            scopes: vec!["openid".to_string(), " profile ".to_string()],
            request_headers: vec![ExternalAuthRequestHeader {
                name: " x-request-id ".to_string(),
                value: "{trace.id}".to_string(),
            }],
            response_header_allowlist: vec![" x-user ".to_string()],
            timeout_ms: 3_000,
            failure_mode: ExternalAuthFailureMode::FailClosed,
        };

        let normalized =
            RuntimeExternalAuth::normalize(&external_auth, "payments").expect("oidc auth");

        assert_eq!(
            normalized,
            RuntimeExternalAuth::Oidc {
                discovery_url: Some(
                    "https://issuer.example/.well-known/openid-configuration".to_string(),
                ),
                issuer_url: Some("https://issuer.example".to_string()),
                client_id: "impulse-edge".to_string(),
                client_secret: Some("secret-value".to_string()),
                audience: Some("payments-api".to_string()),
                scopes: vec!["openid".to_string(), "profile".to_string()],
                request_headers: vec![RuntimeExternalAuthRequestHeader {
                    name: "x-request-id".to_string(),
                    value: "{trace.id}".to_string(),
                }],
                response_header_allowlist: vec!["x-user".to_string()],
                timeout: Duration::from_millis(3_000),
                failure_mode: RuntimeExternalAuthFailureMode::FailClosed,
            }
        );
    }

    #[test]
    fn external_oidc_auth_normalization_accepts_loopback_http_urls() {
        let external_auth = ExternalAuth::Oidc {
            discovery_url: Some(" http://127.0.0.1:9000/oidc ".into()),
            issuer_url: Some(" http://localhost:9000 ".into()),
            client_id: " impulse-edge ".to_string(),
            client_secret: None,
            client_secret_ref: None,
            audience: None,
            scopes: vec!["openid".to_string()],
            request_headers: Vec::new(),
            response_header_allowlist: Vec::new(),
            timeout_ms: 3_000,
            failure_mode: ExternalAuthFailureMode::FailClosed,
        };

        let normalized =
            RuntimeExternalAuth::normalize(&external_auth, "payments").expect("oidc auth");

        assert_eq!(
            normalized,
            RuntimeExternalAuth::Oidc {
                discovery_url: Some("http://127.0.0.1:9000/oidc".to_string()),
                issuer_url: Some("http://localhost:9000".to_string()),
                client_id: "impulse-edge".to_string(),
                client_secret: None,
                audience: None,
                scopes: vec!["openid".to_string()],
                request_headers: Vec::new(),
                response_header_allowlist: Vec::new(),
                timeout: Duration::from_millis(3_000),
                failure_mode: RuntimeExternalAuthFailureMode::FailClosed,
            }
        );
    }

    #[test]
    fn external_oidc_auth_normalization_rejects_non_loopback_http_urls() {
        let external_auth = ExternalAuth::Oidc {
            discovery_url: Some("http://issuer.example/oidc".into()),
            issuer_url: None,
            client_id: "impulse-edge".to_string(),
            client_secret: None,
            client_secret_ref: None,
            audience: None,
            scopes: vec!["openid".to_string()],
            request_headers: Vec::new(),
            response_header_allowlist: Vec::new(),
            timeout_ms: 3_000,
            failure_mode: ExternalAuthFailureMode::FailClosed,
        };

        let err = RuntimeExternalAuth::normalize(&external_auth, "payments")
            .expect_err("non-loopback http oidc discovery must fail");

        assert_config_invalid(
            err,
            "upstream 'payments' auth.external_auth.oidc.discovery_url must be an absolute https URL or loopback http URL",
        );
    }

    #[test]
    fn auth_policy_normalization_rejects_invalid_empty_values() {
        let auth = RouteAuth {
            required_scopes: vec!["payments:write".to_string(), "   ".to_string()],
            ..RouteAuth::default()
        };

        let err = RuntimeAuthPolicy::normalize(&auth, "payments").expect_err("empty scope");

        assert_config_invalid(
            err,
            "upstream 'payments' auth.required_scopes must not contain empty values",
        );
    }

    #[test]
    fn api_key_auth_normalization_rejects_empty_header_name() {
        let api_key = ApiKeyAuth {
            header_name: "   ".to_string(),
            keys: vec!["secret".to_string()],
        };

        let err = RuntimeApiKeyAuth::normalize(&api_key, "payments")
            .expect_err("empty api key header must fail");

        assert_config_invalid(
            err,
            "upstream 'payments' auth.api_key.header_name must be non-empty",
        );
    }

    #[test]
    fn jwt_auth_normalization_rejects_empty_secret() {
        let jwt = JwtAuth {
            secret: "   ".to_string(),
            ..JwtAuth::default()
        };

        let err =
            RuntimeJwtAuth::normalize(&jwt, "payments").expect_err("empty jwt secret must fail");

        assert_config_invalid(err, "upstream 'payments' auth.jwt.secret must be non-empty");
    }

    #[test]
    fn external_auth_normalization_rejects_invalid_header_name() {
        let external_auth = ExternalAuth::Http {
            endpoint: "http://auth.internal/check".to_string(),
            request_headers: vec![ExternalAuthRequestHeader {
                name: "bad header".to_string(),
                value: "value".to_string(),
            }],
            response_header_allowlist: Vec::new(),
            timeout_ms: 1_000,
            failure_mode: ExternalAuthFailureMode::FailClosed,
        };

        let err = RuntimeExternalAuth::normalize(&external_auth, "payments")
            .expect_err("invalid external auth header must fail");

        assert_config_invalid(
            err,
            "upstream 'payments' auth.external_auth.http.request_headers[0].name must be a valid HTTP header name",
        );
    }

    #[test]
    fn external_oidc_auth_normalization_rejects_empty_client_id() {
        let external_auth = ExternalAuth::Oidc {
            discovery_url: None,
            issuer_url: None,
            client_id: "   ".to_string(),
            client_secret: None,
            client_secret_ref: None,
            audience: None,
            scopes: vec!["openid".to_string()],
            request_headers: Vec::new(),
            response_header_allowlist: Vec::new(),
            timeout_ms: 1_000,
            failure_mode: ExternalAuthFailureMode::FailClosed,
        };

        let err = RuntimeExternalAuth::normalize(&external_auth, "payments")
            .expect_err("empty oidc client id must fail");

        assert_config_invalid(
            err,
            "upstream 'payments' auth.external_auth.oidc.client_id must be non-empty",
        );
    }
}
