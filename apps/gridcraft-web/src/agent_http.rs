//! Same-origin browser fetch for the connected agent service (`gridcraft_ui_egui::agent_api`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use gridcraft_ui_egui::agent_api::{AgentApi, ApiRequest, ApiResponse, MAX_RESPONSE_BYTES, Method};

const SESSION_KEY: &str = "gridcraft.agent.session";
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::JsFuture;

pub struct HttpAgentApi {
    inbox: Arc<Mutex<Vec<ApiResponse>>>,
    ctx: egui::Context,
    session: String,
    fallback: u64,
    recorder: Rc<RefCell<Option<Recorder>>>,
    audio: Option<(web_sys::HtmlAudioElement, String)>,
}

/// One microphone capture. Closures live as long as the recorder.
struct Recorder {
    recorder: web_sys::MediaRecorder,
    cancelled: Rc<Cell<bool>>,
    _on_data: Closure<dyn FnMut(web_sys::BlobEvent)>,
    _on_stop: Closure<dyn FnMut()>,
}

impl HttpAgentApi {
    pub fn new(ctx: egui::Context) -> Self {
        let mut api = Self { inbox: Arc::default(), ctx, session: String::new(), fallback: 0, recorder: Rc::default(), audio: None };
        api.session = api.load(SESSION_KEY).unwrap_or_else(|| format!("web-{}", random_id().unwrap_or_else(|| "session".into())));
        let session = api.session.clone();
        api.store(SESSION_KEY, Some(&session));
        api
    }
}

const PREFERRED_AUDIO: &[&str] = &["audio/webm;codecs=opus", "audio/ogg;codecs=opus", "audio/mp4"];

/// Record until stopped; on stop deliver the audio (unless cancelled) and release the microphone.
fn start(
    ticket: u64,
    stream: web_sys::MediaStream,
    inbox: Arc<Mutex<Vec<ApiResponse>>>,
    ctx: egui::Context,
    slot: Rc<RefCell<Option<Recorder>>>,
) -> Option<Recorder> {
    let options = web_sys::MediaRecorderOptions::new();
    if let Some(mime) = PREFERRED_AUDIO.iter().find(|mime| web_sys::MediaRecorder::is_type_supported(mime)) {
        options.set_mime_type(mime);
    }
    let recorder = web_sys::MediaRecorder::new_with_media_stream_and_media_recorder_options(&stream, &options).ok()?;
    let chunks: Rc<RefCell<Vec<web_sys::Blob>>> = Rc::default();
    let cancelled = Rc::new(Cell::new(false));
    let data_chunks = chunks.clone();
    let on_data = Closure::<dyn FnMut(web_sys::BlobEvent)>::new(move |event: web_sys::BlobEvent| {
        if let Some(blob) = event.data() {
            data_chunks.borrow_mut().push(blob);
        }
    });
    let stop_recorder = recorder.clone();
    let stop_cancelled = cancelled.clone();
    let on_stop = Closure::<dyn FnMut()>::new(move || {
        let tracks = stream.get_tracks();
        for track in tracks.iter() {
            if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
                track.stop();
            }
        }
        let mime = stop_recorder.mime_type().replace(' ', "").to_ascii_lowercase();
        let parts = js_sys::Array::new();
        for blob in chunks.borrow_mut().drain(..) {
            parts.push(&blob);
        }
        let cancelled = stop_cancelled.get();
        let (inbox, ctx, slot) = (inbox.clone(), ctx.clone(), slot.clone());
        wasm_bindgen_futures::spawn_local(async move {
            // Drop the recorder (and these closures) only after this callback returned.
            slot.borrow_mut().take();
            if cancelled {
                return;
            }
            let bytes = match web_sys::Blob::new_with_blob_sequence(&parts) {
                Ok(blob) => JsFuture::from(blob.array_buffer()).await.ok().map(|buffer| js_sys::Uint8Array::new(&buffer).to_vec()),
                Err(_) => None,
            };
            let response = match bytes.filter(|b| !b.is_empty() && b.len() <= MAX_RESPONSE_BYTES) {
                Some(body) => ApiResponse { ticket, status: 200, body, headers: vec![("content-type".into(), mime)] },
                None => ApiResponse { ticket, status: 0, body: Vec::new(), headers: Vec::new() },
            };
            inbox.lock().unwrap_or_else(PoisonError::into_inner).push(response);
            ctx.request_repaint();
        });
    });
    recorder.set_ondataavailable(Some(on_data.as_ref().unchecked_ref()));
    recorder.set_onstop(Some(on_stop.as_ref().unchecked_ref()));
    recorder.start().ok()?;
    Some(Recorder { recorder, cancelled, _on_data: on_data, _on_stop: on_stop })
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

    fn start_recording(&mut self, ticket: u64) -> Result<(), String> {
        if self.recorder.borrow().is_some() {
            return Err("A recording is already in progress.".into());
        }
        let devices = web_sys::window().and_then(|w| w.navigator().media_devices().ok()).ok_or("This browser has no microphone access.")?;
        let constraints = web_sys::MediaStreamConstraints::new();
        constraints.set_audio(&JsValue::TRUE);
        let promise = devices.get_user_media_with_constraints(&constraints).map_err(|_| "This browser has no microphone access.")?;
        let (inbox, ctx, slot) = (self.inbox.clone(), self.ctx.clone(), self.recorder.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let fail = |inbox: &Arc<Mutex<Vec<ApiResponse>>>| {
                inbox.lock().unwrap_or_else(PoisonError::into_inner).push(ApiResponse { ticket, status: 0, body: Vec::new(), headers: Vec::new() });
                ctx.request_repaint();
            };
            let Some(stream) = JsFuture::from(promise).await.ok().and_then(|s| s.dyn_into::<web_sys::MediaStream>().ok()) else {
                return fail(&inbox);
            };
            match start(ticket, stream, inbox.clone(), ctx.clone(), slot.clone()) {
                Some(recorder) => *slot.borrow_mut() = Some(recorder),
                None => fail(&inbox),
            }
        });
        Ok(())
    }

    fn stop_recording(&mut self) {
        if let Some(recorder) = self.recorder.borrow().as_ref() {
            let _ = recorder.recorder.stop();
        }
    }

    fn cancel_recording(&mut self) {
        if let Some(recorder) = self.recorder.borrow().as_ref() {
            recorder.cancelled.set(true);
            let _ = recorder.recorder.stop();
        }
    }

    fn play(&mut self, audio: Vec<u8>) -> Result<(), String> {
        self.stop_playback();
        let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(audio.as_slice()));
        let options = web_sys::BlobPropertyBag::new();
        options.set_type("audio/mpeg");
        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options).map_err(|_| "Couldn't prepare the audio.")?;
        let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(|_| "Couldn't prepare the audio.")?;
        let element = web_sys::HtmlAudioElement::new_with_src(&url).map_err(|_| "Audio playback is unavailable.")?;
        let _ = element.play();
        self.audio = Some((element, url));
        Ok(())
    }

    fn stop_playback(&mut self) {
        if let Some((element, url)) = self.audio.take() {
            let _ = element.pause();
            let _ = web_sys::Url::revoke_object_url(&url);
        }
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
    if let Some(raw) = &request.raw {
        headers.set("content-type", &raw.mime).ok()?;
        init.set_body(&js_sys::Uint8Array::from(raw.bytes.as_slice()));
    } else if let Some(body) = &request.body {
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
