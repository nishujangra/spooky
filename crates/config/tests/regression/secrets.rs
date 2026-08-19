//! Secret-reference parsing and validation compatibility coverage.

use rcgen::{Certificate, CertificateParams, SanType};
use spooky_config::{
    config::{JwtAuth, SecretRef},
    validator::validate,
};
use tempfile::tempdir;

use crate::common::{parse_config, sample_config};

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
      base_dir: /etc/spooky/secrets
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
