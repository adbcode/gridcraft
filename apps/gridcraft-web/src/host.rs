//! Version 2, same-origin parent bridge. See `docs/agentic/bridge-protocol.md`.
//! Work is queued by postMessage and applied after UI input in one synchronous frame.
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use gridcraft_engine::agent_bridge::{BridgeState, Identity};
use gridcraft_ui_egui::{Services, SheetApp};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};

const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_JSON_BYTES: usize = 256 * 1024;
const MAX_QUEUE: usize = 8;
const MAX_REQUESTS: usize = 128;
const MAX_EXPORTS: usize = 4;

#[derive(Clone)]
pub struct Bridge {
    window: web_sys::Window,
    parent: web_sys::Window,
    origin: String,
    session_id: String,
    queue: Rc<RefCell<VecDeque<Incoming>>>,
    state: Rc<RefCell<State>>,
    reported: Rc<Cell<Option<bool>>>,
}

struct Incoming {
    id: String,
    method: String,
    params: Json,
    bytes: Option<Vec<u8>>,
}

#[derive(Clone)]
struct Reply {
    fingerprint: Vec<u8>,
    message: Json,
    bytes: Option<Vec<u8>>,
}

#[derive(Default)]
struct State {
    adapter: BridgeState,
    opened: bool,
    ledger: HashMap<String, Reply>,
    exports: usize,
    next_save: u64,
}

impl Bridge {
    pub fn new() -> Option<Self> {
        let window = web_sys::window()?;
        let parent = window.parent().ok().flatten()?;
        if js_sys::Object::is(&parent, &window) {
            return None;
        }
        let origin = window.location().origin().ok()?;
        let session_id = window.crypto().ok()?.random_uuid();
        Some(Self { window, parent, origin, session_id, queue: Rc::default(), state: Rc::default(), reported: Rc::default() })
    }

    pub fn connect(&self, services: &mut Services, ctx: egui::Context) {
        self.listen(ctx);
        let bridge = self.clone();
        services.download = Some(Box::new(move |name, bytes| {
            // Non-workbook exports never change saved state.
            if !bridge.send(&json!({"type":"gridcraft:export", "session_id":bridge.session_id, "name":name}), Some(bytes)) {
                log::error!("could not deliver export to embedding page");
            }
        }));
        let bridge = self.clone();
        services.download_workbook = Some(Box::new(move |doc, name, bytes| {
            if bytes.len() > MAX_FILE_BYTES {
                return Err("host snapshot exceeds 16 MiB".into());
            }
            let mut state = bridge.state.borrow_mut();
            state.next_save = state.next_save.checked_add(1).ok_or("snapshot identifier limit reached")?;
            let snapshot_id = format!("ui-{}", state.next_save);
            let captured = state.adapter.capture_document_snapshot(doc, &snapshot_id).map_err(|e| e.to_string())?;
            let message = json!({
                "type":"gridcraft:save", "session_id":bridge.session_id,
                "snapshot_id":snapshot_id, "name":name, "identity":captured.identity,
            });
            if !bridge.send(&message, Some(bytes)) {
                let _ = state.adapter.discard_snapshot(&snapshot_id);
                return Err("could not deliver snapshot; workbook remains unsaved".into());
            }
            Ok(())
        }));
    }

    pub fn set_author(app: &mut SheetApp, author: &str) {
        let author: String = author.trim().chars().take(200).collect();
        if !author.is_empty() {
            app.session.prefs.user_name = author;
        }
    }

    pub fn post_ready(&self) {
        self.send(
            &json!({
                "type":"gridcraft:ready", "protocol":2, "session_id":self.session_id,
                "version":env!("CARGO_PKG_VERSION"),
                "capabilities":["context", "inspect", "set_values", "undo", "snapshot.export", "snapshot.ack"],
            }),
            None,
        );
    }

    pub fn post_failed(&self, error: &str) {
        self.send(&json!({"type":"gridcraft:failed", "session_id":self.session_id, "error":error}), None);
    }

    /// Run after UI input: there is no await between conflict checking and mutation/readback.
    pub fn sync(&self, app: &mut SheetApp, ctx: &egui::Context) {
        let mut state = self.state.borrow_mut();
        let _ = state.adapter.observe(&app.session);
        let requests = std::mem::take(&mut *self.queue.borrow_mut());
        if !requests.is_empty() {
            // UI painting preceded these edits. Paint their results in the next frame.
            ctx.request_repaint();
        }
        for request in requests {
            let reply = self.process(&mut state, app, request);
            self.send(&reply.message, reply.bytes.as_deref());
        }
        let dirty = app.session.documents().iter().any(|doc| doc.is_dirty());
        if self.reported.get() != Some(dirty) && self.send(&json!({"type":"gridcraft:dirty", "session_id":self.session_id, "dirty":dirty}), None) {
            self.reported.set(Some(dirty));
        }
    }

    fn process(&self, state: &mut State, app: &mut SheetApp, request: Incoming) -> Reply {
        let mut digest = Sha256::new();
        digest.update(request.method.as_bytes());
        digest.update(request.params.to_string().as_bytes());
        if let Some(bytes) = &request.bytes {
            digest.update(bytes);
        }
        let fingerprint = digest.finalize().to_vec();
        if let Some(previous) = state.ledger.get(&request.id) {
            return if previous.fingerprint == fingerprint {
                previous.clone()
            } else {
                self.error(&request, "duplicate_id", "request id already belongs to another operation", fingerprint)
            };
        }
        let stateful = !matches!(request.method.as_str(), "context" | "inspect");
        if stateful && state.ledger.len() >= MAX_REQUESTS {
            return self.error(&request, "session_limit", "request ledger full; save and start a new session", fingerprint);
        }
        let result = self.execute(state, app, &request);
        let reply = match result {
            Ok((result, bytes)) => Reply {
                fingerprint,
                message: json!({
                    "type":if request.method == "open" { "gridcraft:opened" } else { "gridcraft:response" },
                    "id":request.id, "session_id":self.session_id, "ok":true, "result":result,
                }),
                bytes,
            },
            Err((code, error)) => self.error(&request, &code, &error, fingerprint),
        };
        if stateful {
            state.ledger.insert(request.id, reply.clone());
        }
        reply
    }

    fn execute(&self, state: &mut State, app: &mut SheetApp, request: &Incoming) -> Result<(Json, Option<Vec<u8>>), (String, String)> {
        let fail = |code: &str, message: &str| (code.to_owned(), message.to_owned());
        if app.host_busy() && request.method != "context" && request.method != "snapshot.ack" {
            return Err(fail("busy", "finish or cancel the current edit or dialog, then inspect again"));
        }
        if request.method == "open" {
            let bytes = request.bytes.as_ref().ok_or_else(|| fail("invalid_request", "open requires bytes"))?;
            let name = request.params.get("name").and_then(Json::as_str).ok_or_else(|| fail("invalid_request", "open requires a name"))?;
            let opened = app
                .session
                .execute_headless("file.open", json!({"name":name, "base64":gridcraft_engine::io::base64_encode(bytes)}))
                .map_err(|e| fail("open_failed", &e.to_string()))?;
            app.after_open(None, &opened);
            app.grid.ensure_visible = true;
            state.opened = true;
            let context = state.adapter.handle(&mut app.session, "context", json!({})).map_err(|e| fail("context_failed", &e.to_string()))?;
            return Ok((json!({"opened":opened, "context":context}), None));
        }
        if !state.opened {
            return Err(fail("not_opened", "wait for the correlated gridcraft:opened response"));
        }
        if request.method == "snapshot.export" {
            if state.exports >= MAX_EXPORTS {
                return Err(fail("snapshot_limit", "snapshot export replay cache full; start a new session after saving"));
            }
            let expected: Identity = serde_json::from_value(request.params.get("expected").cloned().unwrap_or(Json::Null))
                .map_err(|e| fail("invalid_request", &e.to_string()))?;
            let hash: String = Sha256::digest(request.id.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
            let snapshot_id = format!("request-{hash}");
            let captured = state.adapter.capture_snapshot(&app.session, &snapshot_id, &expected).map_err(|e| (e.code.to_owned(), e.message))?;
            let bytes = match gridcraft_engine::io::save_bytes(&captured.workbook, "Book.xlsx") {
                Ok(bytes) => bytes,
                Err(error) => {
                    let _ = state.adapter.discard_snapshot(&snapshot_id);
                    return Err(fail("export_failed", &error.to_string()));
                }
            };
            if bytes.len() > MAX_FILE_BYTES {
                let _ = state.adapter.discard_snapshot(&snapshot_id);
                return Err(fail("size_limit", "host snapshot exceeds 16 MiB"));
            }
            state.exports += 1;
            return Ok((json!({"snapshot_id":snapshot_id, "identity":captured.identity, "name":"Book.xlsx"}), Some(bytes)));
        }
        if request.method == "snapshot.ack" {
            let id = request.params.get("snapshot_id").and_then(Json::as_str).ok_or_else(|| fail("invalid_request", "snapshot_id required"))?;
            // A durable database version is opaque to GridCraft and distinct from its token.
            let version = request
                .params
                .get("storage_version")
                .and_then(Json::as_str)
                .filter(|s| !s.is_empty() && s.len() <= 128)
                .ok_or_else(|| fail("invalid_request", "ack requires a durable storage_version"))?;
            let ack = state.adapter.acknowledge_snapshot(&mut app.session, id).map_err(|e| (e.code.to_owned(), e.message))?;
            return Ok((json!({"snapshot_id":id, "storage_version":version, "ack":ack}), None));
        }
        let mut params = request.params.clone();
        if matches!(request.method.as_str(), "set_values" | "undo") {
            let object = params.as_object_mut().ok_or_else(|| fail("invalid_request", "params must be an object"))?;
            object.insert("request_id".into(), Json::String(request.id.clone()));
        }
        let mut result = state.adapter.handle(&mut app.session, &request.method, params).map_err(|e| (e.code.to_owned(), e.message))?;
        if request.method == "context"
            && let Some(object) = result.as_object_mut()
        {
            object.insert("busy".into(), Json::Bool(app.host_busy()));
        }
        app.grid.ensure_visible = true;
        Ok((result, None))
    }

    fn error(&self, request: &Incoming, code: &str, error: &str, fingerprint: Vec<u8>) -> Reply {
        Reply {
            fingerprint,
            bytes: None,
            message: json!({
                "type":if request.method == "open" {"gridcraft:open-error"} else {"gridcraft:response"},
                "id":request.id, "session_id":self.session_id, "ok":false,
                "error":{"code":code, "message":error},
            }),
        }
    }

    fn listen(&self, ctx: egui::Context) {
        let bridge = self.clone();
        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            if event.origin() != bridge.origin || !event.source().is_some_and(|source| js_sys::Object::is(&source, &bridge.parent)) {
                return;
            }
            let data = event.data();
            let kind = string(&data, "type").unwrap_or_default();
            if kind == "gridcraft:hello" {
                bridge.post_ready();
                return;
            }
            if !matches!(kind.as_str(), "gridcraft:open" | "gridcraft:request") {
                return;
            }
            let id = string(&data, "id").filter(|id| !id.is_empty() && id.len() <= 128).unwrap_or_default();
            let parsed = (|| {
                if id.is_empty() {
                    return Err("non-empty id of at most 128 bytes required");
                }
                if string(&data, "session_id").as_deref() != Some(&bridge.session_id) {
                    return Err("session_id does not match this mounted workbook");
                }
                if bridge.queue.borrow().len() >= MAX_QUEUE {
                    return Err("host request queue is full");
                }
                if kind == "gridcraft:open" {
                    let name = string(&data, "name")
                        .filter(|n| !n.trim().is_empty() && n.len() <= 255)
                        .ok_or("open requires a name of at most 255 bytes")?;
                    let bytes = read_bytes(&data).ok_or("bytes must be an ArrayBuffer or Uint8Array, at most 16 MiB")?;
                    Ok(Incoming { id: id.clone(), method: "open".into(), params: json!({"name":name}), bytes: Some(bytes) })
                } else {
                    let method = string(&data, "method").filter(|m| m.len() <= 64).ok_or("method required")?;
                    let value = js_sys::Reflect::get(&data, &"params".into()).map_err(|_| "params required")?;
                    let text = js_sys::JSON::stringify(&value)
                        .ok()
                        .and_then(|s| s.as_string())
                        .filter(|s| s.len() <= MAX_JSON_BYTES)
                        .ok_or("params exceed limit or are not JSON")?;
                    let params: Json = serde_json::from_str(&text).map_err(|_| "invalid params")?;
                    if !params.is_object() {
                        return Err("params must be an object");
                    }
                    Ok(Incoming { id: id.clone(), method, params, bytes: None })
                }
            })();
            match parsed {
                Ok(request) => {
                    bridge.queue.borrow_mut().push_back(request);
                    ctx.request_repaint();
                }
                Err(error) => {
                    bridge.send(
                        &json!({"type":if kind == "gridcraft:open" {"gridcraft:open-error"} else {"gridcraft:response"},
                        "id":id, "session_id":bridge.session_id, "ok":false, "error":{"code":"invalid_request","message":error}}),
                        None,
                    );
                }
            }
        });
        if self.window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref()).is_ok() {
            on_message.forget();
        }
    }

    fn send(&self, value: &Json, bytes: Option<&[u8]>) -> bool {
        let Ok(message) = js_sys::JSON::parse(&value.to_string()) else {
            return false;
        };
        if let Some(bytes) = bytes {
            let buffer = js_sys::Uint8Array::from(bytes).buffer();
            if js_sys::Reflect::set(&message, &"bytes".into(), &buffer).is_err() {
                return false;
            }
            self.parent.post_message_with_transfer(&message, &self.origin, &js_sys::Array::of1(&buffer)).is_ok()
        } else {
            self.parent.post_message(&message, &self.origin).is_ok()
        }
    }
}

fn string(data: &JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(data, &key.into()).ok()?.as_string()
}

fn read_bytes(data: &JsValue) -> Option<Vec<u8>> {
    let bytes = js_sys::Reflect::get(data, &"bytes".into()).ok()?;
    let array = if let Some(array) = bytes.dyn_ref::<js_sys::Uint8Array>() {
        array.clone()
    } else if bytes.is_instance_of::<js_sys::ArrayBuffer>() {
        js_sys::Uint8Array::new(&bytes)
    } else {
        return None;
    };
    if array.length() as usize > MAX_FILE_BYTES {
        return None;
    }
    Some(array.to_vec())
}
