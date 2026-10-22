//! Same-origin browser fetch for the connected agent service (`gridcraft_ui_egui::agent_api`).

use std::sync::{Arc, Mutex, PoisonError};

use gridcraft_ui_egui::agent_api::{AgentApi, ApiRequest, ApiResponse, MAX_RESPONSE_BYTES, Method};
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::JsFuture;

pub struct HttpAgentApi {
    inbox: Arc<Mutex<Vec<ApiResponse>>>,
    ctx: egui::Context,
    session: String,
    fallback: u64,
}

impl HttpAgentApi {
    pub fn new(ctx: egui::Context) -> Self {
        let session = format!("web-{}", random_id().unwrap_or_else(|| "session".into()));
        Self { inbox: Arc::default(), ctx, session, fallback: 0 }
    }
}

fn random_id() -> Option<String> {
    Some(web_sys::window()?.crypto().ok()?.random_uuid())
}

impl AgentApi for HttpAgentApi {
    fn send(&mut self, request: ApiRequest) {
        let inbox = self.inbox.clone();
        let ctx = self.ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let (status, body) = fetch(&request).await.unwrap_or((0, Vec::new()));
            inbox.lock().unwrap_or_else(PoisonError::into_inner).push(ApiResponse { ticket: request.ticket, status, body });
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) -> Vec<ApiResponse> {
        std::mem::take(&mut *self.inbox.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn new_id(&mut self) -> String {
        random_id().unwrap_or_else(|| {
            self.fallback = self.fallback.saturating_add(1);
            format!("{}-{}", self.session, self.fallback)
        })
    }

    fn session_id(&self) -> String {
        self.session.clone()
    }
}

/// `None` is a network-level failure: the caller reports status 0 and may retry.
async fn fetch(request: &ApiRequest) -> Option<(u16, Vec<u8>)> {
    if !request.path.starts_with("/api/") {
        return None;
    }
    let window = web_sys::window()?;
    let init = web_sys::RequestInit::new();
    init.set_method(match request.method {
        Method::Get => "GET",
        Method::Post => "POST",
    });
    let headers = web_sys::Headers::new().ok()?;
    headers.set("accept", "application/json").ok()?;
    if let Some(body) = &request.body {
        headers.set("content-type", "application/json").ok()?;
        init.set_body(&JsValue::from_str(&body.to_string()));
    }
    init.set_headers(&headers);
    let fetch_request = web_sys::Request::new_with_str_and_init(&request.path, &init).ok()?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_request(&fetch_request)).await.ok()?.dyn_into().ok()?;
    let status = response.status();
    let buffer = JsFuture::from(response.array_buffer().ok()?).await.ok()?;
    let bytes = js_sys::Uint8Array::new(&buffer);
    if usize::try_from(bytes.length()).ok()? > MAX_RESPONSE_BYTES {
        return None;
    }
    Some((status, bytes.to_vec()))
}
