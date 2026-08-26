use super::*;

pub const CURRENT_CONFIG_VERSION: u32 = 1;
pub const SUPPORTED_CONFIG_VERSIONS: &[u32] = &[CURRENT_CONFIG_VERSION];

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "Config::default_version")]
    pub version: u32,
    pub listen: Listen,
    #[serde(default)]
    pub listeners: Vec<Listen>,
    pub upstream: HashMap<String, Upstream>,
    #[serde(default)]
    pub load_balancing: Option<LoadBalancing>,
    #[serde(default)]
    pub upstream_tls: UpstreamTls,
    #[serde(default)]
    pub secrets: Secrets,
    #[serde(default)]
    pub log: Log,
    #[serde(default)]
    pub performance: Performance,
    #[serde(default)]
    pub observability: Observability,
    #[serde(default)]
    pub resilience: Resilience,
    #[serde(default)]
    pub security: Security,
}

impl Config {
    pub(crate) fn default_version() -> u32 {
        CURRENT_CONFIG_VERSION
    }
}

pub fn effective_listens(config: &Config) -> Vec<Listen> {
    if config.listeners.is_empty() {
        vec![config.listen.clone()]
    } else {
        config.listeners.clone()
    }
}
