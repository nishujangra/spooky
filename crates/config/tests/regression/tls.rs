//! Upstream TLS lowering: effective-TLS resolution and validation.

use spooky_config::{
    config::{ForwardedHeaderPolicyMode, UpstreamHostPolicyMode, UpstreamTls},
    runtime::RuntimeBackendTransportKind,
};

use crate::common::{
    api_backend_mut, api_runtime_upstream, api_upstream_mut, assert_config_error_contains,
    sample_runtime_config_err_with, sample_runtime_config_with,
};

#[test]
fn runtime_config_lowers_effective_tls_and_upstream_policy_wrappers() {
    let runtime = sample_runtime_config_with(|config| {
        config.upstream_tls = UpstreamTls {
            verify_certificates: true,
            strict_sni: true,
            ca_file: Some("/tmp/roots/global.pem".to_string()),
            ca_dir: None,
        };
        api_upstream_mut(config).tls = Some(UpstreamTls {
            verify_certificates: false,
            strict_sni: false,
            ca_file: Some("/tmp/roots/upstream.pem".to_string()),
            ca_dir: Some("/tmp/roots/upstream".to_string()),
        });
    });
    let upstream = api_runtime_upstream(&runtime);

    assert_eq!(upstream.name, "api");
    assert!(!upstream.effective_tls.verify_certificates);
    assert!(!upstream.effective_tls.strict_sni);
    assert_eq!(
        upstream.effective_tls.ca_file.as_deref(),
        Some("/tmp/roots/upstream.pem")
    );
    assert_eq!(upstream.backends.len(), 1);
    assert_eq!(
        upstream.backends[0].backend.address,
        "https://api.internal:8443"
    );
    assert_eq!(upstream.backends[0].endpoint.authority_host, "api.internal");
    assert_eq!(upstream.backends[0].endpoint.authority_port, 8443);
    assert_eq!(
        upstream.backends[0].endpoint.transport_kind,
        RuntimeBackendTransportKind::H2
    );
    assert_eq!(
        upstream.backend_tls_policy().ca_file.as_deref(),
        Some("/tmp/roots/upstream.pem")
    );
    assert_eq!(upstream.policy.host.0.mode, UpstreamHostPolicyMode::Rewrite);
    assert_eq!(
        upstream.policy.forwarded_headers.0.mode,
        ForwardedHeaderPolicyMode::Append
    );
}

#[test]
fn runtime_config_skips_global_tls_validation_for_http_only_upstreams() {
    let runtime = sample_runtime_config_with(|config| {
        api_backend_mut(config).address = "http://127.0.0.1:8080".to_string();
        config.upstream_tls.ca_file = Some("   ".to_string());
        config.upstream_tls.ca_dir = Some("   ".to_string());
    });
    let upstream = api_runtime_upstream(&runtime);

    assert_eq!(
        upstream.backends[0].backend.address,
        "http://127.0.0.1:8080"
    );
}

#[test]
fn runtime_config_skips_per_upstream_tls_validation_for_http_only_upstreams() {
    let runtime = sample_runtime_config_with(|config| {
        api_backend_mut(config).address = "http://127.0.0.1:8080".to_string();
        api_upstream_mut(config).tls = Some(UpstreamTls {
            verify_certificates: true,
            strict_sni: true,
            ca_file: Some("   ".to_string()),
            ca_dir: Some("   ".to_string()),
        });
    });
    let upstream = api_runtime_upstream(&runtime);

    assert_eq!(
        upstream.backends[0].backend.address,
        "http://127.0.0.1:8080"
    );
}

#[test]
fn runtime_config_requires_non_empty_effective_tls_fields_for_https_upstreams() {
    let err = sample_runtime_config_err_with(|config| {
        config.upstream_tls.ca_file = Some("   ".to_string());
    });
    assert_config_error_contains(
        &err,
        "tls_material_invalid",
        "upstream 'api' has an empty effective upstream_tls.ca_file",
    );
}
