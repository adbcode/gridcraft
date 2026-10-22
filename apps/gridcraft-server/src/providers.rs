//! Server-only HTTP adapters. These do not execute tools or retry uncertain calls.
//!
//! The future durable run loop owns tool authorization, receipts and conversation
//! persistence. Keep the complete assistant message when returning tool results.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, Response, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

pub const MAX_CHAT_REQUEST_BYTES: usize = 256 * 1024;
pub const MAX_CHAT_RESPONSE_BYTES: usize = 1024 * 1024;
pub const MAX_AUDIO_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_SPEECH_TEXT_BYTES: usize = 4 * 1024;
const MAX_MESSAGES: usize = 128;
const MAX_TOOLS: usize = 16;
const MAX_TOOL_ARGUMENT_BYTES: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Errors intentionally omit response bodies, credentials and request URLs.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderError {
    #[error("invalid provider configuration")]
    InvalidConfiguration,
    #[error("provider request exceeds the input limit")]
    InputLimit,
    #[error("invalid conversation or tool-result correlation")]
    InvalidConversation,
    #[error("provider connection failed")]
    Transport,
    #[error("provider request timed out; its remote outcome is unknown")]
    Timeout,
    #[error("provider returned HTTP status {status}")]
    Rejected { status: u16 },
    #[error("provider response exceeds the size limit")]
    ResponseLimit,
    #[error("provider returned an unsupported or invalid response")]
    InvalidResponse,
    #[error("a speech voice must be configured before synthesis")]
    VoiceNotConfigured,
}

// No Debug implementation: these configuration values contain credentials.
pub struct OpenAiConfig {
    pub api_key: String,
    /// API root including any version prefix, e.g. https://api.openai.com/v1.
    pub base_url: String,
    pub model: String,
    /// Optional `reasoning_effort` request field. Some reasoning models only
    /// accept function tools on chat completions when this is `none`.
    pub reasoning_effort: Option<String>,
}

pub struct ElevenLabsConfig {
    pub api_key: String,
    /// API root including /v1, e.g. https://api.elevenlabs.io/v1.
    pub base_url: String,
    pub stt_model: String,
    pub tts_model: String,
    pub voice_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// `provider_message` retains fields such as refusal and reasoning metadata.
/// Its normalized fields are checked again before replaying persisted messages.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AssistantTurn {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    provider_message: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ChatMessage {
    System(String),
    User(String),
    Assistant(AssistantTurn),
    ToolResult { tool_call_id: String, content: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Completion {
    pub assistant: AssistantTurn,
    /// Preserve truncation/refusal outcomes; a returned message is not proof of
    /// successful tool execution or a complete conversational answer.
    pub finish_reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Transcript {
    pub text: String,
    pub language_code: Option<String>,
}

#[derive(Clone)]
pub struct OpenAiClient {
    client: Client,
    endpoint: Url,
    model: String,
    reasoning_effort: Option<String>,
}

impl OpenAiClient {
    pub fn new(config: OpenAiConfig) -> Result<Self, ProviderError> {
        Self::with_timeout(config, REQUEST_TIMEOUT)
    }

    fn with_timeout(config: OpenAiConfig, timeout: Duration) -> Result<Self, ProviderError> {
        validate_model(&config.model)?;
        if let Some(effort) = &config.reasoning_effort
            && (effort.is_empty() || effort.len() > 16 || !effort.bytes().all(|byte| byte.is_ascii_lowercase()))
        {
            return Err(ProviderError::InvalidConfiguration);
        }
        let root = api_root(&config.base_url)?;
        let endpoint = root.join("chat/completions").map_err(|_| ProviderError::InvalidConfiguration)?;
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, secret_header(&config.api_key, true)?);
        Ok(Self { client: http_client(headers, timeout)?, endpoint, model: config.model, reasoning_effort: config.reasoning_effort })
    }

    pub async fn complete(&self, messages: &[ChatMessage], tools: &[ToolDefinition]) -> Result<Completion, ProviderError> {
        let messages = wire_messages(messages)?;
        if tools.len() > MAX_TOOLS {
            return Err(ProviderError::InputLimit);
        }
        let mut tool_names = BTreeSet::new();
        let mut wire_tools = Vec::with_capacity(tools.len());
        for tool in tools {
            if !valid_name(&tool.name) || !tool_names.insert(tool.name.clone()) || !tool.parameters.is_object() {
                return Err(ProviderError::InvalidConversation);
            }
            bounded_json(tool, MAX_TOOL_ARGUMENT_BYTES)?;
            wire_tools.push(json!({"type": "function", "function": tool}));
        }
        // Avoid provider-specific sampling or token-limit fields in the minimum
        // compatibility path. The model and API root are deployment choices.
        let mut request = json!({"model": self.model, "messages": messages});
        if let Some(effort) = &self.reasoning_effort {
            request["reasoning_effort"] = Value::String(effort.clone());
        }
        if !wire_tools.is_empty() {
            request["tools"] = Value::Array(wire_tools);
            request["parallel_tool_calls"] = Value::Bool(false);
        }
        let body = bounded_json(&request, MAX_CHAT_REQUEST_BYTES)?;
        let response =
            self.client.post(self.endpoint.clone()).header(CONTENT_TYPE, "application/json").body(body).send().await.map_err(transport_error)?;
        let bytes = response_bytes(response, MAX_CHAT_RESPONSE_BYTES).await?;
        let response: Value = serde_json::from_slice(&bytes).map_err(|_| ProviderError::InvalidResponse)?;
        let choices = response.get("choices").and_then(Value::as_array).filter(|choices| choices.len() == 1).ok_or(ProviderError::InvalidResponse)?;
        let choice = choices.first().ok_or(ProviderError::InvalidResponse)?;
        let finish_reason = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty() && reason.len() <= 64)
            .ok_or(ProviderError::InvalidResponse)?;
        let assistant = parse_assistant(choice.get("message").ok_or(ProviderError::InvalidResponse)?)?;
        // A truncated function call must never reach the live workbook adapter.
        if (!assistant.tool_calls.is_empty() && finish_reason != "tool_calls")
            || (assistant.tool_calls.is_empty() && finish_reason == "tool_calls")
            || assistant.tool_calls.iter().any(|call| !tool_names.contains(&call.name))
        {
            return Err(ProviderError::InvalidResponse);
        }
        Ok(Completion { assistant, finish_reason: finish_reason.to_owned() })
    }
}

#[derive(Clone)]
pub struct ElevenLabsClient {
    client: Client,
    root: Url,
    stt_model: String,
    tts_model: String,
    voice_id: Option<String>,
}

impl ElevenLabsClient {
    pub fn new(config: ElevenLabsConfig) -> Result<Self, ProviderError> {
        validate_model(&config.stt_model)?;
        validate_model(&config.tts_model)?;
        if config.voice_id.as_ref().is_some_and(|id| !valid_name(id)) {
            return Err(ProviderError::InvalidConfiguration);
        }
        let root = api_root(&config.base_url)?;
        let mut headers = HeaderMap::new();
        headers.insert("xi-api-key", secret_header(&config.api_key, false)?);
        Ok(Self {
            client: http_client(headers, REQUEST_TIMEOUT)?,
            root,
            stt_model: config.stt_model,
            tts_model: config.tts_model,
            voice_id: config.voice_id,
        })
    }

    /// Returns editable text only; transcription never submits an agent request.
    pub async fn transcribe(&self, audio: Vec<u8>, mime: &str) -> Result<Transcript, ProviderError> {
        if audio.is_empty() || audio.len() > MAX_AUDIO_BYTES {
            return Err(ProviderError::InputLimit);
        }
        let filename = match mime {
            "audio/webm" | "audio/webm;codecs=opus" => "recording.webm",
            "audio/ogg" | "audio/ogg;codecs=opus" => "recording.ogg",
            "audio/wav" | "audio/x-wav" => "recording.wav",
            "audio/mpeg" => "recording.mp3",
            "audio/mp4" => "recording.m4a",
            _ => return Err(ProviderError::InvalidConversation),
        };
        let part = reqwest::multipart::Part::bytes(audio).file_name(filename).mime_str(mime).map_err(|_| ProviderError::InvalidConversation)?;
        let form = reqwest::multipart::Form::new().text("model_id", self.stt_model.clone()).part("file", part);
        let endpoint = self.root.join("speech-to-text").map_err(|_| ProviderError::InvalidConfiguration)?;
        let response = self.client.post(endpoint).multipart(form).send().await.map_err(transport_error)?;
        let bytes = response_bytes(response, MAX_CHAT_RESPONSE_BYTES).await?;
        let transcript: Transcript = serde_json::from_slice(&bytes).map_err(|_| ProviderError::InvalidResponse)?;
        if transcript.text.len() > MAX_CHAT_REQUEST_BYTES || transcript.language_code.as_ref().is_some_and(|code| code.len() > 32) {
            return Err(ProviderError::ResponseLimit);
        }
        Ok(transcript)
    }

    /// The caller must supply a brief, verified result. Audio failure cannot undo
    /// workbook mutations that already completed.
    pub async fn synthesize(&self, text: &str) -> Result<Vec<u8>, ProviderError> {
        if text.trim().is_empty() || text.len() > MAX_SPEECH_TEXT_BYTES {
            return Err(ProviderError::InputLimit);
        }
        let voice = self.voice_id.as_ref().ok_or(ProviderError::VoiceNotConfigured)?;
        let mut endpoint = self.root.join(&format!("text-to-speech/{voice}")).map_err(|_| ProviderError::InvalidConfiguration)?;
        endpoint.query_pairs_mut().append_pair("output_format", "mp3_44100_128");
        let body = bounded_json(&json!({"text": text, "model_id": self.tts_model}), MAX_CHAT_REQUEST_BYTES)?;
        let response = self
            .client
            .post(endpoint)
            .header(CONTENT_TYPE, "application/json")
            .header("accept", "audio/mpeg")
            .body(body)
            .send()
            .await
            .map_err(transport_error)?;
        let content_type = response.headers().get(CONTENT_TYPE).and_then(|value| value.to_str().ok()).map(str::to_owned);
        let bytes = response_bytes(response, MAX_AUDIO_BYTES).await?;
        if bytes.is_empty()
            || !content_type.as_deref().is_some_and(|mime| matches!(mime.split(';').next(), Some("audio/mpeg" | "application/octet-stream")))
        {
            return Err(ProviderError::InvalidResponse);
        }
        Ok(bytes)
    }
}

fn api_root(value: &str) -> Result<Url, ProviderError> {
    if value.len() > 2048 {
        return Err(ProviderError::InvalidConfiguration);
    }
    let mut url = Url::parse(value).map_err(|_| ProviderError::InvalidConfiguration)?;
    let loopback =
        url.host_str().and_then(|host| host.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok()).is_some_and(|address| address.is_loopback());
    if !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ProviderError::InvalidConfiguration);
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
}

fn validate_model(model: &str) -> Result<(), ProviderError> {
    if model.trim().is_empty() || model.len() > 128 || model.chars().any(char::is_control) {
        return Err(ProviderError::InvalidConfiguration);
    }
    Ok(())
}

fn secret_header(key: &str, bearer: bool) -> Result<HeaderValue, ProviderError> {
    if key.trim().is_empty() || key.len() > 4096 {
        return Err(ProviderError::InvalidConfiguration);
    }
    let value = if bearer { format!("Bearer {key}") } else { key.to_owned() };
    let mut header = HeaderValue::from_str(&value).map_err(|_| ProviderError::InvalidConfiguration)?;
    header.set_sensitive(true);
    Ok(header)
}

fn http_client(headers: HeaderMap, timeout: Duration) -> Result<Client, ProviderError> {
    Client::builder()
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(timeout.min(Duration::from_secs(10)))
        .timeout(timeout)
        .build()
        .map_err(|_| ProviderError::InvalidConfiguration)
}

fn transport_error(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() { ProviderError::Timeout } else { ProviderError::Transport }
}

async fn response_bytes(mut response: Response, max: usize) -> Result<Vec<u8>, ProviderError> {
    if !response.status().is_success() {
        return Err(ProviderError::Rejected { status: response.status().as_u16() });
    }
    if response.content_length().is_some_and(|length| length > max as u64) {
        return Err(ProviderError::ResponseLimit);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if chunk.len() > max.saturating_sub(bytes.len()) {
            return Err(ProviderError::ResponseLimit);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for LimitedWriter {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if input.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("serialization limit"));
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn bounded_json(value: &impl Serialize, limit: usize) -> Result<Vec<u8>, ProviderError> {
    let mut writer = LimitedWriter { bytes: Vec::new(), limit };
    serde_json::to_writer(&mut writer, value).map_err(|_| ProviderError::InputLimit)?;
    Ok(writer.bytes)
}

fn valid_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn valid_call_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn parse_assistant(message: &Value) -> Result<AssistantTurn, ProviderError> {
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return Err(ProviderError::InvalidResponse);
    }
    let text = match message.get("content") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        _ => return Err(ProviderError::InvalidResponse),
    };
    let mut tool_calls = Vec::new();
    let mut ids = BTreeSet::new();
    if let Some(calls) = message.get("tool_calls").filter(|value| !value.is_null()) {
        let calls = calls.as_array().filter(|calls| calls.len() <= MAX_TOOLS).ok_or(ProviderError::InvalidResponse)?;
        for call in calls {
            if call.get("type").and_then(Value::as_str) != Some("function") {
                return Err(ProviderError::InvalidResponse);
            }
            let id = call.get("id").and_then(Value::as_str).filter(|id| valid_call_id(id)).ok_or(ProviderError::InvalidResponse)?;
            let function = call.get("function").ok_or(ProviderError::InvalidResponse)?;
            let name = function.get("name").and_then(Value::as_str).filter(|name| valid_name(name)).ok_or(ProviderError::InvalidResponse)?;
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .filter(|arguments| arguments.len() <= MAX_TOOL_ARGUMENT_BYTES)
                .ok_or(ProviderError::InvalidResponse)?;
            let arguments: Value = serde_json::from_str(arguments).map_err(|_| ProviderError::InvalidResponse)?;
            if !arguments.is_object() || !ids.insert(id) {
                return Err(ProviderError::InvalidResponse);
            }
            tool_calls.push(ToolCall { id: id.to_owned(), name: name.to_owned(), arguments });
        }
    }
    Ok(AssistantTurn { text, tool_calls, provider_message: message.clone() })
}

fn wire_messages(messages: &[ChatMessage]) -> Result<Vec<Value>, ProviderError> {
    if messages.is_empty() || messages.len() > MAX_MESSAGES {
        return Err(ProviderError::InputLimit);
    }
    let mut pending = BTreeSet::new();
    let mut all_calls = BTreeSet::new();
    let mut wire = Vec::with_capacity(messages.len());
    for message in messages {
        bounded_json(message, MAX_CHAT_REQUEST_BYTES)?;
        let value = match message {
            ChatMessage::System(content) | ChatMessage::User(content) => {
                if !pending.is_empty() {
                    return Err(ProviderError::InvalidConversation);
                }
                let role = if matches!(message, ChatMessage::System(_)) { "system" } else { "user" };
                json!({"role": role, "content": content})
            }
            ChatMessage::Assistant(assistant) => {
                let parsed = parse_assistant(&assistant.provider_message).map_err(|_| ProviderError::InvalidConversation)?;
                if !pending.is_empty() || parsed != *assistant {
                    return Err(ProviderError::InvalidConversation);
                }
                for call in &assistant.tool_calls {
                    if !all_calls.insert(call.id.clone()) {
                        return Err(ProviderError::InvalidConversation);
                    }
                    pending.insert(call.id.clone());
                }
                assistant.provider_message.clone()
            }
            ChatMessage::ToolResult { tool_call_id, content } => {
                if !pending.remove(tool_call_id) {
                    return Err(ProviderError::InvalidConversation);
                }
                json!({"role": "tool", "tool_call_id": tool_call_id, "content": content})
            }
        };
        wire.push(value);
    }
    if !pending.is_empty() {
        return Err(ProviderError::InvalidConversation);
    }
    Ok(wire)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::body::Bytes;
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::{Json, Router};

    use super::*;

    struct MockServer {
        root: String,
        task: tokio::task::JoinHandle<io::Result<()>>,
    }

    impl MockServer {
        async fn start(router: Router) -> io::Result<Self> {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let root = format!("http://{}/v1", listener.local_addr()?);
            let task = tokio::spawn(async move { axum::serve(listener, router).await });
            Ok(Self { root, task })
        }

        fn chat(&self) -> Result<OpenAiClient, ProviderError> {
            OpenAiClient::new(OpenAiConfig {
                api_key: "test-only-secret".to_owned(),
                base_url: self.root.clone(),
                model: "test-model".to_owned(),
                reasoning_effort: None,
            })
        }

        fn speech(&self) -> Result<ElevenLabsClient, ProviderError> {
            ElevenLabsClient::new(ElevenLabsConfig {
                api_key: "speech-test-secret".to_owned(),
                base_url: self.root.clone(),
                stt_model: "test-transcribe".to_owned(),
                tts_model: "test-speak".to_owned(),
                voice_id: Some("test-voice".to_owned()),
            })
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    fn tool_message() -> Value {
        json!({
            "role": "assistant", "content": "I will inspect the cells.",
            "refusal": null, "reasoning_content": "provider-specific-context",
            "tool_calls": [{"id": "call_1", "type": "function", "function": {
                "name": "inspect_workbook", "arguments": "{\"range\":\"A1:E5\"}"
            }}]
        })
    }

    fn tools() -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "inspect_workbook".to_owned(),
            description: "Inspect a bounded live range".to_owned(),
            parameters: json!({
                "type": "object", "properties": {"range": {"type": "string"}},
                "required": ["range"], "additionalProperties": false
            }),
        }]
    }

    #[tokio::test]
    async fn chat_round_trip_preserves_assistant_and_correlates_results() -> Result<(), Box<dyn Error>> {
        let received = Arc::new(Mutex::new(Vec::<(HeaderMap, Value)>::new()));
        let capture = Arc::clone(&received);
        let router = Router::new().route(
            "/v1/chat/completions",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let capture = Arc::clone(&capture);
                async move {
                    let Ok(mut requests) = capture.lock() else {
                        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({})));
                    };
                    requests.push((headers, body));
                    let (message, finish) = if requests.len() == 1 {
                        (tool_message(), "tool_calls")
                    } else {
                        (json!({"role": "assistant", "content": "Cells inspected."}), "stop")
                    };
                    (StatusCode::OK, Json(json!({"choices": [{"message": message, "finish_reason": finish}]})))
                }
            }),
        );
        let server = MockServer::start(router).await?;
        let client = server.chat()?;
        let mut conversation =
            vec![ChatMessage::System("Use only the declared tools.".to_owned()), ChatMessage::User("Inspect this sheet.".to_owned())];
        let first = client.complete(&conversation, &tools()).await?;
        assert_eq!(first.assistant.tool_calls.first().map(|call| call.id.as_str()), Some("call_1"));
        // Round-trip through durable JSON without dropping provider-owned fields.
        let persisted = serde_json::to_vec(&first.assistant)?;
        conversation.push(ChatMessage::Assistant(serde_json::from_slice(&persisted)?));
        conversation.push(ChatMessage::ToolResult { tool_call_id: "call_1".to_owned(), content: "{\"cells\":[]}".to_owned() });
        let second = client.complete(&conversation, &tools()).await?;
        assert_eq!(second.assistant.text.as_deref(), Some("Cells inspected."));
        let requests = received.lock().map_err(|_| io::Error::other("test lock"))?;
        assert_eq!(requests.len(), 2);
        let (headers, body) = requests.first().ok_or("first request missing")?;
        assert_eq!(headers.get(AUTHORIZATION).and_then(|value| value.to_str().ok()), Some("Bearer test-only-secret"));
        assert_eq!(body.get("model"), Some(&json!("test-model")));
        assert_eq!(body.pointer("/tools/0/function/name"), Some(&json!("inspect_workbook")));
        let (_, body) = requests.get(1).ok_or("second request missing")?;
        assert_eq!(body.pointer("/messages/2"), Some(&tool_message()));
        assert_eq!(body.pointer("/messages/3/tool_call_id"), Some(&json!("call_1")));
        assert_eq!(body.pointer("/messages/3/role"), Some(&json!("tool")));
        Ok(())
    }

    #[test]
    fn rejects_wrong_missing_duplicate_or_modified_tool_results() -> Result<(), Box<dyn Error>> {
        let assistant = parse_assistant(&tool_message())?;
        let base = vec![ChatMessage::Assistant(assistant.clone())];
        assert_eq!(wire_messages(&base), Err(ProviderError::InvalidConversation));
        let mut wrong = base.clone();
        wrong.push(ChatMessage::ToolResult { tool_call_id: "wrong".to_owned(), content: "done".to_owned() });
        assert_eq!(wire_messages(&wrong), Err(ProviderError::InvalidConversation));
        let result = ChatMessage::ToolResult { tool_call_id: "call_1".to_owned(), content: "done".to_owned() };
        let mut duplicate = base;
        duplicate.push(result.clone());
        assert!(wire_messages(&duplicate).is_ok());
        duplicate.push(result.clone());
        assert_eq!(wire_messages(&duplicate), Err(ProviderError::InvalidConversation));
        let mut modified = assistant;
        modified.text = Some("a different message".to_owned());
        assert_eq!(wire_messages(&[ChatMessage::Assistant(modified), result]), Err(ProviderError::InvalidConversation));
        Ok(())
    }

    #[tokio::test]
    async fn request_and_response_bodies_are_bounded() -> Result<(), Box<dyn Error>> {
        let server =
            MockServer::start(Router::new().route("/v1/chat/completions", post(|| async { "x".repeat(MAX_CHAT_RESPONSE_BYTES + 1) }))).await?;
        let client = server.chat()?;
        let over_limit = vec![ChatMessage::User("\n".repeat(MAX_CHAT_REQUEST_BYTES / 2))];
        assert_eq!(client.complete(&over_limit, &[]).await, Err(ProviderError::InputLimit));
        assert_eq!(client.complete(&[ChatMessage::User("Hello".to_owned())], &[]).await, Err(ProviderError::ResponseLimit));
        Ok(())
    }

    #[tokio::test]
    async fn errors_omit_secrets_and_redirects_are_not_followed() -> Result<(), Box<dyn Error>> {
        let redirected = Arc::new(AtomicUsize::new(0));
        let redirect_count = Arc::clone(&redirected);
        let server = MockServer::start(
            Router::new()
                .route(
                    "/v1/chat/completions",
                    post(|| async { (StatusCode::TEMPORARY_REDIRECT, [("location", "/unexpected")], "test-only-secret echoed by upstream") }),
                )
                .route(
                    "/unexpected",
                    post(move || {
                        redirect_count.fetch_add(1, Ordering::SeqCst);
                        async { "must not be called" }
                    }),
                ),
        )
        .await?;
        let result = server.chat()?.complete(&[ChatMessage::User("Hello".to_owned())], &[]).await;
        assert_eq!(result, Err(ProviderError::Rejected { status: 307 }));
        assert_eq!(redirected.load(Ordering::SeqCst), 0);
        let error = result.err().ok_or("missing error")?;
        assert!(!format!("{error:?} {error}").contains("test-only-secret"));
        assert!(!format!("{error:?} {error}").contains(&server.root));
        Ok(())
    }

    #[tokio::test]
    async fn a_timeout_is_not_retried() -> Result<(), Box<dyn Error>> {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&calls);
        let server = MockServer::start(Router::new().route(
            "/v1/chat/completions",
            post(move || {
                count.fetch_add(1, Ordering::SeqCst);
                async {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    "late result"
                }
            }),
        ))
        .await?;
        let client = OpenAiClient::with_timeout(
            OpenAiConfig {
                api_key: "test-only-secret".to_owned(),
                base_url: server.root.clone(),
                model: "test-model".to_owned(),
                reasoning_effort: None,
            },
            Duration::from_millis(25),
        )?;
        assert_eq!(client.complete(&[ChatMessage::User("Hello".to_owned())], &[]).await, Err(ProviderError::Timeout));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn configuration_rejects_insecure_remote_urls_and_embedded_credentials() {
        for url in [
            "http://example.com/v1",
            "https://user:secret@example.com/v1",
            "https://example.com/v1?key=secret",
            "https://example.com/v1#fragment",
            "file:///tmp/provider",
        ] {
            assert_eq!(api_root(url), Err(ProviderError::InvalidConfiguration));
        }
        assert!(api_root("http://127.0.0.1:8080/v1").is_ok());
        assert!(api_root("http://[::1]:8080/v1").is_ok());
        assert_eq!(secret_header("bad\nheader", true), Err(ProviderError::InvalidConfiguration));
    }

    #[tokio::test]
    async fn speech_uses_multipart_and_returns_mp3_with_bounded_inputs() -> Result<(), Box<dyn Error>> {
        let received = Arc::new(Mutex::new(Vec::<(HeaderMap, Vec<u8>)>::new()));
        let stt_capture = Arc::clone(&received);
        let tts_capture = Arc::clone(&received);
        let server = MockServer::start(
            Router::new()
                .route(
                    "/v1/speech-to-text",
                    post(move |headers: HeaderMap, body: Bytes| {
                        if let Ok(mut requests) = stt_capture.lock() {
                            requests.push((headers, body.to_vec()));
                        }
                        async { Json(json!({"text": "Compare sales", "language_code": "en", "words": []})) }
                    }),
                )
                .route(
                    "/v1/text-to-speech/test-voice",
                    post(move |headers: HeaderMap, body: Bytes| {
                        if let Ok(mut requests) = tts_capture.lock() {
                            requests.push((headers, body.to_vec()));
                        }
                        async { ([("content-type", "audio/mpeg")], b"fake-mp3".to_vec()) }
                    }),
                ),
        )
        .await?;
        let client = server.speech()?;
        let transcript = client.transcribe(b"fake-webm".to_vec(), "audio/webm").await?;
        assert_eq!(transcript.text, "Compare sales");
        assert_eq!(client.synthesize("Verified result.").await?, b"fake-mp3");
        assert_eq!(client.transcribe(Vec::new(), "audio/webm").await, Err(ProviderError::InputLimit));
        assert_eq!(client.transcribe(vec![1], "text/plain").await, Err(ProviderError::InvalidConversation));
        assert_eq!(client.synthesize(&"x".repeat(MAX_SPEECH_TEXT_BYTES + 1)).await, Err(ProviderError::InputLimit));
        let requests = received.lock().map_err(|_| io::Error::other("test lock"))?;
        assert_eq!(requests.len(), 2);
        let (headers, body) = requests.first().ok_or("STT request missing")?;
        assert_eq!(headers.get("xi-api-key").and_then(|value| value.to_str().ok()), Some("speech-test-secret"));
        let multipart = std::str::from_utf8(body)?;
        assert!(multipart.contains("name=\"model_id\""));
        assert!(multipart.contains("test-transcribe"));
        assert!(multipart.contains("filename=\"recording.webm\""));
        assert!(multipart.contains("fake-webm"));
        let (_, body) = requests.get(1).ok_or("TTS request missing")?;
        let json: Value = serde_json::from_slice(body)?;
        assert_eq!(json.get("model_id"), Some(&json!("test-speak")));
        assert_eq!(json.get("text"), Some(&json!("Verified result.")));
        Ok(())
    }

    #[test]
    fn malformed_arguments_and_duplicate_calls_cannot_become_tools() {
        let mut malformed = tool_message();
        malformed["tool_calls"][0]["function"]["arguments"] = json!("{unfinished");
        assert_eq!(parse_assistant(&malformed), Err(ProviderError::InvalidResponse));
        let mut duplicate = tool_message();
        let call = duplicate.pointer("/tool_calls/0").cloned();
        if let Some(call) = call {
            duplicate["tool_calls"] = json!([call, call]);
        }
        assert_eq!(parse_assistant(&duplicate), Err(ProviderError::InvalidResponse));
    }

    #[tokio::test]
    async fn truncated_and_undeclared_calls_are_rejected() -> Result<(), Box<dyn Error>> {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&calls);
        let server = MockServer::start(Router::new().route(
            "/v1/chat/completions",
            post(move || {
                let finish = if count.fetch_add(1, Ordering::SeqCst) == 0 { "length" } else { "tool_calls" };
                async move { Json(json!({"choices": [{"message": tool_message(), "finish_reason": finish}]})) }
            }),
        ))
        .await?;
        let client = server.chat()?;
        let messages = [ChatMessage::User("Inspect the sheet".to_owned())];
        assert_eq!(client.complete(&messages, &tools()).await, Err(ProviderError::InvalidResponse));
        assert_eq!(client.complete(&messages, &[]).await, Err(ProviderError::InvalidResponse));
        Ok(())
    }
}
