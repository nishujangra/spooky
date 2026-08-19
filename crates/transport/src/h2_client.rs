use std::{
    collections::HashMap,
    convert::Infallible,
    ffi::OsStr,
    future::Future,
    io,
    net::SocketAddr,
    path::Path,
    pin::Pin,
    sync::{Arc, RwLock},
    task::{Context, Poll},
    time::Duration,
};

use http_body_util::combinators::BoxBody;
use hyper::{Request, body::Bytes, http::Uri, rt::Executor};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::{
    client::legacy::{
        Client,
        connect::{
            HttpConnector,
            dns::{GaiResolver, Name},
        },
    },
    rt::TokioIo,
};
use log::warn;
use rustls::{
    ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime},
};
use rustls_pki_types::pem::PemObject;
use spooky_config::{
    config::{SecretRef, UpstreamTls},
    runtime::{
        RuntimeBackendTlsPolicy, RuntimeResolvedSecret, RuntimeSecretResolutionErrorKind,
        RuntimeSecretResolver, RuntimeUpstream, resolve_file_secret_path,
    },
};
use tower_service::Service;

/// TLS client policy applied to HTTP/2 backend connections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsClientMaterialSource {
    LegacyFile(String),
    SecretRef(SecretRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsClientConfig {
    pub verify_certificates: bool,
    pub strict_sni: bool,
    pub ca_file: Option<String>,
    pub ca_dir: Option<String>,
    pub client_certificate: Option<TlsClientMaterialSource>,
    pub client_key: Option<TlsClientMaterialSource>,
    pub client_certificate_fingerprint_sha256: Option<String>,
    pub client_key_fingerprint_sha256: Option<String>,
}

impl Default for TlsClientConfig {
    fn default() -> Self {
        Self {
            verify_certificates: true,
            strict_sni: true,
            ca_file: None,
            ca_dir: None,
            client_certificate: None,
            client_key: None,
            client_certificate_fingerprint_sha256: None,
            client_key_fingerprint_sha256: None,
        }
    }
}

impl From<&RuntimeBackendTlsPolicy> for TlsClientConfig {
    fn from(value: &RuntimeBackendTlsPolicy) -> Self {
        Self {
            verify_certificates: value.verify_certificates,
            strict_sni: value.strict_sni,
            ca_file: value.ca_file.clone(),
            ca_dir: value.ca_dir.clone(),
            client_certificate: None,
            client_key: None,
            client_certificate_fingerprint_sha256: value
                .client_certificate
                .as_ref()
                .map(|metadata| metadata.fingerprint_sha256.clone()),
            client_key_fingerprint_sha256: value
                .client_key
                .as_ref()
                .map(|metadata| metadata.fingerprint_sha256.clone()),
        }
    }
}

impl TlsClientConfig {
    pub fn from_runtime_upstream(upstream: &RuntimeUpstream) -> Self {
        let effective_tls: &UpstreamTls = &upstream.effective_tls;
        let policy = upstream.backend_tls_policy();

        Self {
            verify_certificates: policy.verify_certificates,
            strict_sni: policy.strict_sni,
            ca_file: policy.ca_file.clone(),
            ca_dir: policy.ca_dir.clone(),
            client_certificate: effective_tls
                .client_certificate
                .clone()
                .map(TlsClientMaterialSource::LegacyFile)
                .or_else(|| {
                    effective_tls
                        .client_certificate_ref
                        .clone()
                        .map(TlsClientMaterialSource::SecretRef)
                }),
            client_key: effective_tls
                .client_key
                .clone()
                .map(TlsClientMaterialSource::LegacyFile)
                .or_else(|| {
                    effective_tls
                        .client_key_ref
                        .clone()
                        .map(TlsClientMaterialSource::SecretRef)
                }),
            client_certificate_fingerprint_sha256: policy
                .client_certificate
                .as_ref()
                .map(|metadata| metadata.fingerprint_sha256.clone()),
            client_key_fingerprint_sha256: policy
                .client_key
                .as_ref()
                .map(|metadata| metadata.fingerprint_sha256.clone()),
        }
    }
}

type ResolverResponse = std::vec::IntoIter<SocketAddr>;
type ResolverFuture =
    Pin<Box<dyn Future<Output = Result<ResolverResponse, io::Error>> + Send + 'static>>;

pub(crate) const DEFAULT_MAX_IDLE_PER_HOST: usize = 64;
pub(crate) const DEFAULT_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Transport-level observation emitted when a backend connect resolves to a
/// concrete socket address.
#[derive(Debug, Clone)]
pub struct ConnectObservation {
    pub backend: String,
    pub hostname: String,
    pub resolved_addr: SocketAddr,
}

/// Optional hook used by transport to observe outbound backend connects.
pub type ConnectObserver = Arc<dyn Fn(ConnectObservation) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DnsCacheUpdate {
    pub host: String,
    pub previous_addrs: Vec<SocketAddr>,
    pub current_addrs: Vec<SocketAddr>,
}

impl DnsCacheUpdate {
    #[cfg(test)]
    pub(crate) fn changed(&self) -> bool {
        self.previous_addrs != self.current_addrs
    }

    #[cfg(test)]
    pub(crate) fn cleared(&self) -> bool {
        self.current_addrs.is_empty()
    }
}

/// Shared DNS cache and resolver used by backend transports.
#[derive(Clone)]
pub struct SharedDnsResolver {
    cache: Arc<RwLock<HashMap<String, Vec<SocketAddr>>>>,
    fallback: GaiResolver,
}

#[derive(Clone)]
pub(crate) struct ObservedHttpConnector {
    inner: HttpConnector<SharedDnsResolver>,
    observer: Option<ConnectObserver>,
}

impl ObservedHttpConnector {
    fn new(inner: HttpConnector<SharedDnsResolver>, observer: Option<ConnectObserver>) -> Self {
        Self { inner, observer }
    }
}

pub(crate) fn build_observed_http_connector(
    dns_resolver: SharedDnsResolver,
    enforce_http: bool,
    connect_timeout: Duration,
    connect_observer: Option<ConnectObserver>,
) -> ObservedHttpConnector {
    let mut http = HttpConnector::new_with_resolver(dns_resolver);
    http.enforce_http(enforce_http);
    http.set_connect_timeout(Some(connect_timeout));
    ObservedHttpConnector::new(http, connect_observer)
}

impl Service<Uri> for ObservedHttpConnector {
    type Response = TokioIo<tokio::net::TcpStream>;
    type Error = <HttpConnector<SharedDnsResolver> as Service<Uri>>::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, dst: Uri) -> Self::Future {
        let mut inner = self.inner.clone();
        let observer = self.observer.clone();
        Box::pin(async move {
            let stream = inner.call(dst.clone()).await?;
            if let Some(observer) = observer
                && let Ok(resolved_addr) = stream.inner().peer_addr()
            {
                let backend = dst
                    .authority()
                    .map(|authority: &hyper::http::uri::Authority| authority.as_str().to_string())
                    .unwrap_or_else(|| dst.to_string());
                let hostname = dst
                    .host()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| backend.clone());
                observer(ConnectObservation {
                    backend,
                    hostname,
                    resolved_addr,
                });
            }
            Ok(stream)
        })
    }
}

impl SharedDnsResolver {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            fallback: GaiResolver::new(),
        }
    }

    pub fn set_host_addrs<I>(&self, host: &str, addrs: I)
    where
        I: IntoIterator<Item = SocketAddr>,
    {
        let _ = self.replace_host_addrs(host, addrs);
    }

    pub(crate) fn replace_host_addrs<I>(&self, host: &str, addrs: I) -> DnsCacheUpdate
    where
        I: IntoIterator<Item = SocketAddr>,
    {
        let normalized = normalize_dns_cache_host(host);
        let addrs = canonicalize_socket_addrs(addrs);
        let previous_addrs = if let Ok(mut guard) = self.cache.write() {
            if addrs.is_empty() {
                guard.remove(&normalized).unwrap_or_default()
            } else {
                guard
                    .insert(normalized.clone(), addrs.clone())
                    .unwrap_or_default()
            }
        } else {
            Vec::new()
        };

        DnsCacheUpdate {
            host: normalized,
            previous_addrs,
            current_addrs: addrs,
        }
    }

    #[cfg(test)]
    pub(crate) fn remove_host(&self, host: &str) -> DnsCacheUpdate {
        self.replace_host_addrs(host, Vec::<SocketAddr>::new())
    }

    pub fn cached_addrs(&self, host: &str) -> Option<Vec<SocketAddr>> {
        self.cache
            .read()
            .ok()
            .and_then(|guard| guard.get(&normalize_dns_cache_host(host)).cloned())
    }

    pub fn snapshot(&self) -> HashMap<String, Vec<SocketAddr>> {
        self.cache
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

impl Default for SharedDnsResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Service<Name> for SharedDnsResolver {
    type Response = ResolverResponse;
    type Error = io::Error;
    type Future = ResolverFuture;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.fallback.poll_ready(cx)
    }

    fn call(&mut self, name: Name) -> Self::Future {
        if let Some(addrs) = self.cached_addrs(name.as_str()) {
            return Box::pin(async move { Ok(addrs.into_iter()) });
        }

        let mut fallback = self.fallback.clone();
        Box::pin(async move {
            let resolved = fallback.call(name).await?;
            Ok(resolved.collect::<Vec<_>>().into_iter())
        })
    }
}

pub(crate) struct H2Client {
    client: Client<HttpsConnector<ObservedHttpConnector>, BoxBody<Bytes, Infallible>>,
}

impl Default for H2Client {
    fn default() -> Self {
        // infallible: default TLS config uses well-known roots and no custom certs
        #[allow(clippy::expect_used)]
        Self::new(
            DEFAULT_MAX_IDLE_PER_HOST,
            DEFAULT_POOL_IDLE_TIMEOUT,
            DEFAULT_CONNECT_TIMEOUT,
            TlsClientConfig::default(),
            SharedDnsResolver::new(),
        )
        .expect("default H2 client should build")
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TokioExecutor;

impl<F> Executor<F> for TokioExecutor
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn execute(&self, fut: F) {
        tokio::spawn(fut);
    }
}

impl H2Client {
    pub(crate) fn new(
        max_idle_per_host: usize,
        pool_idle_timeout: Duration,
        connect_timeout: Duration,
        tls: TlsClientConfig,
        dns_resolver: SharedDnsResolver,
    ) -> Result<Self, String> {
        Self::new_with_observer(
            max_idle_per_host,
            pool_idle_timeout,
            connect_timeout,
            tls,
            dns_resolver,
            None,
        )
    }

    pub(crate) fn new_with_observer(
        max_idle_per_host: usize,
        pool_idle_timeout: Duration,
        connect_timeout: Duration,
        tls: TlsClientConfig,
        dns_resolver: SharedDnsResolver,
        connect_observer: Option<ConnectObserver>,
    ) -> Result<Self, String> {
        let http =
            build_observed_http_connector(dns_resolver, false, connect_timeout, connect_observer);

        let tls_config = build_tls_config(&tls)?;
        let https = HttpsConnectorBuilder::new()
            .with_tls_config(tls_config)
            .https_or_http()
            .enable_http2()
            .wrap_connector(http);

        let client = Client::builder(TokioExecutor)
            .http2_only(true)
            .pool_max_idle_per_host(max_idle_per_host)
            .pool_idle_timeout(pool_idle_timeout)
            .build(https);

        Ok(Self { client })
    }

    pub(crate) async fn send(
        &self,
        req: Request<BoxBody<Bytes, Infallible>>,
    ) -> Result<hyper::Response<hyper::body::Incoming>, hyper_util::client::legacy::Error> {
        self.client.request(req).await
    }

    #[cfg(test)]
    fn try_default() -> Result<Self, String> {
        Self::new(
            DEFAULT_MAX_IDLE_PER_HOST,
            DEFAULT_POOL_IDLE_TIMEOUT,
            DEFAULT_CONNECT_TIMEOUT,
            TlsClientConfig::default(),
            SharedDnsResolver::new(),
        )
    }
}

fn normalize_dns_cache_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn canonicalize_socket_addrs<I>(addrs: I) -> Vec<SocketAddr>
where
    I: IntoIterator<Item = SocketAddr>,
{
    let mut addrs: Vec<_> = addrs.into_iter().collect();
    addrs.sort_unstable();
    addrs.dedup();
    addrs
}

fn build_tls_config(tls: &TlsClientConfig) -> Result<ClientConfig, String> {
    let client_identity = load_client_identity(tls)?;

    if !tls.verify_certificates {
        warn!(
            "upstream TLS certificate verification is disabled (upstream_tls.verify_certificates=false); this is insecure and should only be used in trusted environments"
        );
        let mut cfg = build_rustls_client_config(RootCertStore::empty(), client_identity)?;
        cfg.enable_sni = tls.strict_sni;
        cfg.dangerous()
            .set_certificate_verifier(Arc::new(InsecureServerCertVerifier));
        return Ok(cfg);
    }

    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    if let Some(ca_file) = tls.ca_file.as_ref() {
        let path = Path::new(ca_file);
        for cert in read_pem_certificates(path)? {
            roots.add(cert).map_err(|err| {
                format!(
                    "failed to add certificate from upstream_tls.ca_file '{}': {}",
                    path.display(),
                    err
                )
            })?;
        }
    }

    if let Some(ca_dir) = tls.ca_dir.as_ref() {
        let loaded = load_ca_directory(&mut roots, Path::new(ca_dir))?;
        if loaded == 0 {
            return Err(format!(
                "upstream_tls.ca_dir '{}' did not contain any readable PEM certificates",
                ca_dir
            ));
        }
    }

    let mut cfg = build_rustls_client_config(roots, client_identity)?;
    cfg.enable_sni = tls.strict_sni;
    Ok(cfg)
}

fn build_rustls_client_config(
    roots: RootCertStore,
    client_identity: Option<(Vec<rustls::pki_types::CertificateDer<'static>>, PrivateKeyDer<'static>)>,
) -> Result<ClientConfig, String> {
    let builder = ClientConfig::builder().with_root_certificates(roots);
    match client_identity {
        Some((chain, key)) => builder.with_client_auth_cert(chain, key).map_err(|err| {
            format!("client_identity_invalid: failed to build upstream TLS client identity: {err}")
        }),
        None => Ok(builder.with_no_client_auth()),
    }
}

fn load_client_identity(
    tls: &TlsClientConfig,
) -> Result<Option<(Vec<rustls::pki_types::CertificateDer<'static>>, PrivateKeyDer<'static>)>, String>
{
    match (&tls.client_certificate, &tls.client_key) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err("client_certificate_missing: upstream TLS client certificate source is not configured".to_string()),
        (Some(_), None) => Err("client_key_missing: upstream TLS client private key source is not configured".to_string()),
        (Some(cert_source), Some(key_source)) => {
            let cert = resolve_client_material(cert_source, "upstream_tls.client_certificate")
                .map_err(|err| classify_client_material_error(err, true))?;
            let key = resolve_client_material(key_source, "upstream_tls.client_key")
                .map_err(|err| classify_client_material_error(err, false))?;

            let certs = cert
                .parse_pem_certificates("upstream_tls.client_certificate")
                .map_err(|err| format!("client_identity_invalid: {err}"))?;
            let key = key
                .parse_pem_private_key("upstream_tls.client_key")
                .map_err(|err| format!("client_identity_invalid: {err}"))?;
            Ok(Some((certs, key)))
        }
    }
}

fn resolve_client_material(
    source: &TlsClientMaterialSource,
    field_name: &str,
) -> Result<RuntimeResolvedSecret, spooky_config::runtime::RuntimeSecretResolutionError> {
    match source {
        TlsClientMaterialSource::LegacyFile(path) => resolve_file_secret_path(path, field_name),
        TlsClientMaterialSource::SecretRef(secret_ref) => {
            RuntimeSecretResolver::default().resolve(secret_ref, field_name)
        }
    }
}

fn classify_client_material_error(
    err: spooky_config::runtime::RuntimeSecretResolutionError,
    certificate: bool,
) -> String {
    let prefix = match (certificate, err.kind()) {
        (true, RuntimeSecretResolutionErrorKind::FileNotFound | RuntimeSecretResolutionErrorKind::MissingSource) => {
            "client_certificate_missing"
        }
        (false, RuntimeSecretResolutionErrorKind::FileNotFound | RuntimeSecretResolutionErrorKind::MissingSource) => {
            "client_key_missing"
        }
        _ => "client_identity_invalid",
    };
    format!("{prefix}: {err}")
}

fn read_pem_certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let certs = CertificateDer::pem_file_iter(path)
        .map_err(|err| {
            format!(
                "failed to open certificate file '{}': {}",
                path.display(),
                err
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            format!(
                "failed to parse PEM certificates from '{}': {}",
                path.display(),
                err
            )
        })?;
    if certs.is_empty() {
        return Err(format!(
            "certificate file '{}' does not contain any PEM certificates",
            path.display()
        ));
    }
    Ok(certs)
}

fn load_ca_directory(roots: &mut RootCertStore, dir: &Path) -> Result<usize, String> {
    let entries = std::fs::read_dir(dir).map_err(|err| {
        format!(
            "failed to read upstream_tls.ca_dir '{}': {}",
            dir.display(),
            err
        )
    })?;

    let mut loaded = 0usize;
    for entry in entries {
        let entry = entry.map_err(|err| {
            format!(
                "failed to read entry in upstream_tls.ca_dir '{}': {}",
                dir.display(),
                err
            )
        })?;
        let path = entry.path();
        if !path.is_file() || !is_pem_like_path(&path) {
            continue;
        }

        for cert in read_pem_certificates(&path)? {
            roots.add(cert).map_err(|err| {
                format!(
                    "failed to add certificate from '{}': {}",
                    path.display(),
                    err
                )
            })?;
            loaded = loaded.saturating_add(1);
        }
    }

    Ok(loaded)
}

fn is_pem_like_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(OsStr::to_str),
        Some("pem" | "crt" | "cer" | "PEM" | "CRT" | "CER")
    )
}

#[derive(Debug)]
struct InsecureServerCertVerifier;

impl ServerCertVerifier for InsecureServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, str::FromStr, time::Duration};

    use hyper_util::client::legacy::connect::dns::Name;
    use tower_service::Service;

    use super::{DnsCacheUpdate, H2Client, SharedDnsResolver, TlsClientConfig};

    #[test]
    fn default_h2_client_does_not_panic() {
        let _client = H2Client::default();
    }

    #[test]
    fn default_tls_client_config_builds_h2_client() {
        assert!(H2Client::try_default().is_ok());
    }

    #[test]
    fn invalid_ca_file_is_rejected() {
        let unique = format!(
            "spooky-invalid-ca-{}-{}.pem",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::write(&path, b"not-a-pem-certificate").expect("write temp file");

        let client = H2Client::new(
            8,
            Duration::from_secs(5),
            Duration::from_secs(1),
            TlsClientConfig {
                verify_certificates: true,
                strict_sni: true,
                ca_file: Some(path.to_string_lossy().to_string()),
                ca_dir: None,
                client_certificate: None,
                client_key: None,
                client_certificate_fingerprint_sha256: None,
                client_key_fingerprint_sha256: None,
            },
            SharedDnsResolver::new(),
        );
        assert!(client.is_err());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn disabling_certificate_verification_is_allowed() {
        let client = H2Client::new(
            8,
            Duration::from_secs(5),
            Duration::from_secs(1),
            TlsClientConfig {
                verify_certificates: false,
                strict_sni: true,
                ca_file: None,
                ca_dir: None,
                client_certificate: None,
                client_key: None,
                client_certificate_fingerprint_sha256: None,
                client_key_fingerprint_sha256: None,
            },
            SharedDnsResolver::new(),
        );
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn shared_dns_resolver_returns_cached_addresses_case_insensitively() {
        let resolver = SharedDnsResolver::new();
        resolver.set_host_addrs(
            "api.example.com",
            [
                SocketAddr::from(([127, 0, 0, 10], 0)),
                SocketAddr::from(([127, 0, 0, 11], 0)),
            ],
        );

        let mut service = resolver.clone();
        let addrs: Vec<_> = service
            .call(Name::from_str("API.EXAMPLE.COM").expect("name"))
            .await
            .expect("resolve")
            .collect();

        assert_eq!(
            addrs,
            vec![
                SocketAddr::from(([127, 0, 0, 10], 0)),
                SocketAddr::from(([127, 0, 0, 11], 0))
            ]
        );
    }

    #[test]
    fn replace_host_addrs_reports_previous_and_current_values() {
        let resolver = SharedDnsResolver::new();
        let first = resolver.replace_host_addrs(
            "api.example.com",
            [SocketAddr::from(([127, 0, 0, 10], 443))],
        );
        assert_eq!(
            first,
            DnsCacheUpdate {
                host: "api.example.com".to_string(),
                previous_addrs: Vec::new(),
                current_addrs: vec![SocketAddr::from(([127, 0, 0, 10], 443))],
            }
        );
        assert!(first.changed());

        let second = resolver.replace_host_addrs(
            "API.EXAMPLE.COM.",
            [
                SocketAddr::from(([127, 0, 0, 11], 443)),
                SocketAddr::from(([127, 0, 0, 12], 443)),
            ],
        );
        assert_eq!(second.host, "api.example.com");
        assert_eq!(
            second.previous_addrs,
            vec![SocketAddr::from(([127, 0, 0, 10], 443))]
        );
        assert_eq!(
            second.current_addrs,
            vec![
                SocketAddr::from(([127, 0, 0, 11], 443)),
                SocketAddr::from(([127, 0, 0, 12], 443))
            ]
        );
        assert!(second.changed());
    }

    #[test]
    fn remove_host_clears_case_insensitive_cache_entry() {
        let resolver = SharedDnsResolver::new();
        resolver.set_host_addrs(
            "api.example.com",
            [SocketAddr::from(([127, 0, 0, 10], 443))],
        );

        let cleared = resolver.remove_host("API.EXAMPLE.COM");
        assert!(cleared.changed());
        assert!(cleared.cleared());
        assert_eq!(
            cleared.previous_addrs,
            vec![SocketAddr::from(([127, 0, 0, 10], 443))]
        );
        assert!(resolver.cached_addrs("api.example.com").is_none());
    }
}
