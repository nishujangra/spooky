use std::{
    fmt, fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use sha2::{Digest, Sha256};

use super::RuntimeConfigError;
use crate::bounded_file::{BoundedFileReadError, read_file_with_limit};
use crate::config::{
    Config, ControlApiBearerToken, ExternalAuth, SecretProvider, SecretRef, Secrets,
};

const MAX_FILE_BACKED_SECRET_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSecretSourceKind {
    Literal,
    File,
}

impl RuntimeSecretSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::File => "file",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSecretResolutionErrorKind {
    ConflictingSources,
    MissingSource,
    UnsupportedScheme,
    MalformedReference,
    FileNotFound,
    PermissionDenied,
    NotAFile,
    InvalidBaseDirectory,
    PathOutsideBaseDir,
    EmptySecret,
    SecretTooLarge,
    Io,
    InvalidUtf8,
    MalformedPemCertificate,
    MalformedPemPrivateKey,
}

impl RuntimeSecretResolutionErrorKind {
    pub fn slug(self) -> &'static str {
        match self {
            Self::ConflictingSources => "conflicting_sources",
            Self::MissingSource => "missing_source",
            Self::UnsupportedScheme => "unsupported_scheme",
            Self::MalformedReference => "malformed_reference",
            Self::FileNotFound => "file_not_found",
            Self::PermissionDenied => "permission_denied",
            Self::NotAFile => "not_a_file",
            Self::InvalidBaseDirectory => "invalid_base_directory",
            Self::PathOutsideBaseDir => "path_outside_base_dir",
            Self::EmptySecret => "empty_secret",
            Self::SecretTooLarge => "secret_too_large",
            Self::Io => "io_error",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::MalformedPemCertificate => "malformed_pem_certificate",
            Self::MalformedPemPrivateKey => "malformed_pem_private_key",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeResolvedSecretMetadata {
    pub source_kind: RuntimeSecretSourceKind,
    pub fingerprint_sha256: String,
    pub byte_len: usize,
    pub loaded_at_unix_ms: u64,
}

impl fmt::Debug for RuntimeResolvedSecretMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeResolvedSecretMetadata")
            .field("source_kind", &self.source_kind)
            .field("fingerprint_sha256", &self.fingerprint_sha256)
            .field("byte_len", &self.byte_len)
            .field("loaded_at_unix_ms", &self.loaded_at_unix_ms)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeResolvedSecret {
    bytes: Vec<u8>,
    metadata: RuntimeResolvedSecretMetadata,
}

impl RuntimeResolvedSecret {
    pub fn metadata(&self) -> &RuntimeResolvedSecretMetadata {
        &self.metadata
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn into_string(self, field_name: &str) -> Result<String, RuntimeSecretResolutionError> {
        String::from_utf8(self.bytes).map_err(|_| {
            RuntimeSecretResolutionError::new(
                field_name,
                Some(self.metadata.source_kind),
                RuntimeSecretResolutionErrorKind::InvalidUtf8,
            )
        })
    }

    pub fn parse_pem_certificates(
        &self,
        field_name: &str,
    ) -> Result<Vec<CertificateDer<'static>>, RuntimeSecretResolutionError> {
        let certs = CertificateDer::pem_slice_iter(&self.bytes)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| {
                RuntimeSecretResolutionError::new(
                    field_name,
                    Some(self.metadata.source_kind),
                    RuntimeSecretResolutionErrorKind::MalformedPemCertificate,
                )
            })?;
        if certs.is_empty() {
            return Err(RuntimeSecretResolutionError::new(
                field_name,
                Some(self.metadata.source_kind),
                RuntimeSecretResolutionErrorKind::MalformedPemCertificate,
            ));
        }
        Ok(certs)
    }

    pub fn parse_pem_private_key(
        &self,
        field_name: &str,
    ) -> Result<PrivateKeyDer<'static>, RuntimeSecretResolutionError> {
        PrivateKeyDer::from_pem_slice(&self.bytes).map_err(|_| {
            RuntimeSecretResolutionError::new(
                field_name,
                Some(self.metadata.source_kind),
                RuntimeSecretResolutionErrorKind::MalformedPemPrivateKey,
            )
        })
    }
}

impl fmt::Debug for RuntimeResolvedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeResolvedSecret")
            .field("metadata", &self.metadata)
            .field("value", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSecretResolutionError {
    field_name: String,
    source_kind: Option<RuntimeSecretSourceKind>,
    kind: RuntimeSecretResolutionErrorKind,
}

impl RuntimeSecretResolutionError {
    pub fn new(
        field_name: impl Into<String>,
        source_kind: Option<RuntimeSecretSourceKind>,
        kind: RuntimeSecretResolutionErrorKind,
    ) -> Self {
        Self {
            field_name: field_name.into(),
            source_kind,
            kind,
        }
    }

    pub fn field_name(&self) -> &str {
        &self.field_name
    }

    pub fn source_kind(&self) -> Option<RuntimeSecretSourceKind> {
        self.source_kind
    }

    pub fn kind(&self) -> RuntimeSecretResolutionErrorKind {
        self.kind
    }
}

impl fmt::Display for RuntimeSecretResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(source_kind) = self.source_kind {
            write!(
                f,
                "{} secret resolution failed for {}: {}",
                source_kind.as_str(),
                self.field_name,
                self.kind.slug()
            )
        } else {
            write!(
                f,
                "secret resolution failed for {}: {}",
                self.field_name,
                self.kind.slug()
            )
        }
    }
}

impl std::error::Error for RuntimeSecretResolutionError {}

pub trait RuntimeSecretProvider: fmt::Debug + Send + Sync {
    fn source_kind(&self) -> RuntimeSecretSourceKind;
    fn supports_scheme(&self, scheme: &str) -> bool;
    fn resolve(
        &self,
        secret_ref: &SecretRef,
        field_name: &str,
    ) -> Result<RuntimeResolvedSecret, RuntimeSecretResolutionError>;
}

#[derive(Debug, Default)]
pub struct LiteralSecretProvider;

impl RuntimeSecretProvider for LiteralSecretProvider {
    fn source_kind(&self) -> RuntimeSecretSourceKind {
        RuntimeSecretSourceKind::Literal
    }

    fn supports_scheme(&self, scheme: &str) -> bool {
        scheme == "literal"
    }

    fn resolve(
        &self,
        secret_ref: &SecretRef,
        field_name: &str,
    ) -> Result<RuntimeResolvedSecret, RuntimeSecretResolutionError> {
        let Some(value) = secret_ref.raw_value().strip_prefix("literal:") else {
            return Err(RuntimeSecretResolutionError::new(
                field_name,
                Some(self.source_kind()),
                RuntimeSecretResolutionErrorKind::MalformedReference,
            ));
        };
        if value.is_empty() {
            return Err(RuntimeSecretResolutionError::new(
                field_name,
                Some(self.source_kind()),
                RuntimeSecretResolutionErrorKind::EmptySecret,
            ));
        }
        Ok(resolved_secret(
            self.source_kind(),
            value.as_bytes().to_vec(),
        ))
    }
}

#[derive(Debug, Clone, Default)]
pub struct FilesystemSecretProvider {
    base_dir: Option<PathBuf>,
}

impl FilesystemSecretProvider {
    pub fn new(base_dir: Option<PathBuf>) -> Self {
        Self { base_dir }
    }

    fn canonical_base_dir(
        &self,
        field_name: &str,
    ) -> Result<Option<PathBuf>, RuntimeSecretResolutionError> {
        let Some(base_dir) = self.base_dir.as_ref() else {
            return Ok(None);
        };

        let canonical_base_dir =
            fs::canonicalize(base_dir).map_err(|err| map_io_error(field_name, err.kind()))?;
        let metadata = fs::metadata(&canonical_base_dir)
            .map_err(|err| map_io_error(field_name, err.kind()))?;
        if !metadata.is_dir() {
            return Err(RuntimeSecretResolutionError::new(
                field_name,
                Some(RuntimeSecretSourceKind::File),
                RuntimeSecretResolutionErrorKind::InvalidBaseDirectory,
            ));
        }

        Ok(Some(canonical_base_dir))
    }

    fn resolve_path(
        &self,
        raw_path: &str,
        field_name: &str,
    ) -> Result<PathBuf, RuntimeSecretResolutionError> {
        let candidate = Path::new(raw_path);
        let Some(canonical_base_dir) = self.canonical_base_dir(field_name)? else {
            return Ok(candidate.to_path_buf());
        };

        if candidate.is_absolute()
            || candidate
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
        {
            return Err(RuntimeSecretResolutionError::new(
                field_name,
                Some(RuntimeSecretSourceKind::File),
                RuntimeSecretResolutionErrorKind::PathOutsideBaseDir,
            ));
        }

        let canonical_path = fs::canonicalize(canonical_base_dir.join(candidate))
            .map_err(|err| map_io_error(field_name, err.kind()))?;
        if !canonical_path.starts_with(&canonical_base_dir) {
            return Err(RuntimeSecretResolutionError::new(
                field_name,
                Some(RuntimeSecretSourceKind::File),
                RuntimeSecretResolutionErrorKind::PathOutsideBaseDir,
            ));
        }

        Ok(canonical_path)
    }
}

impl RuntimeSecretProvider for FilesystemSecretProvider {
    fn source_kind(&self) -> RuntimeSecretSourceKind {
        RuntimeSecretSourceKind::File
    }

    fn supports_scheme(&self, scheme: &str) -> bool {
        scheme == "file"
    }

    fn resolve(
        &self,
        secret_ref: &SecretRef,
        field_name: &str,
    ) -> Result<RuntimeResolvedSecret, RuntimeSecretResolutionError> {
        let Some(path) = secret_ref.raw_value().strip_prefix("file://") else {
            return Err(RuntimeSecretResolutionError::new(
                field_name,
                Some(self.source_kind()),
                RuntimeSecretResolutionErrorKind::MalformedReference,
            ));
        };
        if path.is_empty() {
            return Err(RuntimeSecretResolutionError::new(
                field_name,
                Some(self.source_kind()),
                RuntimeSecretResolutionErrorKind::MalformedReference,
            ));
        }
        let resolved_path = self.resolve_path(path, field_name)?;
        let Some(resolved_path) = resolved_path.to_str() else {
            return Err(RuntimeSecretResolutionError::new(
                field_name,
                Some(self.source_kind()),
                RuntimeSecretResolutionErrorKind::MalformedReference,
            ));
        };
        resolve_file_bytes(resolved_path, field_name)
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeSecretResolver {
    providers: Vec<Arc<dyn RuntimeSecretProvider>>,
}

impl Default for RuntimeSecretResolver {
    fn default() -> Self {
        Self {
            providers: vec![
                Arc::new(LiteralSecretProvider),
                Arc::new(FilesystemSecretProvider::default()),
            ],
        }
    }
}

impl RuntimeSecretResolver {
    pub fn from_secrets_config(secrets: &Secrets) -> Self {
        let file_provider = if let Some(default_provider) = secrets.default_provider.as_deref() {
            filesystem_provider_from_config(secrets.providers.get(default_provider))
        } else if secrets.providers.len() == 1 {
            filesystem_provider_from_config(secrets.providers.values().next())
        } else {
            None
        };

        Self {
            providers: vec![
                Arc::new(LiteralSecretProvider),
                Arc::new(file_provider.unwrap_or_default()),
            ],
        }
    }

    pub fn resolve(
        &self,
        secret_ref: &SecretRef,
        field_name: &str,
    ) -> Result<RuntimeResolvedSecret, RuntimeSecretResolutionError> {
        let Some(scheme) = secret_ref.scheme() else {
            return Err(RuntimeSecretResolutionError::new(
                field_name,
                None,
                RuntimeSecretResolutionErrorKind::MalformedReference,
            ));
        };
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.supports_scheme(scheme))
            .ok_or_else(|| {
                RuntimeSecretResolutionError::new(
                    field_name,
                    None,
                    RuntimeSecretResolutionErrorKind::UnsupportedScheme,
                )
            })?;
        provider.resolve(secret_ref, field_name)
    }

    pub fn resolve_from_sources(
        &self,
        literal_value: Option<&str>,
        secret_ref: Option<&SecretRef>,
        field_name: &str,
    ) -> Result<RuntimeResolvedSecret, RuntimeSecretResolutionError> {
        match (literal_value.filter(|value| !value.is_empty()), secret_ref) {
            (Some(_), Some(_)) => Err(RuntimeSecretResolutionError::new(
                field_name,
                None,
                RuntimeSecretResolutionErrorKind::ConflictingSources,
            )),
            (Some(value), None) => Ok(resolved_secret(
                RuntimeSecretSourceKind::Literal,
                value.as_bytes().to_vec(),
            )),
            (None, Some(secret_ref)) => self.resolve(secret_ref, field_name),
            (None, None) => Err(RuntimeSecretResolutionError::new(
                field_name,
                None,
                RuntimeSecretResolutionErrorKind::MissingSource,
            )),
        }
    }
}

pub fn resolve_config_secrets(config: &Config) -> Result<Config, RuntimeConfigError> {
    let resolver = RuntimeSecretResolver::from_secrets_config(&config.secrets);
    let mut resolved = config.clone();

    for (upstream_name, upstream) in &mut resolved.upstream {
        if let Some(jwt) = upstream.auth.jwt.as_mut()
            && jwt.secret_ref.is_some()
        {
            let secret = resolver
                .resolve_from_sources(
                    (!jwt.secret.trim().is_empty()).then_some(jwt.secret.trim()),
                    jwt.secret_ref.as_ref(),
                    &format!("upstream '{upstream_name}' auth.jwt.secret_ref"),
                )
                .map_err(runtime_secret_config_error)?;
            jwt.secret = secret
                .into_string(&format!("upstream '{upstream_name}' auth.jwt.secret_ref"))
                .map_err(runtime_secret_config_error)?;
            jwt.secret_ref = None;
        }

        if let Some(ExternalAuth::Oidc {
            client_secret,
            client_secret_ref,
            ..
        }) = upstream.auth.external_auth.as_mut()
            && client_secret_ref.is_some()
        {
            let resolved_secret = resolver
                .resolve_from_sources(
                    client_secret
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty()),
                    client_secret_ref.as_ref(),
                    &format!(
                        "upstream '{upstream_name}' auth.external_auth.oidc.client_secret_ref"
                    ),
                )
                .map_err(runtime_secret_config_error)?;
            *client_secret = Some(
                resolved_secret
                    .into_string(&format!(
                        "upstream '{upstream_name}' auth.external_auth.oidc.client_secret_ref"
                    ))
                    .map_err(runtime_secret_config_error)?,
            );
            *client_secret_ref = None;
        }
    }

    if resolved.observability.control_api.auth_token_ref.is_some() {
        let token = resolver
            .resolve_from_sources(
                resolved
                    .observability
                    .control_api
                    .auth_token
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty()),
                resolved.observability.control_api.auth_token_ref.as_ref(),
                "observability.control_api.auth_token_ref",
            )
            .map_err(runtime_secret_config_error)?;
        resolved.observability.control_api.auth_token = Some(
            token
                .into_string("observability.control_api.auth_token_ref")
                .map_err(runtime_secret_config_error)?,
        );
        resolved.observability.control_api.auth_token_ref = None;
    }

    for (index, token) in resolved
        .observability
        .control_api
        .auth
        .bearer_tokens
        .iter_mut()
        .enumerate()
    {
        resolve_control_api_bearer_token(&resolver, token, index)
            .map_err(runtime_secret_config_error)?;
    }

    Ok(resolved)
}

fn filesystem_provider_from_config(
    provider: Option<&SecretProvider>,
) -> Option<FilesystemSecretProvider> {
    provider.map(|SecretProvider::File { base_dir }| {
        FilesystemSecretProvider::new(base_dir.as_deref().map(PathBuf::from))
    })
}

fn resolve_control_api_bearer_token(
    resolver: &RuntimeSecretResolver,
    token: &mut ControlApiBearerToken,
    index: usize,
) -> Result<(), RuntimeSecretResolutionError> {
    if token.token_ref.is_none() {
        return Ok(());
    }

    let resolved = resolver.resolve_from_sources(
        (!token.token.trim().is_empty()).then_some(token.token.trim()),
        token.token_ref.as_ref(),
        &format!("observability.control_api.auth.bearer_tokens[{index}].token_ref"),
    )?;
    token.token = resolved.into_string(&format!(
        "observability.control_api.auth.bearer_tokens[{index}].token_ref"
    ))?;
    token.token_ref = None;
    Ok(())
}

pub fn resolve_file_secret_path(
    path: &str,
    field_name: &str,
) -> Result<RuntimeResolvedSecret, RuntimeSecretResolutionError> {
    resolve_file_bytes(path, field_name)
}

fn resolve_file_bytes(
    path: &str,
    field_name: &str,
) -> Result<RuntimeResolvedSecret, RuntimeSecretResolutionError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(RuntimeSecretResolutionError::new(
            field_name,
            Some(RuntimeSecretSourceKind::File),
            RuntimeSecretResolutionErrorKind::MalformedReference,
        ));
    }

    let bytes = match read_file_with_limit(trimmed, MAX_FILE_BACKED_SECRET_BYTES) {
        Ok(bytes) => bytes,
        Err(BoundedFileReadError::Io(err)) => return Err(map_io_error(field_name, err.kind())),
        Err(BoundedFileReadError::NotAFile) => {
            return Err(RuntimeSecretResolutionError::new(
                field_name,
                Some(RuntimeSecretSourceKind::File),
                RuntimeSecretResolutionErrorKind::NotAFile,
            ));
        }
        Err(BoundedFileReadError::TooLarge) => {
            return Err(RuntimeSecretResolutionError::new(
                field_name,
                Some(RuntimeSecretSourceKind::File),
                RuntimeSecretResolutionErrorKind::SecretTooLarge,
            ));
        }
    };
    if bytes.is_empty() {
        return Err(RuntimeSecretResolutionError::new(
            field_name,
            Some(RuntimeSecretSourceKind::File),
            RuntimeSecretResolutionErrorKind::EmptySecret,
        ));
    }

    Ok(resolved_secret(RuntimeSecretSourceKind::File, bytes))
}

fn resolved_secret(source_kind: RuntimeSecretSourceKind, bytes: Vec<u8>) -> RuntimeResolvedSecret {
    let fingerprint_sha256 = hex::encode(Sha256::digest(&bytes));
    RuntimeResolvedSecret {
        metadata: RuntimeResolvedSecretMetadata {
            source_kind,
            fingerprint_sha256,
            byte_len: bytes.len(),
            loaded_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
                .unwrap_or(0),
        },
        bytes,
    }
}

fn map_io_error(field_name: &str, kind: std::io::ErrorKind) -> RuntimeSecretResolutionError {
    let mapped = match kind {
        std::io::ErrorKind::NotFound => RuntimeSecretResolutionErrorKind::FileNotFound,
        std::io::ErrorKind::PermissionDenied => RuntimeSecretResolutionErrorKind::PermissionDenied,
        _ => RuntimeSecretResolutionErrorKind::Io,
    };
    RuntimeSecretResolutionError::new(field_name, Some(RuntimeSecretSourceKind::File), mapped)
}

fn runtime_secret_config_error(err: RuntimeSecretResolutionError) -> RuntimeConfigError {
    RuntimeConfigError::SecretResolutionFailed(err.to_string())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn literal_provider_resolves_bytes_and_stable_fingerprint() {
        let resolver = RuntimeSecretResolver::default();
        let secret = resolver
            .resolve(
                &SecretRef {
                    reference: "literal:super-secret".to_string(),
                },
                "auth.jwt.secret_ref",
            )
            .expect("literal secret");

        assert_eq!(secret.bytes(), b"super-secret");
        assert_eq!(
            secret.metadata().source_kind,
            RuntimeSecretSourceKind::Literal
        );
        assert_eq!(secret.metadata().byte_len, 12);
    }

    #[test]
    fn file_provider_uses_default_provider_base_dir_for_relative_refs() {
        let dir = tempdir().expect("tempdir");
        let secrets_dir = dir.path().join("secrets");
        fs::create_dir_all(&secrets_dir).expect("create secrets dir");
        fs::write(secrets_dir.join("jwt.secret"), b"base-dir-secret").expect("write secret");

        let resolver = RuntimeSecretResolver::from_secrets_config(&Secrets {
            default_provider: Some("local".to_string()),
            providers: std::iter::once((
                "local".to_string(),
                SecretProvider::File {
                    base_dir: Some(secrets_dir.to_string_lossy().to_string()),
                },
            ))
            .collect(),
        });

        let secret = resolver
            .resolve(
                &SecretRef {
                    reference: "file://jwt.secret".to_string(),
                },
                "auth.jwt.secret_ref",
            )
            .expect("file secret");

        assert_eq!(secret.bytes(), b"base-dir-secret");
    }

    #[test]
    fn file_provider_rejects_missing_file_with_sanitized_error() {
        let err = resolve_file_secret_path("/definitely/missing/secret.txt", "auth.jwt.secret_ref")
            .expect_err("missing file");

        assert_eq!(err.kind(), RuntimeSecretResolutionErrorKind::FileNotFound);
        assert_eq!(
            err.to_string(),
            "file secret resolution failed for auth.jwt.secret_ref: file_not_found"
        );
    }

    #[test]
    fn file_provider_rejects_unreadable_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("secret.txt");
        fs::write(&path, b"secret").expect("write");
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o000);
        fs::set_permissions(&path, perms).expect("chmod");

        let err = resolve_file_secret_path(path.to_string_lossy().as_ref(), "auth.jwt.secret_ref")
            .expect_err("permission denied");

        assert_eq!(
            err.kind(),
            RuntimeSecretResolutionErrorKind::PermissionDenied
        );
    }

    #[test]
    fn file_provider_rejects_empty_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("secret.txt");
        fs::write(&path, b"").expect("write");

        let err = resolve_file_secret_path(path.to_string_lossy().as_ref(), "auth.jwt.secret_ref")
            .expect_err("empty file");

        assert_eq!(err.kind(), RuntimeSecretResolutionErrorKind::EmptySecret);
    }

    #[test]
    fn file_provider_rejects_oversized_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("secret.txt");
        fs::write(
            &path,
            vec![b'x'; (MAX_FILE_BACKED_SECRET_BYTES as usize) + 1],
        )
        .expect("write");

        let err = resolve_file_secret_path(path.to_string_lossy().as_ref(), "auth.jwt.secret_ref")
            .expect_err("oversized file");

        assert_eq!(err.kind(), RuntimeSecretResolutionErrorKind::SecretTooLarge);
    }

    #[test]
    fn file_provider_accepts_file_at_size_limit() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("secret.txt");
        fs::write(&path, vec![b'x'; MAX_FILE_BACKED_SECRET_BYTES as usize]).expect("write");

        let secret =
            resolve_file_secret_path(path.to_string_lossy().as_ref(), "auth.jwt.secret_ref")
                .expect("file at size limit");

        assert_eq!(secret.bytes().len(), MAX_FILE_BACKED_SECRET_BYTES as usize);
        assert_eq!(
            secret.metadata().byte_len,
            MAX_FILE_BACKED_SECRET_BYTES as usize
        );
    }

    #[test]
    fn pem_helpers_reject_malformed_material() {
        let secret = resolved_secret(RuntimeSecretSourceKind::Literal, b"not-pem".to_vec());
        assert_eq!(
            secret
                .parse_pem_certificates("tls.client_certificate_ref")
                .expect_err("bad cert")
                .kind(),
            RuntimeSecretResolutionErrorKind::MalformedPemCertificate
        );
        assert_eq!(
            secret
                .parse_pem_private_key("tls.client_key_ref")
                .expect_err("bad key")
                .kind(),
            RuntimeSecretResolutionErrorKind::MalformedPemPrivateKey
        );
    }

    #[test]
    fn fingerprint_changes_when_file_contents_rotate() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("secret.txt");
        fs::write(&path, b"first-secret").expect("write");
        let first =
            resolve_file_secret_path(path.to_string_lossy().as_ref(), "auth.jwt.secret_ref")
                .expect("first");
        fs::write(&path, b"second-secret").expect("rewrite");
        let second =
            resolve_file_secret_path(path.to_string_lossy().as_ref(), "auth.jwt.secret_ref")
                .expect("second");

        assert_ne!(
            first.metadata().fingerprint_sha256,
            second.metadata().fingerprint_sha256
        );
    }

    #[test]
    fn file_provider_rejects_absolute_path_when_base_dir_is_configured() {
        let dir = tempdir().expect("tempdir");
        let secrets_dir = dir.path().join("secrets");
        fs::create_dir_all(&secrets_dir).expect("create secrets dir");

        let resolver = RuntimeSecretResolver::from_secrets_config(&Secrets {
            default_provider: Some("local".to_string()),
            providers: std::iter::once((
                "local".to_string(),
                SecretProvider::File {
                    base_dir: Some(secrets_dir.to_string_lossy().to_string()),
                },
            ))
            .collect(),
        });

        let absolute = dir.path().join("outside.txt");
        fs::write(&absolute, b"outside-secret").expect("write outside secret");

        let err = resolver
            .resolve(
                &SecretRef {
                    reference: format!("file://{}", absolute.to_string_lossy()),
                },
                "auth.jwt.secret_ref",
            )
            .expect_err("absolute path must be rejected");

        assert_eq!(
            err.kind(),
            RuntimeSecretResolutionErrorKind::PathOutsideBaseDir
        );
    }

    #[test]
    fn file_provider_rejects_parent_dir_escape_when_base_dir_is_configured() {
        let dir = tempdir().expect("tempdir");
        let secrets_dir = dir.path().join("secrets");
        fs::create_dir_all(&secrets_dir).expect("create secrets dir");
        fs::write(dir.path().join("outside.txt"), b"outside-secret").expect("write outside secret");

        let resolver = RuntimeSecretResolver::from_secrets_config(&Secrets {
            default_provider: Some("local".to_string()),
            providers: std::iter::once((
                "local".to_string(),
                SecretProvider::File {
                    base_dir: Some(secrets_dir.to_string_lossy().to_string()),
                },
            ))
            .collect(),
        });

        let err = resolver
            .resolve(
                &SecretRef {
                    reference: "file://../outside.txt".to_string(),
                },
                "auth.jwt.secret_ref",
            )
            .expect_err("parent-dir escape must be rejected");

        assert_eq!(
            err.kind(),
            RuntimeSecretResolutionErrorKind::PathOutsideBaseDir
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_provider_rejects_symlink_escape_when_base_dir_is_configured() {
        let dir = tempdir().expect("tempdir");
        let secrets_dir = dir.path().join("secrets");
        fs::create_dir_all(&secrets_dir).expect("create secrets dir");

        let outside = dir.path().join("outside.txt");
        fs::write(&outside, b"outside-secret").expect("write outside secret");
        symlink(&outside, secrets_dir.join("linked.txt")).expect("create symlink");

        let resolver = RuntimeSecretResolver::from_secrets_config(&Secrets {
            default_provider: Some("local".to_string()),
            providers: std::iter::once((
                "local".to_string(),
                SecretProvider::File {
                    base_dir: Some(secrets_dir.to_string_lossy().to_string()),
                },
            ))
            .collect(),
        });

        let err = resolver
            .resolve(
                &SecretRef {
                    reference: "file://linked.txt".to_string(),
                },
                "auth.jwt.secret_ref",
            )
            .expect_err("symlink escape must be rejected");

        assert_eq!(
            err.kind(),
            RuntimeSecretResolutionErrorKind::PathOutsideBaseDir
        );
    }
}
