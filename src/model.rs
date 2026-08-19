use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CONTRACT_VERSION: &str = "charles-local/v1";
pub const REQUIRED_CHARLES_VERSION: &str = "4.6.8";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub contract_version: String,
    pub status: ResponseStatus,
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<Checkpoint>,
}

impl Response {
    pub fn ready(operation: impl Into<String>, data: Value) -> Self {
        Self {
            contract_version: CONTRACT_VERSION.into(),
            status: ResponseStatus::Ready,
            operation: operation.into(),
            data: Some(data),
            error: None,
            checkpoint: None,
        }
    }

    pub fn needs_action(operation: impl Into<String>, data: Value, checkpoint: Checkpoint) -> Self {
        Self {
            contract_version: CONTRACT_VERSION.into(),
            status: ResponseStatus::NeedsUserAction,
            operation: operation.into(),
            data: Some(data),
            error: None,
            checkpoint: Some(checkpoint),
        }
    }

    pub fn error(
        operation: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            contract_version: CONTRACT_VERSION.into(),
            status: ResponseStatus::Error,
            operation: operation.into(),
            data: None,
            error: Some(ApiError {
                code: code.into(),
                message: message.into(),
            }),
            checkpoint: None,
        }
    }

    pub fn is_error(&self) -> bool {
        self.status == ResponseStatus::Error
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Ready,
    NeedsUserAction,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub kind: String,
    pub instruction: String,
    pub resume_token: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DevicePlatform {
    #[default]
    Host,
    Android,
    Ios,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupPlanRequest {
    pub profile: String,
    #[serde(default)]
    pub platform: DevicePlatform,
    #[serde(default)]
    pub device: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenRequest {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct EmptyRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRecord {
    pub pid: u32,
    pub executable: PathBuf,
    pub marker: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProxySnapshot {
    pub device_id: String,
    pub previous_value: String,
    pub configured_value: String,
}

impl ProxySnapshot {
    pub fn restore_value<'a>(&'a self, current: &str) -> Option<&'a str> {
        (current.trim() == self.configured_value.trim()).then_some(&self.previous_value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReverseSnapshot {
    pub device_id: String,
    pub device_port: u16,
    pub host_port: u16,
    pub owned: bool,
}
