use super::*;

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Secrets {
    pub default_provider: Option<String>,
    pub providers: HashMap<String, SecretProvider>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SecretProvider {
    File {
        #[serde(default)]
        base_dir: Option<String>,
    },
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SecretRef {
    #[serde(rename = "ref")]
    pub reference: String,
}

impl SecretRef {
    pub fn scheme(&self) -> Option<&str> {
        self.reference.split_once(':').map(|(scheme, _)| scheme)
    }

    pub fn raw_value(&self) -> &str {
        &self.reference
    }
}
