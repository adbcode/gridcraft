//! Serde-only contracts shared by the durable agent service and browser shell.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::agent_bridge::Context;

pub type Json = Value;

pub const MAX_REQUEST_ID_BYTES: usize = 128;
pub const MAX_WORKBOOK_SESSION_ID_BYTES: usize = 256;
pub const MAX_PROMPT_BYTES: usize = 2_000;
pub const MAX_TOOL_CALL_ID_BYTES: usize = 128;
pub const MAX_TOOL_NAME_BYTES: usize = 128;
pub const MAX_ARGUMENT_BYTES: usize = 65_536;
pub const MAX_ERROR_BYTES: usize = 512;
pub const MAX_FINAL_TEXT_BYTES: usize = 16_384;
pub const MAX_SOURCE_ITEMS: usize = 32;
pub const MAX_ATTACHMENTS: usize = 16;
pub const MAX_ACTION_COUNT: i32 = 1_000;

fn bounded_string<'de, D>(deserializer: D, max: usize) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(serde::de::Error::custom("string exceeds the supported limit"));
    }
    Ok(value)
}

fn request_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_string(deserializer, MAX_REQUEST_ID_BYTES)
}

fn session_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_string(deserializer, MAX_WORKBOOK_SESSION_ID_BYTES)
}

fn prompt<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_string(deserializer, MAX_PROMPT_BYTES)
}

fn tool_call_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_string(deserializer, MAX_TOOL_CALL_ID_BYTES)
}

fn tool_name<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_string(deserializer, MAX_TOOL_NAME_BYTES)
}

fn optional_bounded<'de, D>(deserializer: D, max: usize) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?
        .map(|value| {
            if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
                Err(serde::de::Error::custom("string exceeds the supported limit"))
            } else {
                Ok(value)
            }
        })
        .transpose()
}

fn optional_error<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    optional_bounded(deserializer, MAX_ERROR_BYTES)
}

fn optional_final_text<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    optional_bounded(deserializer, MAX_FINAL_TEXT_BYTES)
}

fn json_arguments<'de, D>(deserializer: D) -> Result<Json, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Json::deserialize(deserializer)?;
    let bytes = serde_json::to_vec(&value).map_err(serde::de::Error::custom)?;
    if bytes.len() > MAX_ARGUMENT_BYTES {
        return Err(serde::de::Error::custom("arguments exceed the supported limit"));
    }
    Ok(value)
}

fn sha256<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(serde::de::Error::custom("sha256 must be 64 hexadecimal characters"));
    }
    Ok(value)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentRef {
    #[serde(deserialize_with = "bounded_uuid_string")]
    pub workbook_id: String,
    pub version: i64,
    #[serde(deserialize_with = "sha256")]
    pub sha256: String,
}

fn bounded_uuid_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = bounded_string(deserializer, 64)?;
    let valid_uuid = value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| if matches!(index, 8 | 13 | 18 | 23) { byte == b'-' } else { byte.is_ascii_hexdigit() });
    if !valid_uuid {
        return Err(serde::de::Error::custom("workbook_id must be a UUID"));
    }
    Ok(value)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunStart {
    #[serde(deserialize_with = "request_id")]
    pub request_id: String,
    #[serde(deserialize_with = "session_id")]
    pub workbook_session_id: String,
    #[serde(deserialize_with = "prompt")]
    pub text: String,
    #[serde(default, deserialize_with = "bounded_attachments")]
    pub attachments: Vec<AttachmentRef>,
    pub context: Context,
}

fn bounded_attachments<'de, D>(deserializer: D) -> Result<Vec<AttachmentRef>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<AttachmentRef>::deserialize(deserializer)?;
    if values.len() > MAX_ATTACHMENTS {
        return Err(serde::de::Error::custom("too many attachments"));
    }
    Ok(values)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolOutcome {
    pub ok: bool,
    #[serde(default, deserialize_with = "optional_json")]
    pub result: Option<Json>,
    #[serde(default, deserialize_with = "optional_error")]
    pub error: Option<String>,
}

fn optional_json<'de, D>(deserializer: D) -> Result<Option<Json>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Json>::deserialize(deserializer)?;
    if let Some(value) = &value {
        let bytes = serde_json::to_vec(value).map_err(serde::de::Error::custom)?;
        if bytes.len() > MAX_ARGUMENT_BYTES {
            return Err(serde::de::Error::custom("result exceeds the supported limit"));
        }
    }
    Ok(value)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunContinue {
    #[serde(deserialize_with = "tool_call_id")]
    pub tool_call_id: String,
    pub outcome: ToolOutcome,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WritePlan {
    #[serde(deserialize_with = "bounded_string_256")]
    pub sheet: String,
    #[serde(deserialize_with = "bounded_string_64")]
    pub range: String,
    #[serde(deserialize_with = "bounded_values")]
    pub values: Vec<Vec<Json>>,
}

fn bounded_string_256<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_string(deserializer, 256)
}

fn bounded_string_64<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_string(deserializer, 64)
}

fn bounded_values<'de, D>(deserializer: D) -> Result<Vec<Vec<Json>>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<Vec<Json>>::deserialize(deserializer)?;
    let rows = values.len();
    let columns = values.first().map_or(0, Vec::len);
    if rows == 0
        || columns == 0
        || rows > 1_000
        || columns > 1_000
        || rows.saturating_mul(columns) > 1_000
        || values.iter().any(|row| row.len() != columns)
    {
        return Err(serde::de::Error::custom("write plan must be a bounded rectangular range"));
    }
    Ok(values)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingTool {
    #[serde(deserialize_with = "tool_call_id")]
    pub tool_call_id: String,
    #[serde(deserialize_with = "tool_name")]
    pub name: String,
    #[serde(deserialize_with = "json_arguments")]
    pub arguments: Json,
    pub write: Option<WritePlan>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    #[serde(deserialize_with = "bounded_string_64")]
    pub kind: String,
    #[serde(deserialize_with = "bounded_string_256")]
    pub label: String,
    #[serde(deserialize_with = "bounded_string_256")]
    pub id: String,
    pub version: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunView {
    #[serde(deserialize_with = "bounded_string_64")]
    pub run_id: String,
    #[serde(deserialize_with = "bounded_string_32")]
    pub status: String,
    pub pending_tool: Option<PendingTool>,
    #[serde(default, deserialize_with = "optional_final_text")]
    pub final_text: Option<String>,
    pub action_count: i32,
    #[serde(default, deserialize_with = "bounded_sources")]
    pub sources: Vec<SourceRef>,
    #[serde(default)]
    pub spoken_summary: Option<String>,
}

fn bounded_string_32<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_string(deserializer, 32)
}

fn bounded_sources<'de, D>(deserializer: D) -> Result<Vec<SourceRef>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<SourceRef>::deserialize(deserializer)?;
    if values.len() > MAX_SOURCE_ITEMS {
        return Err(serde::de::Error::custom("too many sources"));
    }
    Ok(values)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptView {
    #[serde(deserialize_with = "bounded_string_256")]
    pub text: String,
    #[serde(default, deserialize_with = "optional_language")]
    pub language_code: Option<String>,
}

fn optional_language<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    optional_bounded(deserializer, 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_protocol_rejects_oversized_prompt_and_non_rectangular_writes() {
        let oversized = serde_json::json!({
            "request_id":"r",
            "workbook_session_id":"s",
            "text":"x".repeat(MAX_PROMPT_BYTES + 1),
            "attachments":[],
            "context": {
                "identity":{"document_id":1,"sheet_index":0,"sheet_name":"Sheet1","change_token":1},
                "selection":"A1","active_cell":"A1","dirty":false,"undo_available":false,"ready":true,"calculation_mode":"Automatic"
            }
        });
        assert!(serde_json::from_value::<RunStart>(oversized).is_err());
        let write = serde_json::json!({"sheet":"Sheet1","range":"A1:B2","values":[[1],[2,3]]});
        assert!(serde_json::from_value::<WritePlan>(write).is_err());
    }
}
