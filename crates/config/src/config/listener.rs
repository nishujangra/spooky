use super::*;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Listen {
    pub protocol: String,
    pub port: u16,
    pub address: String,
    pub tls: Tls,
}

impl Default for Listen {
    fn default() -> Self {
        Self {
            protocol: "http3".to_string(),
            port: 9889,
            address: "0.0.0.0".to_string(),
            tls: Tls::default(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Tls {
    pub cert: String,
    pub key: String,
    pub certificates: Vec<TlsCertificate>,
    pub client_auth: ClientAuth,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct TlsCertificate {
    pub server_name: String,
    pub cert: String,
    pub key: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ClientAuth {
    pub enabled: bool,
    pub require_client_cert: bool,
    pub ca_file: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct UpstreamTls {
    pub verify_certificates: bool,
    pub strict_sni: bool,
    pub ca_file: Option<String>,
    pub ca_dir: Option<String>,
    pub client_certificate: Option<String>,
    pub client_certificate_ref: Option<SecretRef>,
    pub client_key: Option<String>,
    pub client_key_ref: Option<SecretRef>,
}

impl Default for UpstreamTls {
    fn default() -> Self {
        Self {
            verify_certificates: true,
            strict_sni: true,
            ca_file: None,
            ca_dir: None,
            client_certificate: None,
            client_certificate_ref: None,
            client_key: None,
            client_key_ref: None,
        }
    }
}
