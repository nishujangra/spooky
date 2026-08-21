use std::collections::HashMap;

use impulse_config::runtime::{RuntimeListenerTls, RuntimeTlsIdentity};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTlsCertificateMetadata {
    pub serial_hex: String,
    pub dns_names: Vec<String>,
    pub not_before_unix_seconds: i64,
    pub not_after_unix_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLoadedTlsIdentity {
    pub identity: RuntimeTlsIdentity,
    pub metadata: RuntimeTlsCertificateMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLoadedClientAuthCa {
    pub ca_file: String,
    pub certificate_count: usize,
}

#[derive(Debug, Clone)]
pub struct ListenerTlsInventory {
    pub listener_tls: RuntimeListenerTls,
    pub default_identity: RuntimeLoadedTlsIdentity,
    pub sni_identities: HashMap<String, RuntimeLoadedTlsIdentity>,
    pub client_auth_ca: Option<RuntimeLoadedClientAuthCa>,
}
