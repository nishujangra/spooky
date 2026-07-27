use std::time::Duration;

use super::{config_invalid, normalize_optional_string};
use crate::runtime::RuntimeConfigError;

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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeJwtAuth {
    pub secret: String,
    pub issuer: Option<String>,
    pub audience: Option<String>,
    pub clock_skew: Duration,
}

impl RuntimeJwtAuth {
    pub(crate) fn normalize(
        jwt: &crate::config::JwtAuth,
        upstream_name: &str,
    ) -> Result<Self, RuntimeConfigError> {
        let secret = jwt.secret.trim();
        if secret.is_empty() {
            return Err(config_invalid(format!(
                "upstream '{upstream_name}' auth.jwt.secret must be non-empty"
            )));
        }

        Ok(Self {
            secret: secret.to_string(),
            issuer: normalize_optional_string(jwt.issuer.as_deref()),
            audience: normalize_optional_string(jwt.audience.as_deref()),
            clock_skew: Duration::from_secs(jwt.clock_skew_secs),
        })
    }

    #[cfg(test)]
    pub(crate) fn as_config(&self) -> crate::config::JwtAuth {
        crate::config::JwtAuth {
            secret: self.secret.clone(),
            issuer: self.issuer.clone(),
            audience: self.audience.clone(),
            clock_skew_secs: self.clock_skew.as_secs(),
        }
    }
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

    fn assert_config_invalid(
        err: RuntimeConfigError,
        expected: impl AsRef<str>,
    ) {
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
        };

        let normalized = RuntimeJwtAuth::normalize(&jwt, "payments").expect("jwt auth");

        assert_eq!(normalized.secret, "signing-secret");
        assert_eq!(normalized.issuer.as_deref(), Some("issuer.example"));
        assert_eq!(normalized.audience.as_deref(), Some("payments-api"));
        assert_eq!(normalized.clock_skew, Duration::from_secs(45));
    }

    #[test]
    fn external_http_auth_normalization_preserves_failure_mode_and_request_headers() {
        let external_auth = ExternalAuth::Http {
            endpoint: "http://auth.internal/check".to_string(),
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
                endpoint: "http://auth.internal/check".to_string(),
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
            client_id: "  spooky-edge  ".to_string(),
            client_secret: Some("  secret-value  ".to_string()),
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
                client_id: "spooky-edge".to_string(),
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

        let err = RuntimeJwtAuth::normalize(&jwt, "payments")
            .expect_err("empty jwt secret must fail");

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
