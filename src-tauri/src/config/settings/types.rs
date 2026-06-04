use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsOption {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SecretStatus {
    pub configured: bool,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    pub config_path: String,
    pub revision: String,
    pub values: Value,
    pub dynamic_options: BTreeMap<String, Vec<SettingsOption>>,
    pub secret_statuses: BTreeMap<String, SecretStatus>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsPatchOperation {
    Set,
    Delete,
}

fn default_patch_operation() -> SettingsPatchOperation {
    SettingsPatchOperation::Set
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub path: String,
    #[serde(default)]
    pub value: Option<Value>,
    pub expected_revision: String,
    #[serde(default = "default_patch_operation")]
    pub operation: SettingsPatchOperation,
    /// Agent profile to apply the patch to. Defaults to "main".
    #[serde(default)]
    pub agent_id: Option<String>,
}
