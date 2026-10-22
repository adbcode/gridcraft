//! Transport-neutral boundary to the connected agent service. Hosts perform same-origin HTTP
//! (the web shell uses browser fetch); this crate never depends on a browser or HTTP library.
//! Responses arrive asynchronously and are drained once per frame.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use serde_json::Value as Json;

/// Largest response body a host should deliver; larger bodies are reported as failures.
pub const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
}

/// A multipart upload: a `metadata` text field and a `file` field.
#[derive(Clone, Debug, PartialEq)]
pub struct Upload {
    pub metadata: String,
    pub file_name: String,
    pub bytes: Vec<u8>,
}

/// `path` is always a same-origin path beginning with `/api/`.
#[derive(Clone, Debug, PartialEq)]
pub struct ApiRequest {
    pub ticket: u64,
    pub method: Method,
    pub path: String,
    pub body: Option<Json>,
    pub upload: Option<Upload>,
}

/// `status == 0` means no HTTP response arrived (network failure or an oversized body).
#[derive(Clone, Debug, PartialEq)]
pub struct ApiResponse {
    pub ticket: u64,
    pub status: u16,
    pub body: Vec<u8>,
    /// Only the service's `x-gridcraft-*` headers, lower-cased.
    pub headers: Vec<(String, String)>,
}

impl ApiResponse {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }
}

pub trait AgentApi {
    fn send(&mut self, request: ApiRequest);
    /// Responses that arrived since the last call, in arrival order.
    fn poll(&mut self) -> Vec<ApiResponse>;
    /// An identifier unique across reloads and tabs (e.g. a random UUID).
    fn new_id(&mut self) -> String;
    /// Identifies this editor session to the service.
    fn session_id(&self) -> String;
    /// Small persistent values that survive a reload (browser storage). Default: none.
    fn load(&self, _key: &str) -> Option<String> {
        None
    }
    /// `None` removes the value.
    fn store(&mut self, _key: &str, _value: Option<&str>) {}
}

/// Percent-encodes a query-string component (RFC 3986 unreserved characters pass through).
pub fn query_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[derive(Default)]
pub struct ScriptedLog {
    pub sent: Vec<ApiRequest>,
    pub ready: Vec<ApiResponse>,
    /// `(method, path prefix, status, body)`; the first match answers immediately.
    pub routes: Vec<(Method, String, u16, Json)>,
    pub storage: BTreeMap<String, String>,
    next_id: u64,
}

/// Deterministic in-memory service for tests and offscreen renders. Requests matching a route
/// are answered at once; others wait for [`ScriptedApi::reply`].
#[derive(Clone, Default)]
pub struct ScriptedApi {
    pub log: Rc<RefCell<ScriptedLog>>,
}

impl ScriptedApi {
    pub fn route(&self, method: Method, prefix: &str, status: u16, body: Json) {
        self.log.borrow_mut().routes.push((method, prefix.to_string(), status, body));
    }

    pub fn reply(&self, ticket: u64, status: u16, body: &Json) {
        let body = if body.is_null() { Vec::new() } else { body.to_string().into_bytes() };
        self.log.borrow_mut().ready.push(ApiResponse { ticket, status, body, headers: Vec::new() });
    }

    pub fn reply_raw(&self, ticket: u64, status: u16, body: Vec<u8>, headers: &[(&str, &str)]) {
        let headers = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        self.log.borrow_mut().ready.push(ApiResponse { ticket, status, body, headers });
    }

    pub fn sent(&self) -> Vec<ApiRequest> {
        self.log.borrow().sent.clone()
    }

    pub fn last(&self) -> Option<ApiRequest> {
        self.log.borrow().sent.last().cloned()
    }
}

impl AgentApi for ScriptedApi {
    fn send(&mut self, request: ApiRequest) {
        let mut log = self.log.borrow_mut();
        let answer = log.routes.iter().find(|(method, prefix, ..)| *method == request.method && request.path.starts_with(prefix.as_str())).cloned();
        if let Some((_, _, status, body)) = answer {
            log.ready.push(ApiResponse { ticket: request.ticket, status, body: body.to_string().into_bytes(), headers: Vec::new() });
        }
        log.sent.push(request);
    }

    fn poll(&mut self) -> Vec<ApiResponse> {
        std::mem::take(&mut self.log.borrow_mut().ready)
    }

    fn new_id(&mut self) -> String {
        let mut log = self.log.borrow_mut();
        log.next_id = log.next_id.saturating_add(1);
        format!("00000000-0000-4000-8000-{:012}", log.next_id)
    }

    fn session_id(&self) -> String {
        "scripted-session".into()
    }

    fn load(&self, key: &str) -> Option<String> {
        self.log.borrow().storage.get(key).cloned()
    }

    fn store(&mut self, key: &str, value: Option<&str>) {
        let mut log = self.log.borrow_mut();
        match value {
            Some(value) => log.storage.insert(key.to_string(), value.to_string()),
            None => log.storage.remove(key),
        };
    }
}
