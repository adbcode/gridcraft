//! Same-origin browser fetch for the connected agent service (`gridcraft_ui_egui::agent_api`).

use std::sync::{Arc, Mutex, PoisonError};

use gridcraft_ui_egui::agent_api::{AgentApi, ApiRequest, ApiResponse, MAX_RESPONSE_BYTES, Method};

const SESSION_KEY: &str = "gridcraft.agent.session";
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
        let mut api = Self { inbox: Arc::default(), ctx, session: String::new(), fallback: 0 };
        api.session = api.load(SESSION_KEY).unwrap_or_else(|| format!("web-{}", random_id().unwrap_or_else(|| "session".into())));
        let session = api.session.clone();
        api.store(SESSION_KEY, Some(&session));
        api
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn random_id() -> Option<String> {
    Some(web_sys::window()?.crypto().ok()?.random_uuid())
}

impl AgentApi for HttpAgentApi {
    fn send(&mut self, request: ApiRequest) {
        let inbox = self.inbox.clone();
        let ctx = self.ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let (status, body, headers) = fetch(&request).await.unwrap_or((0, Vec::new(), Vec::new()));
            inbox.lock().unwrap_or_else(PoisonError::into_inner).push(ApiResponse { ticket: request.ticket, status, body, headers });
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

    fn load(&self, key: &str) -> Option<String> {
        storage()?.get_item(key).ok()?
    }

    fn store(&mut self, key: &str, value: Option<&str>) {
        if let Some(storage) = storage() {
            let _ = match value {
                Some(value) => storage.set_item(key, value),
                None => storage.remove_item(key),
            };
        }
    }
}

/// `None` is a network-level failure: the caller reports status 0 and may retry.
const HEADERS: &[&str] = &["x-gridcraft-storage-version", "x-gridcraft-sha256"];

type Fetched = (u16, Vec<u8>, Vec<(String, String)>);

async fn fetch(request: &ApiRequest) -> Option<Fetched> {
    if !request.path.starts_with("/api/") {
        return None;
    }
    let window = web_sys::window()?;
    let init = web_sys::RequestInit::new();
    init.set_method(match request.method {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
    });
    let headers = web_sys::Headers::new().ok()?;
    headers.set("accept", "application/json").ok()?;
    if let Some(body) = &request.body {
        headers.set("content-type", "application/json").ok()?;
        init.set_body(&JsValue::from_str(&body.to_string()));
    } else if let Some(upload) = &request.upload {
        let form = web_sys::FormData::new().ok()?;
        form.append_with_str("metadata", &upload.metadata).ok()?;
        let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(upload.bytes.as_slice()));
        let options = web_sys::BlobPropertyBag::new();
        options.set_type("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet");
        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options).ok()?;
        form.append_with_blob_and_filename("file", &blob, &upload.file_name).ok()?;
        init.set_body(&form);
    }
    init.set_headers(&headers);
    let fetch_request = web_sys::Request::new_with_str_and_init(&request.path, &init).ok()?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_request(&fetch_request)).await.ok()?.dyn_into().ok()?;
    let status = response.status();
    let headers = HEADERS.iter().filter_map(|name| response.headers().get(name).ok().flatten().map(|value| (name.to_string(), value))).collect();
    let buffer = JsFuture::from(response.array_buffer().ok()?).await.ok()?;
    let bytes = js_sys::Uint8Array::new(&buffer);
    if usize::try_from(bytes.length()).ok()? > MAX_RESPONSE_BYTES {
        return None;
    }
    Some((status, bytes.to_vec(), headers))
}
