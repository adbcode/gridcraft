//! Transport-neutral boundary to the connected agent service. Hosts perform same-origin HTTP
//! (the web shell uses browser fetch); this crate never depends on a browser or HTTP library.
//! Responses arrive asynchronously and are drained once per frame.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::Value as Json;

/// Largest response body a host should deliver; larger bodies are reported as failures.
pub const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// `path` is always a same-origin path beginning with `/api/`.
#[derive(Clone, Debug, PartialEq)]
pub struct ApiRequest {
    pub ticket: u64,
    pub method: Method,
    pub path: String,
    pub body: Option<Json>,
}

/// `status == 0` means no HTTP response arrived (network failure or an oversized body).
#[derive(Clone, Debug, PartialEq)]
pub struct ApiResponse {
    pub ticket: u64,
    pub status: u16,
    pub body: Vec<u8>,
}

impl ApiResponse {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
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
        self.log.borrow_mut().ready.push(ApiResponse { ticket, status, body });
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
            log.ready.push(ApiResponse { ticket: request.ticket, status, body: body.to_string().into_bytes() });
        }
        log.sent.push(request);
    }

    fn poll(&mut self) -> Vec<ApiResponse> {
        std::mem::take(&mut self.log.borrow_mut().ready)
    }

    fn new_id(&mut self) -> String {
        let mut log = self.log.borrow_mut();
        log.next_id = log.next_id.saturating_add(1);
        format!("scripted-{}", log.next_id)
    }

    fn session_id(&self) -> String {
        "scripted-session".into()
    }
}
