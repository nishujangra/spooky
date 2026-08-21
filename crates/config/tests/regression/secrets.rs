//! Secret-reference parsing and validation compatibility coverage.

use rcgen::{Certificate, CertificateParams, SanType};
use impulse_config::{
    config::{
        ExternalAuth, ExternalAuthFailureMode, JwtAuth, SecretProvider, SecretRef, UpstreamTls,
    },
    runtime::RuntimeExternalAuth,
    validator::validate,
};
use tempfile::tempdir;

use crate::common::{
    api_runtime_upstream, api_upstream_mut, assert_config_error_contains, parse_config,
    runtime_config, runtime_config_err, sample_config,
};

fn write_test_certs(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let mut params = CertificateParams::new(vec!["localhost".into()]);
    params
        .subject_alt_names
        .push(SanType::IpAddress("127.0.0.1".parse().expect("ip")));
    let cert = Certificate::from_params(params).expect("build cert");

    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    std::fs::write(&cert_path, cert.serialize_pem().expect("serialize cert")).expect("cert");
    std::fs::write(&key_path, cert.serialize_private_key_pem()).expect("key");

    (cert_path, key_path)
}

fn write_secret_file(dir: &std::path::Path, name: &str, value: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, value).expect("write secret");
    path
}

#[test]
fn config_parses_secret_reference_shapes_without_plaintext_values() {
    let config = parse_config(
        r#"
version: 1
listen:
  protocol: http3
  address: 0.0.0.0
  port: 443
  tls:
    cert: /tmp/tls/default.pem
    key: /tmp/tls/default.key
secrets:
  default_provider: local_files
  providers:
    local_files:
      kind: file
      base_dir: /etc/impulse/secrets
upstream:
  api:
    route:
      host: api.example.com
      path_prefix: /
    auth:
      jwt:
        secret_ref:
          ref: literal:jwt-signing-secret
    backends:
      - id: api-1
        address: https://api.internal:8443
"#,
    );

    assert_eq!(
        config.secrets.default_provider.as_deref(),
        Some("local_files")
    );
    let jwt = config
        .upstream
        .get("api")
        .and_then(|upstream| upstream.auth.jwt.as_ref())
        .expect("jwt config");
    assert!(jwt.secret.is_empty());
    assert_eq!(
        jwt.secret_ref
            .as_ref()
            .map(|secret_ref| secret_ref.reference.as_str()),
        Some("literal:jwt-signing-secret")
    );
}

#[test]
fn legacy_plaintext_secret_fields_remain_valid_in_phase_one() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_certs(dir.path());
    let mut config = sample_config();
    config.listen.tls.cert = cert.to_string_lossy().to_string();
    config.listen.tls.key = key.to_string_lossy().to_string();
    config
        .upstream
        .get_mut("api")
        .expect("api upstream")
        .auth
        .jwt = Some(JwtAuth {
        secret: "legacy-jwt-secret".to_string(),
        ..JwtAuth::default()
    });

    validate(&config).expect("legacy plaintext jwt secret should remain valid");
}

#[test]
fn invalid_secret_reference_combination_rejects_through_public_validator() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_certs(dir.path());
    let mut config = sample_config();
    config.listen.tls.cert = cert.to_string_lossy().to_string();
    config.listen.tls.key = key.to_string_lossy().to_string();
    config
        .upstream
        .get_mut("api")
        .expect("api upstream")
        .auth
        .jwt = Some(JwtAuth {
        secret: "legacy-jwt-secret".to_string(),
        secret_ref: Some(SecretRef {
            reference: "literal:new-jwt-secret".to_string(),
        }),
        ..JwtAuth::default()
    });

    let err = validate(&config).expect_err("dual jwt secret sources must reject");
    assert_eq!(
        err.to_string(),
        "upstream 'api' auth.jwt.secret and upstream 'api' auth.jwt.secret_ref cannot both be set"
    );
}

#[test]
fn runtime_config_resolves_jwt_secret_ref_eagerly_from_file() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_certs(dir.path());
    let secret_path = write_secret_file(dir.path(), "jwt.secret", b"file-backed-jwt-secret");

    let mut config = sample_config();
    config.listen.tls.cert = cert.to_string_lossy().to_string();
    config.listen.tls.key = key.to_string_lossy().to_string();
    api_upstream_mut(&mut config).auth.jwt = Some(JwtAuth {
        secret: String::new(),
        secret_ref: Some(SecretRef {
            reference: format!("file://{}", secret_path.display()),
        }),
        ..JwtAuth::default()
    });

    let runtime = runtime_config(&config);
    let jwt = api_runtime_upstream(&runtime)
        .policy
        .upstream_auth
        .jwt
        .as_ref()
        .expect("jwt policy");

    assert_eq!(jwt.secret, "file-backed-jwt-secret");
    assert!(runtime.observability.control_api.auth_token.is_none());
}

#[test]
fn runtime_config_resolves_oidc_client_secret_ref_eagerly_from_file() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_certs(dir.path());
    let secret_path = write_secret_file(dir.path(), "oidc.secret", b"oidc-client-secret");

    let mut config = sample_config();
    config.listen.tls.cert = cert.to_string_lossy().to_string();
    config.listen.tls.key = key.to_string_lossy().to_string();
    api_upstream_mut(&mut config).auth.external_auth = Some(ExternalAuth::Oidc {
        discovery_url: Some(
            "https://issuer.example.com/.well-known/openid-configuration".to_string(),
        ),
        issuer_url: Some("https://issuer.example.com".to_string()),
        client_id: "edge-gateway".to_string(),
        client_secret: None,
        client_secret_ref: Some(SecretRef {
            reference: format!("file://{}", secret_path.display()),
        }),
        audience: Some("impulse-api".to_string()),
        scopes: vec!["openid".to_string()],
        request_headers: Vec::new(),
        response_header_allowlist: Vec::new(),
        timeout_ms: 1_500,
        failure_mode: ExternalAuthFailureMode::FailClosed,
    });

    let runtime = runtime_config(&config);
    match api_runtime_upstream(&runtime)
        .policy
        .upstream_auth
        .external_auth
        .as_ref()
    {
        Some(RuntimeExternalAuth::Oidc { client_secret, .. }) => {
            assert_eq!(client_secret.as_deref(), Some("oidc-client-secret"));
        }
        other => panic!("unexpected external auth contract: {:?}", other),
    }
}

#[test]
fn runtime_config_materializes_control_api_token_refs_before_runtime_snapshot() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_certs(dir.path());
    let auth_token = write_secret_file(dir.path(), "admin.token", b"admin-secret-token");
    let viewer_token = write_secret_file(dir.path(), "viewer.token", b"viewer-secret-token");

    let mut config = sample_config();
    config.listen.tls.cert = cert.to_string_lossy().to_string();
    config.listen.tls.key = key.to_string_lossy().to_string();
    config.observability.control_api.enabled = true;
    config.observability.control_api.auth_token_ref = Some(SecretRef {
        reference: format!("file://{}", auth_token.display()),
    });
    config.observability.control_api.auth.bearer_tokens.push(
        impulse_config::config::ControlApiBearerToken {
            token: String::new(),
            token_ref: Some(SecretRef {
                reference: format!("file://{}", viewer_token.display()),
            }),
            role: impulse_config::config::ControlApiRole::Viewer,
            actor_id: Some("viewer".to_string()),
        },
    );

    let runtime = runtime_config(&config);
    assert_eq!(
        runtime.observability.control_api.auth_token.as_deref(),
        Some("admin-secret-token")
    );
    assert!(runtime.observability.control_api.auth_token_ref.is_none());
    assert_eq!(
        runtime.observability.control_api.auth.bearer_tokens[0].token,
        "viewer-secret-token"
    );
    assert!(
        runtime.observability.control_api.auth.bearer_tokens[0]
            .token_ref
            .is_none()
    );
}

#[test]
fn runtime_config_rejects_missing_secret_ref_during_normalization() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_certs(dir.path());

    let mut config = sample_config();
    config.listen.tls.cert = cert.to_string_lossy().to_string();
    config.listen.tls.key = key.to_string_lossy().to_string();
    api_upstream_mut(&mut config).auth.jwt = Some(JwtAuth {
        secret: String::new(),
        secret_ref: Some(SecretRef {
            reference: "file:///does/not/exist/jwt.secret".to_string(),
        }),
        ..JwtAuth::default()
    });

    let err = runtime_config_err(&config);
    assert_config_error_contains(
        &err,
        "secret_resolution_failed",
        "file secret resolution failed for upstream 'api' auth.jwt.secret_ref: file_not_found",
    );
}

#[test]
fn runtime_config_rejects_malformed_upstream_mtls_pem_during_normalization() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_certs(dir.path());
    let cert_secret = write_secret_file(dir.path(), "client.crt", b"not-a-pem-certificate");
    let key_secret = write_secret_file(dir.path(), "client.key", b"not-a-pem-key");

    let mut config = sample_config();
    config.listen.tls.cert = cert.to_string_lossy().to_string();
    config.listen.tls.key = key.to_string_lossy().to_string();
    api_upstream_mut(&mut config).tls = Some(UpstreamTls {
        verify_certificates: true,
        strict_sni: true,
        ca_file: None,
        ca_dir: None,
        client_certificate: None,
        client_certificate_ref: Some(SecretRef {
            reference: format!("file://{}", cert_secret.display()),
        }),
        client_key: None,
        client_key_ref: Some(SecretRef {
            reference: format!("file://{}", key_secret.display()),
        }),
    });

    let err = runtime_config_err(&config);
    assert_config_error_contains(
        &err,
        "tls_material_invalid",
        "file secret resolution failed for upstream 'api' tls.client_certificate: malformed_pem_certificate",
    );
}

#[test]
fn runtime_config_resolves_relative_file_refs_from_default_provider_base_dir() {
    let dir = tempdir().expect("tempdir");
    let (cert, key) = write_test_certs(dir.path());
    let secrets_dir = dir.path().join("secrets");
    std::fs::create_dir_all(&secrets_dir).expect("create secrets dir");
    write_secret_file(&secrets_dir, "jwt.secret", b"relative-base-dir-secret");

    let mut config = sample_config();
    config.listen.tls.cert = cert.to_string_lossy().to_string();
    config.listen.tls.key = key.to_string_lossy().to_string();
    config.secrets.default_provider = Some("local_files".to_string());
    config.secrets.providers.insert(
        "local_files".to_string(),
        SecretProvider::File {
            base_dir: Some(secrets_dir.to_string_lossy().to_string()),
        },
    );
    api_upstream_mut(&mut config).auth.jwt = Some(JwtAuth {
        secret: String::new(),
        secret_ref: Some(SecretRef {
            reference: "file://jwt.secret".to_string(),
        }),
        ..JwtAuth::default()
    });

    let runtime = runtime_config(&config);
    let jwt = api_runtime_upstream(&runtime)
        .policy
        .upstream_auth
        .jwt
        .as_ref()
        .expect("jwt policy");

    assert_eq!(jwt.secret, "relative-base-dir-secret");
}
