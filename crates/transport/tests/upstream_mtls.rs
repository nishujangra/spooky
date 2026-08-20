use std::{
    collections::HashMap,
    convert::Infallible,
    future::Future,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use hyper::{Request, Response, body::Incoming, service::service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyUsagePurpose, SanType,
};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use spooky_config::{config::SecretRef, runtime::RuntimeBackendTransportKind};
use spooky_errors::{ProxyError, UpstreamTlsReason, classify_upstream_proxy_error};
use spooky_transport::{
    SharedDnsResolver, TlsClientConfig, TlsClientMaterialSource, UpstreamTransportPool,
};
use tempfile::{TempDir, tempdir};
use tokio::net::TcpListener;
use tokio_rustls::{
    TlsAcceptor,
    rustls::{RootCertStore, ServerConfig, server::WebPkiClientVerifier},
};

struct PemIdentity {
    cert_path: PathBuf,
    key_path: PathBuf,
}

struct MtlsFixture {
    _dir: TempDir,
    ca_cert_path: PathBuf,
    wrong_ca_cert_path: PathBuf,
    server: PemIdentity,
    client_a: PemIdentity,
    client_b: PemIdentity,
    wrong_client: PemIdentity,
}

impl MtlsFixture {
    fn new() -> Self {
        let dir = tempdir().expect("tempdir");

        let ca = build_ca("Spooky Test CA");
        let wrong_ca = build_ca("Wrong Spooky Test CA");

        let server = write_identity(
            dir.path(),
            "server",
            signed_cert(
                "localhost",
                &ca,
                vec!["localhost".to_string()],
                vec![
                    SanType::DnsName("localhost".to_string()),
                    SanType::IpAddress(IpAddr::from([127, 0, 0, 1])),
                ],
                ExtendedKeyUsagePurpose::ServerAuth,
            ),
        );
        let client_a = write_identity(
            dir.path(),
            "client-a",
            signed_cert(
                "client-a",
                &ca,
                Vec::new(),
                Vec::new(),
                ExtendedKeyUsagePurpose::ClientAuth,
            ),
        );
        let client_b = write_identity(
            dir.path(),
            "client-b",
            signed_cert(
                "client-b",
                &ca,
                Vec::new(),
                Vec::new(),
                ExtendedKeyUsagePurpose::ClientAuth,
            ),
        );
        let wrong_client = write_identity(
            dir.path(),
            "wrong-client",
            signed_cert(
                "wrong-client",
                &wrong_ca,
                Vec::new(),
                Vec::new(),
                ExtendedKeyUsagePurpose::ClientAuth,
            ),
        );

        let ca_cert_path = dir.path().join("ca.pem");
        std::fs::write(&ca_cert_path, ca.pem()).expect("write ca");
        let wrong_ca_cert_path = dir.path().join("wrong-ca.pem");
        std::fs::write(&wrong_ca_cert_path, wrong_ca.pem()).expect("write wrong ca");

        Self {
            _dir: dir,
            ca_cert_path,
            wrong_ca_cert_path,
            server,
            client_a,
            client_b,
            wrong_client,
        }
    }
}

struct TestCa {
    cert: Certificate,
}

impl TestCa {
    fn pem(&self) -> String {
        self.cert.serialize_pem().expect("serialize ca")
    }
}

fn build_ca(common_name: &str) -> TestCa {
    let mut params = CertificateParams::new(Vec::new());
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::CrlSign,
    ];
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CommonName, common_name);
    params.distinguished_name = distinguished_name;
    TestCa {
        cert: Certificate::from_params(params).expect("build ca"),
    }
}

fn signed_cert(
    common_name: &str,
    ca: &TestCa,
    dns_names: Vec<String>,
    subject_alt_names: Vec<SanType>,
    usage: ExtendedKeyUsagePurpose,
) -> (String, String) {
    let mut params = CertificateParams::new(dns_names);
    params.extended_key_usages = vec![usage];
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.subject_alt_names = subject_alt_names;
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CommonName, common_name);
    params.distinguished_name = distinguished_name;
    let cert = Certificate::from_params(params).expect("build cert");
    (
        cert.serialize_pem_with_signer(&ca.cert)
            .expect("serialize signed cert"),
        cert.serialize_private_key_pem(),
    )
}

fn write_identity(dir: &Path, name: &str, pem: (String, String)) -> PemIdentity {
    let cert_path = dir.join(format!("{name}.cert.pem"));
    let key_path = dir.join(format!("{name}.key.pem"));
    std::fs::write(&cert_path, pem.0).expect("write cert");
    std::fs::write(&key_path, pem.1).expect("write key");
    PemIdentity {
        cert_path,
        key_path,
    }
}

fn connection_policy() -> spooky_config::runtime::RuntimeBackendConnectionPolicy {
    spooky_config::runtime::RuntimeBackendConnectionPolicy {
        max_inflight: 8,
        max_idle_per_backend: 8,
        pool_idle_timeout: Duration::from_secs(5),
        connect_timeout: Duration::from_secs(2),
        execution_timeout: Duration::from_secs(5),
    }
}

fn backend_tls(ca_cert_path: &Path, client_identity: Option<&PemIdentity>) -> TlsClientConfig {
    TlsClientConfig {
        verify_certificates: true,
        strict_sni: true,
        ca_file: Some(ca_cert_path.to_string_lossy().to_string()),
        ca_file_fingerprint_sha256: None,
        ca_dir: None,
        ca_dir_fingerprint_sha256: None,
        client_certificate: client_identity.map(|identity| {
            TlsClientMaterialSource::SecretRef(SecretRef {
                reference: format!("file://{}", identity.cert_path.display()),
            })
        }),
        client_key: client_identity.map(|identity| {
            TlsClientMaterialSource::SecretRef(SecretRef {
                reference: format!("file://{}", identity.key_path.display()),
            })
        }),
        client_certificate_fingerprint_sha256: None,
        client_key_fingerprint_sha256: None,
    }
}

fn request(uri: &str) -> Request<BoxBody<Bytes, Infallible>> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Empty::<Bytes>::new().boxed())
        .expect("request")
}

async fn read_body(response: Response<Incoming>) -> Bytes {
    response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes()
}

async fn start_h2_mtls_backend<F, Fut>(
    server_identity: &PemIdentity,
    client_ca_cert_path: &Path,
    handler: F,
) -> std::io::Result<u16>
where
    F: Fn(Request<Incoming>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Response<Full<Bytes>>, Infallible>> + Send + 'static,
{
    let mut roots = RootCertStore::empty();
    for cert in read_chain(client_ca_cert_path) {
        roots.add(cert).expect("add client auth root");
    }
    let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .expect("client verifier");

    let mut tls_config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            read_chain(&server_identity.cert_path),
            read_key(&server_identity.key_path),
        )
        .expect("server tls config");
    tls_config.alpn_protocols = vec![b"h2".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let handler = Arc::new(handler);

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(value) => value,
                Err(_) => break,
            };
            let acceptor = acceptor.clone();
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                let tls_stream = match acceptor.accept(stream).await {
                    Ok(stream) => stream,
                    Err(_) => return,
                };
                let service = service_fn(move |req: Request<Incoming>| {
                    let handler = Arc::clone(&handler);
                    async move { handler(req).await }
                });

                let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(tls_stream), service)
                    .await;
            });
        }
    });

    Ok(port)
}

fn read_chain(path: &Path) -> Vec<CertificateDer<'static>> {
    CertificateDer::pem_file_iter(path)
        .expect("open chain")
        .collect::<Result<Vec<_>, _>>()
        .expect("parse chain")
}

fn read_key(path: &Path) -> PrivateKeyDer<'static> {
    PrivateKeyDer::from_pem_file(path).expect("parse key")
}

fn classify_tls_reason(err: &ProxyError) -> UpstreamTlsReason {
    classify_upstream_proxy_error(err)
        .expect("classified proxy error")
        .classification
        .tls_reason
        .expect("tls reason")
}

fn loopback_bind_restricted(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::PermissionDenied
        || matches!(err.raw_os_error(), Some(1) | Some(13))
}

#[tokio::test]
async fn h2_request_fails_without_client_certificate_when_backend_requires_mtls() {
    let fixture = MtlsFixture::new();
    let port =
        match start_h2_mtls_backend(&fixture.server, &fixture.ca_cert_path, |_req| async move {
            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(b"ok"))))
        })
        .await
        {
            Ok(port) => port,
            Err(err) if loopback_bind_restricted(&err) => return,
            Err(err) => panic!("backend: {err}"),
        };

    let backend = format!("https://localhost:{port}");
    let resolver = SharedDnsResolver::new();
    resolver.set_host_addrs(
        "localhost",
        [std::net::SocketAddr::from(([127, 0, 0, 1], 0))],
    );

    let pool = UpstreamTransportPool::new_from_runtime_backends(
        [(backend.clone(), RuntimeBackendTransportKind::H2)],
        HashMap::from([(backend.clone(), backend_tls(&fixture.ca_cert_path, None))]),
        connection_policy(),
        resolver,
    )
    .expect("transport pool");

    let err = pool
        .send_backend_request(&backend, request(&format!("{backend}/")))
        .await
        .expect_err("request should fail without client cert");

    assert_eq!(
        classify_tls_reason(&err),
        UpstreamTlsReason::ClientAuthRejected
    );
}

#[tokio::test]
async fn h2_request_succeeds_with_valid_client_certificate() {
    let fixture = MtlsFixture::new();
    let port =
        match start_h2_mtls_backend(&fixture.server, &fixture.ca_cert_path, |_req| async move {
            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(b"mtls-ok"))))
        })
        .await
        {
            Ok(port) => port,
            Err(err) if loopback_bind_restricted(&err) => return,
            Err(err) => panic!("backend: {err}"),
        };

    let backend = format!("https://localhost:{port}");
    let resolver = SharedDnsResolver::new();
    resolver.set_host_addrs(
        "localhost",
        [std::net::SocketAddr::from(([127, 0, 0, 1], 0))],
    );

    let pool = UpstreamTransportPool::new_from_runtime_backends(
        [(backend.clone(), RuntimeBackendTransportKind::H2)],
        HashMap::from([(
            backend.clone(),
            backend_tls(&fixture.ca_cert_path, Some(&fixture.client_a)),
        )]),
        connection_policy(),
        resolver,
    )
    .expect("transport pool");

    let response = pool
        .send_backend_request(&backend, request(&format!("{backend}/")))
        .await
        .expect("request should succeed");

    assert_eq!(read_body(response).await, Bytes::from_static(b"mtls-ok"));
}

#[tokio::test]
async fn h2_request_fails_with_wrong_ca_or_wrong_client_identity() {
    let fixture = MtlsFixture::new();
    let port =
        match start_h2_mtls_backend(&fixture.server, &fixture.ca_cert_path, |_req| async move {
            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(b"unexpected"))))
        })
        .await
        {
            Ok(port) => port,
            Err(err) if loopback_bind_restricted(&err) => return,
            Err(err) => panic!("backend: {err}"),
        };

    let backend = format!("https://localhost:{port}");
    let resolver = SharedDnsResolver::new();
    resolver.set_host_addrs(
        "localhost",
        [std::net::SocketAddr::from(([127, 0, 0, 1], 0))],
    );

    let wrong_ca_pool = UpstreamTransportPool::new_from_runtime_backends(
        [(backend.clone(), RuntimeBackendTransportKind::H2)],
        HashMap::from([(
            backend.clone(),
            backend_tls(&fixture.wrong_ca_cert_path, Some(&fixture.client_a)),
        )]),
        connection_policy(),
        resolver.clone(),
    )
    .expect("transport pool");

    let wrong_ca_err = wrong_ca_pool
        .send_backend_request(&backend, request(&format!("{backend}/")))
        .await
        .expect_err("wrong ca should fail");
    assert_eq!(
        classify_tls_reason(&wrong_ca_err),
        UpstreamTlsReason::UnknownIssuer
    );

    let wrong_identity_pool = UpstreamTransportPool::new_from_runtime_backends(
        [(backend.clone(), RuntimeBackendTransportKind::H2)],
        HashMap::from([(
            backend.clone(),
            backend_tls(&fixture.ca_cert_path, Some(&fixture.wrong_client)),
        )]),
        connection_policy(),
        resolver,
    )
    .expect("transport pool");

    let wrong_identity_err = wrong_identity_pool
        .send_backend_request(&backend, request(&format!("{backend}/")))
        .await
        .expect_err("wrong client identity should fail");
    assert_eq!(
        classify_tls_reason(&wrong_identity_err),
        UpstreamTlsReason::ClientAuthRejected
    );
}

#[tokio::test]
async fn h2_pool_rotates_after_client_certificate_replacement() {
    let fixture = MtlsFixture::new();
    let port =
        match start_h2_mtls_backend(&fixture.server, &fixture.ca_cert_path, |_req| async move {
            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(b"rotated"))))
        })
        .await
        {
            Ok(port) => port,
            Err(err) if loopback_bind_restricted(&err) => return,
            Err(err) => panic!("backend: {err}"),
        };

    let backend = format!("https://localhost:{port}");
    let resolver = SharedDnsResolver::new();
    resolver.set_host_addrs(
        "localhost",
        [std::net::SocketAddr::from(([127, 0, 0, 1], 0))],
    );

    let initial_tls = backend_tls(&fixture.ca_cert_path, Some(&fixture.client_a));
    let pool = UpstreamTransportPool::new_from_runtime_backends(
        [(backend.clone(), RuntimeBackendTransportKind::H2)],
        HashMap::from([(backend.clone(), initial_tls.clone())]),
        connection_policy(),
        resolver,
    )
    .expect("transport pool");

    let first_response = pool
        .send_backend_request(&backend, request(&format!("{backend}/")))
        .await
        .expect("initial request");
    assert_eq!(
        read_body(first_response).await,
        Bytes::from_static(b"rotated")
    );

    let unchanged = pool
        .rotate_backend_client_with_tls(&backend, initial_tls)
        .expect("unchanged rotation");
    assert!(!unchanged.rotated());
    assert_eq!(unchanged.generations(), None);

    let rotated = pool
        .rotate_backend_client_with_tls(
            &backend,
            backend_tls(&fixture.ca_cert_path, Some(&fixture.client_b)),
        )
        .expect("rotated with new client cert");
    assert!(rotated.rotated());
    assert_eq!(rotated.generations(), Some((0, 1)));

    let second_response = pool
        .send_backend_request(&backend, request(&format!("{backend}/")))
        .await
        .expect("request after rotation");
    assert_eq!(
        read_body(second_response).await,
        Bytes::from_static(b"rotated")
    );
}
