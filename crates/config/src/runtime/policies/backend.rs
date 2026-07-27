use std::time::Duration;

use super::config_invalid;
use crate::{
    backend_endpoint::{BackendEndpoint, BackendScheme},
    config::{HealthCheck, Performance, UpstreamTls},
    runtime::RuntimeConfigError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeBackendAddressKind {
    Hostname,
    IpLiteral,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBackendEndpoint {
    pub configured_address: String,
    pub canonical: BackendEndpoint,
    pub origin: String,
    pub authority_host: String,
    pub authority_port: u16,
    pub address_kind: RuntimeBackendAddressKind,
    pub transport_kind: super::RuntimeBackendTransportKind,
}

impl RuntimeBackendEndpoint {
    pub(crate) fn normalize(
        upstream_name: &str,
        backend_id: &str,
        address: &str,
    ) -> Result<Self, RuntimeConfigError> {
        let canonical = BackendEndpoint::parse(address).map_err(|reason| {
            RuntimeConfigError::BackendAddressInvalid {
                upstream: upstream_name.to_string(),
                backend: backend_id.to_string(),
                address: address.to_string(),
                reason,
            }
        })?;
        let authority_host = canonical.authority_host().to_string();
        let authority_port = canonical.authority_port();
        let address_kind = if canonical.authority_is_ip_literal() {
            RuntimeBackendAddressKind::IpLiteral
        } else {
            RuntimeBackendAddressKind::Hostname
        };
        let transport_kind = match canonical.scheme() {
            BackendScheme::Http => super::RuntimeBackendTransportKind::Http1,
            BackendScheme::Https => super::RuntimeBackendTransportKind::H2,
        };
        let origin = canonical.origin();

        Ok(Self {
            configured_address: address.to_string(),
            canonical,
            origin,
            authority_host,
            authority_port,
            address_kind,
            transport_kind,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBackendHealthCheck {
    pub path: String,
    pub interval: Duration,
    pub timeout: Duration,
    pub failure_threshold: u32,
    pub success_threshold: u32,
    pub cooldown: Duration,
}

impl RuntimeBackendHealthCheck {
    pub(crate) fn normalize(
        upstream_name: &str,
        backend_id: &str,
        health_check: &HealthCheck,
    ) -> Result<Self, RuntimeConfigError> {
        if health_check.interval == 0 {
            return Err(config_invalid(format!(
                "health check interval is invalid (0) for backend '{backend_id}' in upstream '{upstream_name}'"
            )));
        }
        if health_check.timeout_ms == 0 {
            return Err(config_invalid(format!(
                "health check timeout is invalid (0) for backend '{backend_id}' in upstream '{upstream_name}'"
            )));
        }
        if health_check.failure_threshold == 0 {
            return Err(config_invalid(format!(
                "health check failure threshold is invalid (0) for backend '{backend_id}' in upstream '{upstream_name}'"
            )));
        }
        if health_check.success_threshold == 0 {
            return Err(config_invalid(format!(
                "health check success threshold is invalid (0) for backend '{backend_id}' in upstream '{upstream_name}'"
            )));
        }

        Ok(Self {
            path: if health_check.path.trim().is_empty() {
                "/".to_string()
            } else {
                health_check.path.clone()
            },
            interval: Duration::from_millis(health_check.interval),
            timeout: Duration::from_millis(health_check.timeout_ms),
            failure_threshold: health_check.failure_threshold,
            success_threshold: health_check.success_threshold,
            cooldown: Duration::from_millis(health_check.cooldown_ms),
        })
    }

    #[cfg(test)]
    pub(crate) fn as_config(&self) -> HealthCheck {
        HealthCheck {
            path: self.path.clone(),
            interval: self.interval.as_millis().try_into().unwrap_or(u64::MAX),
            timeout_ms: self.timeout.as_millis().try_into().unwrap_or(u64::MAX),
            failure_threshold: self.failure_threshold,
            success_threshold: self.success_threshold,
            cooldown_ms: self.cooldown.as_millis().try_into().unwrap_or(u64::MAX),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBackendTlsPolicy {
    pub verify_certificates: bool,
    pub strict_sni: bool,
    pub ca_file: Option<String>,
    pub ca_dir: Option<String>,
}

impl RuntimeBackendTlsPolicy {
    pub(crate) fn from_effective_tls(effective_tls: &UpstreamTls) -> Self {
        Self {
            verify_certificates: effective_tls.verify_certificates,
            strict_sni: effective_tls.strict_sni,
            ca_file: effective_tls.ca_file.clone(),
            ca_dir: effective_tls.ca_dir.clone(),
        }
    }

    pub(crate) fn as_upstream_tls(&self) -> UpstreamTls {
        UpstreamTls {
            verify_certificates: self.verify_certificates,
            strict_sni: self.strict_sni,
            ca_file: self.ca_file.clone(),
            ca_dir: self.ca_dir.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBackendDnsPolicy {
    pub refresh_enabled: bool,
    pub refresh_interval: Duration,
}

impl RuntimeBackendDnsPolicy {
    pub(crate) fn from_performance(performance: &Performance) -> Self {
        Self {
            refresh_enabled: performance.backend_dns_refresh_enabled,
            refresh_interval: Duration::from_millis(performance.backend_dns_refresh_interval_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimeBackendTransportKind;

    fn assert_config_invalid(err: RuntimeConfigError, expected: impl AsRef<str>) {
        let expected = expected.as_ref();
        assert_eq!(err.category(), "config_invalid");
        assert_eq!(err.to_string(), format!("config_invalid: {expected}"));
    }

    #[test]
    fn runtime_backend_endpoint_normalizes_hostname_backend_and_canonical_authority() {
        let endpoint = RuntimeBackendEndpoint::normalize("payments", "primary", "api.example.com")
            .expect("hostname backend");

        assert_eq!(endpoint.configured_address, "api.example.com");
        assert_eq!(endpoint.origin, "https://api.example.com:443");
        assert_eq!(endpoint.authority_host, "api.example.com");
        assert_eq!(endpoint.authority_port, 443);
        assert_eq!(endpoint.address_kind, RuntimeBackendAddressKind::Hostname);
        assert_eq!(endpoint.transport_kind, RuntimeBackendTransportKind::H2);
        assert_eq!(endpoint.canonical.scheme(), BackendScheme::Https);
        assert_eq!(endpoint.canonical.authority(), "api.example.com:443");
    }

    #[test]
    fn runtime_backend_endpoint_normalizes_ip_literal_backend_and_canonical_authority() {
        let endpoint = RuntimeBackendEndpoint::normalize(
            "payments",
            "edge-ipv6",
            "https://[2001:db8::10]:9443",
        )
        .expect("ip literal backend");

        assert_eq!(endpoint.origin, "https://[2001:db8::10]:9443");
        assert_eq!(endpoint.authority_host, "2001:db8::10");
        assert_eq!(endpoint.authority_port, 9443);
        assert_eq!(endpoint.address_kind, RuntimeBackendAddressKind::IpLiteral);
        assert_eq!(endpoint.transport_kind, RuntimeBackendTransportKind::H2);
        assert_eq!(endpoint.canonical.authority(), "[2001:db8::10]:9443");
    }

    #[test]
    fn runtime_backend_endpoint_derives_transport_kind_from_scheme() {
        let http_endpoint =
            RuntimeBackendEndpoint::normalize("payments", "legacy", "http://127.0.0.1:8080")
                .expect("http backend");
        let https_endpoint =
            RuntimeBackendEndpoint::normalize("payments", "modern", "https://api.example.com:8443")
                .expect("https backend");

        assert_eq!(
            http_endpoint.transport_kind,
            RuntimeBackendTransportKind::Http1
        );
        assert_eq!(http_endpoint.authority_host, "127.0.0.1");
        assert_eq!(http_endpoint.authority_port, 8080);
        assert_eq!(
            http_endpoint.address_kind,
            RuntimeBackendAddressKind::IpLiteral
        );
        assert_eq!(http_endpoint.canonical.scheme(), BackendScheme::Http);

        assert_eq!(
            https_endpoint.transport_kind,
            RuntimeBackendTransportKind::H2
        );
        assert_eq!(https_endpoint.authority_host, "api.example.com");
        assert_eq!(https_endpoint.authority_port, 8443);
        assert_eq!(
            https_endpoint.address_kind,
            RuntimeBackendAddressKind::Hostname
        );
        assert_eq!(https_endpoint.canonical.scheme(), BackendScheme::Https);
    }

    #[test]
    fn runtime_backend_tls_policy_shapes_effective_tls_settings() {
        let effective_tls = UpstreamTls {
            verify_certificates: false,
            strict_sni: false,
            ca_file: Some("/etc/spooky/ca.pem".to_string()),
            ca_dir: Some("/etc/spooky/ca.d".to_string()),
        };

        let policy = RuntimeBackendTlsPolicy::from_effective_tls(&effective_tls);

        assert_eq!(
            policy,
            RuntimeBackendTlsPolicy {
                verify_certificates: false,
                strict_sni: false,
                ca_file: Some("/etc/spooky/ca.pem".to_string()),
                ca_dir: Some("/etc/spooky/ca.d".to_string()),
            }
        );
        let roundtrip = policy.as_upstream_tls();
        assert_eq!(
            roundtrip.verify_certificates,
            effective_tls.verify_certificates
        );
        assert_eq!(roundtrip.strict_sni, effective_tls.strict_sni);
        assert_eq!(roundtrip.ca_file, effective_tls.ca_file);
        assert_eq!(roundtrip.ca_dir, effective_tls.ca_dir);
    }

    #[test]
    fn runtime_backend_dns_policy_shapes_refresh_settings_from_performance() {
        let performance = Performance {
            backend_dns_refresh_enabled: true,
            backend_dns_refresh_interval_ms: 45_000,
            ..Performance::default()
        };

        let policy = RuntimeBackendDnsPolicy::from_performance(&performance);

        assert!(policy.refresh_enabled);
        assert_eq!(policy.refresh_interval, Duration::from_millis(45_000));
    }

    #[test]
    fn runtime_backend_health_check_normalizes_values_and_blank_path() {
        let health_check = HealthCheck {
            path: "   ".to_string(),
            interval: 5_000,
            timeout_ms: 900,
            failure_threshold: 4,
            success_threshold: 2,
            cooldown_ms: 1_500,
        };

        let normalized = RuntimeBackendHealthCheck::normalize("payments", "primary", &health_check)
            .expect("health check");

        assert_eq!(
            normalized,
            RuntimeBackendHealthCheck {
                path: "/".to_string(),
                interval: Duration::from_millis(5_000),
                timeout: Duration::from_millis(900),
                failure_threshold: 4,
                success_threshold: 2,
                cooldown: Duration::from_millis(1_500),
            }
        );
    }

    #[test]
    fn runtime_backend_health_check_rejects_zero_value_fields() {
        let cases = [
            (
                "health check interval is invalid (0) for backend 'primary' in upstream 'payments'",
                HealthCheck {
                    interval: 0,
                    ..valid_health_check()
                },
            ),
            (
                "health check timeout is invalid (0) for backend 'primary' in upstream 'payments'",
                HealthCheck {
                    timeout_ms: 0,
                    ..valid_health_check()
                },
            ),
            (
                "health check failure threshold is invalid (0) for backend 'primary' in upstream 'payments'",
                HealthCheck {
                    failure_threshold: 0,
                    ..valid_health_check()
                },
            ),
            (
                "health check success threshold is invalid (0) for backend 'primary' in upstream 'payments'",
                HealthCheck {
                    success_threshold: 0,
                    ..valid_health_check()
                },
            ),
        ];

        for (expected, health_check) in cases {
            let err = RuntimeBackendHealthCheck::normalize("payments", "primary", &health_check)
                .expect_err(expected);
            assert_config_invalid(err, expected);
        }
    }

    fn valid_health_check() -> HealthCheck {
        HealthCheck {
            path: "/health".to_string(),
            interval: 1_000,
            timeout_ms: 500,
            failure_threshold: 3,
            success_threshold: 2,
            cooldown_ms: 250,
        }
    }
}
