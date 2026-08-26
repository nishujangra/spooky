use super::*;

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Security {
    pub privileges: PrivilegeDrop,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct PrivilegeDrop {
    pub enabled: bool,
    pub user: String,
    pub group: String,
}

impl Default for PrivilegeDrop {
    fn default() -> Self {
        Self {
            enabled: true,
            user: "nobody".to_string(),
            group: "nogroup".to_string(),
        }
    }
}
