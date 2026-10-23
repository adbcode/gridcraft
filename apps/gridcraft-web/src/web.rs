use std::cell::RefCell;
use std::rc::Rc;

use gridcraft_core::{CellRef, RangeRef};
use gridcraft_web::{MAX_FILE_BYTES, MAX_JSON_BYTES, Reply, Request, Runtime};
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Document, Element, HtmlAnchorElement, HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement, MessageEvent, Window};

struct App {
    runtime: Runtime,
    document: Document,
    window: Window,
    parent: Option<Window>,
    origin: String,
    session_id: String,
    counter: u64,
}

type Shared = Rc<RefCell<App>>;

pub fn start() {
    if let Err(error) = setup() {
        if let Some(document) = web_sys::window().and_then(|w| w.document()) {
            if let Some(status) = document.get_element_by_id("status") {
                status.set_text_content(Some(&format!("Could not start: {error:?}")));
                status.set_class_name("error");
            }
        }
    }
}

fn setup() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or("window unavailable")?;
    let document = window.document().ok_or("document unavailable")?;
    let parent = window.parent()?.filter(|p| !js_sys::Object::is(p, &window));
    let app = Rc::new(RefCell::new(App {
        runtime: Runtime::default(),
        origin: window.location().origin()?,
        session_id: window.crypto()?.random_uuid(),
        document,
        window,
        parent,
        counter: 0,
    }));
    bind(&app, "execute", "click", |app| {
        let command = app.input("command")?;
        let input: HtmlTextAreaElement = app.element("params")?.dyn_into().map_err(js_error)?;
        if input.value().len() > MAX_JSON_BYTES {
            return Err("parameters exceed 64 KiB".into());
        }
        let params: Value = serde_json::from_str(&input.value()).map_err(|e| format!("Invalid JSON: {e}"))?;
        app.execute(&command, params)
    })?;
    bind(&app, "commands", "click", |app| {
        let request = app.request("commands", json!({}))?;
        let reply = app.runtime.request(request, None)?;
        let list = app.element("command-list")?;
        list.set_text_content(None);
        for command in reply.result.as_array().into_iter().flatten() {
            let option = app.document.create_element("option").map_err(js_error)?;
            option.set_attribute("value", command["id"].as_str().unwrap_or_default()).map_err(js_error)?;
            option.set_text_content(command["params"].as_str());
            list.append_child(&option).map_err(js_error)?;
        }
        app.result(&reply.result);
        Ok(())
    })?;
    bind(&app, "refresh", "click", |app| app.render())?;
    bind(&app, "sheet", "change", |app| {
        let select: HtmlSelectElement = app.element("sheet")?.dyn_into().map_err(js_error)?;
        let index = select.value().parse::<usize>().map_err(|e| e.to_string())?;
        app.execute("sheet.activate", json!({"sheet":index}))
    })?;
    bind(&app, "download", "click", |app| {
        let bytes = app.runtime.download()?;
        download(&app.document, &bytes).map_err(js_error)?;
        app.status("Download started. The browser cannot confirm that it was saved; dirty state is unchanged.", false);
        Ok(())
    })?;
    bind_file(&app)?;
    bind_host(&app)?;
    let shared = app.clone();
    let unload = Closure::<dyn FnMut(web_sys::BeforeUnloadEvent)>::new(move |event: web_sys::BeforeUnloadEvent| {
        if shared.borrow().runtime.session.active().is_some_and(|d| d.is_dirty()) {
            event.prevent_default();
            event.set_return_value("");
        }
    });
    app.borrow().window.add_event_listener_with_callback("beforeunload", unload.as_ref().unchecked_ref())?;
    unload.forget();
    {
        let mut app = app.borrow_mut();
        app.render().map_err(|e| JsValue::from_str(&e))?;
        for id in ["file", "download", "execute", "commands", "refresh", "sheet"] {
            app.element(id).map_err(|e| JsValue::from_str(&e))?.remove_attribute("disabled")?;
        }
        app.status("Engine ready. Open a file or run a command against the blank workbook.", false);
        app.ready();
    }
    Ok(())
}

fn js_error(error: impl Into<JsValue>) -> String {
    let error = error.into();
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}

fn bind(app: &Shared, id: &str, event: &str, mut f: impl FnMut(&mut App) -> Result<(), String> + 'static) -> Result<(), JsValue> {
    let element = app.borrow().element(id).map_err(|e| JsValue::from_str(&e))?;
    let shared = app.clone();
    let callback = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
        let mut app = shared.borrow_mut();
        if let Err(error) = f(&mut app) {
            app.status(&error, true);
        }
    });
    element.add_event_listener_with_callback(event, callback.as_ref().unchecked_ref())?;
    callback.forget();
    Ok(())
}

impl App {
    fn element(&self, id: &str) -> Result<Element, String> {
        self.document.get_element_by_id(id).ok_or_else(|| format!("missing {id}"))
    }

    fn input(&self, id: &str) -> Result<String, String> {
        Ok(self.element(id)?.dyn_into::<HtmlInputElement>().map_err(js_error)?.value())
    }

    fn status(&self, message: &str, error: bool) {
        if let Ok(status) = self.element("status") {
            status.set_text_content(Some(message));
            status.set_class_name(if error { "error" } else { "" });
        }
    }

    fn result(&self, value: &Value) {
        if let Ok(element) = self.element("result") {
            let text = serde_json::to_string_pretty(value).unwrap_or_default();
            let mut limited: String = text.chars().take(24_000).collect();
            if text.chars().count() > 24_000 {
                limited.push_str("\n… response truncated in this preview");
            }
            element.set_text_content(Some(&limited));
        }
    }

    fn request(&mut self, method: &str, mut params: Value) -> Result<Request, String> {
        self.counter = self.counter.checked_add(1).ok_or("request counter exhausted")?;
        if !matches!(method, "commands" | "context" | "snapshot.ack") {
            params["expected"] = self.runtime.context()?["identity"].clone();
        }
        Ok(Request { id: format!("local-{}", self.counter), method: method.into(), params })
    }

    fn execute(&mut self, command: &str, params: Value) -> Result<(), String> {
        let request = self.request("execute", json!({"command":command, "params":params}))?;
        let outcome = self.runtime.request(request, None);
        self.render()?;
        match outcome {
            Ok(reply) => {
                self.result(&reply.result);
                self.status("Command completed. Inspect the result and workbook preview.", false);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn render(&mut self) -> Result<(), String> {
        if let Some(doc) = self.runtime.session.active() {
            self.element("title")?.set_text_content(Some(&doc.title));
            self.element("dirty")?.set_text_content(Some(if doc.is_dirty() { "Unsaved changes" } else { "Unchanged" }));
            let select = self.element("sheet")?;
            select.set_text_content(None);
            for (index, sheet) in doc.wb.sheets.iter().enumerate() {
                let option = self.document.create_element("option").map_err(js_error)?;
                option.set_attribute("value", &index.to_string()).map_err(js_error)?;
                option.set_text_content(Some(&sheet.name));
                if index == doc.sheet_index() {
                    option.set_attribute("selected", "").map_err(js_error)?;
                }
                select.append_child(&option).map_err(js_error)?;
            }
        }
        let text = self.input("range")?;
        let range = RangeRef::parse(&text).ok_or("invalid preview range")?;
        let data = self.runtime.preview(&text)?;
        let table = self.document.create_element("table").map_err(js_error)?;
        table.set_attribute("aria-label", "Workbook values").map_err(js_error)?;
        let head = self.document.create_element("thead").map_err(js_error)?;
        let row = self.document.create_element("tr").map_err(js_error)?;
        self.cell(&row, "th", "")?;
        for col in range.start.col..=range.end.col {
            let label = CellRef::new(0, col).a1();
            self.cell(&row, "th", label.trim_end_matches('1'))?;
        }
        head.append_child(&row).map_err(js_error)?;
        table.append_child(&head).map_err(js_error)?;
        let body = self.document.create_element("tbody").map_err(js_error)?;
        for (offset, values) in data["values"].as_array().into_iter().flatten().enumerate() {
            let row = self.document.create_element("tr").map_err(js_error)?;
            self.cell(&row, "th", &(range.start.row as usize + offset + 1).to_string())?;
            for value in values.as_array().into_iter().flatten() {
                self.cell(&row, "td", &value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string()))?;
            }
            body.append_child(&row).map_err(js_error)?;
        }
        table.append_child(&body).map_err(js_error)?;
        let preview = self.element("preview")?;
        preview.set_text_content(None);
        preview.append_child(&table).map_err(js_error)?;
        Ok(())
    }

    fn cell(&self, row: &Element, tag: &str, text: &str) -> Result<(), String> {
        let cell = self.document.create_element(tag).map_err(js_error)?;
        cell.set_text_content(Some(text));
        row.append_child(&cell).map_err(js_error)?;
        Ok(())
    }

    fn ready(&mut self) {
        let context = self.runtime.context().unwrap_or(Value::Null);
        let _ = self.post(&json!({"type":"gridcraft:ready", "protocol":3, "session_id":self.session_id,
            "capabilities":["context","commands","inspect","execute","set_values","undo","open","snapshot.export","snapshot.ack"], "context":context}), None);
    }

    fn post(&self, message: &Value, bytes: Option<&[u8]>) -> Result<(), JsValue> {
        let Some(parent) = &self.parent else {
            return Ok(());
        };
        let message = js_sys::JSON::parse(&message.to_string())?;
        let transfer = js_sys::Array::new();
        if let Some(bytes) = bytes {
            let buffer = js_sys::Uint8Array::from(bytes).buffer();
            js_sys::Reflect::set(&message, &"bytes".into(), &buffer)?;
            transfer.push(&buffer);
        }
        parent.post_message_with_transfer(&message, &self.origin, &transfer)
    }
}

fn bind_file(app: &Shared) -> Result<(), JsValue> {
    let input: HtmlInputElement = app.borrow().element("file").map_err(|e| JsValue::from_str(&e))?.dyn_into()?;
    let shared = app.clone();
    let callback = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
        let Some(file) = input.files().and_then(|files| files.get(0)) else {
            return;
        };
        input.set_value("");
        if file.size() > MAX_FILE_BYTES as f64 {
            shared.borrow().status("File exceeds the 16 MiB browser import limit.", true);
            return;
        }
        let request = {
            let mut app = shared.borrow_mut();
            let dirty = app.runtime.session.active().is_some_and(|d| d.is_dirty());
            if dirty && !app.window.confirm_with_message("Discard unsaved changes and open another file?").unwrap_or(false) {
                return;
            }
            match app.request("open", json!({"name":file.name(), "discardChanges":dirty})) {
                Ok(request) => request,
                Err(error) => {
                    app.status(&error, true);
                    return;
                }
            }
        };
        let shared = shared.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let loaded = JsFuture::from(file.array_buffer()).await;
            let mut app = shared.borrow_mut();
            match loaded {
                Ok(buffer) => {
                    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
                    match app.runtime.request(request, Some(&bytes)) {
                        Ok(reply) => {
                            app.result(&reply.result);
                            match app.render() {
                                Ok(()) => app.status("File opened. Review import warnings in the response.", false),
                                Err(error) => app.status(&error, true),
                            }
                        }
                        Err(error) => app.status(&error, true),
                    }
                }
                Err(error) => app.status(&js_error(error), true),
            }
        });
    });
    app.borrow().element("file").map_err(|e| JsValue::from_str(&e))?.add_event_listener_with_callback("change", callback.as_ref().unchecked_ref())?;
    callback.forget();
    Ok(())
}

fn bind_host(app: &Shared) -> Result<(), JsValue> {
    let shared = app.clone();
    let callback = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let mut app = shared.borrow_mut();
        if event.origin() != app.origin
            || app.origin == "null"
            || !app.parent.as_ref().is_some_and(|parent| event.source().is_some_and(|source| js_sys::Object::is(&source, parent)))
        {
            return;
        }
        let data = event.data();
        let get = |key: &str| js_sys::Reflect::get(&data, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED);
        if get("type").as_string().as_deref() == Some("gridcraft:hello") {
            app.ready();
            return;
        }
        if get("type").as_string().as_deref() != Some("gridcraft:request") || get("session_id").as_string().as_deref() != Some(&app.session_id) {
            return;
        }
        let id = get("id").as_string().unwrap_or_default();
        if id.starts_with("local-") {
            return;
        }
        let outcome = (|| -> Result<Reply, String> {
            let method = get("method").as_string().ok_or("method required")?;
            let text = js_sys::JSON::stringify(&get("params")).map_err(js_error)?.as_string().ok_or("params required")?;
            if text.len() > MAX_JSON_BYTES {
                return Err("JSON exceeds 64 KiB".into());
            }
            let params = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let binary = get("bytes");
            let bytes = if binary.is_undefined() {
                None
            } else {
                let array = if binary.is_instance_of::<js_sys::ArrayBuffer>() {
                    js_sys::Uint8Array::new(&binary)
                } else {
                    binary.dyn_into::<js_sys::Uint8Array>().map_err(|_| "bytes must be an ArrayBuffer or Uint8Array")?
                };
                if array.length() as usize > MAX_FILE_BYTES {
                    return Err("file exceeds 16 MiB".into());
                }
                Some(array.to_vec())
            };
            app.runtime.request(Request { id: id.clone(), method, params }, bytes.as_deref())
        })();
        let _ = app.render();
        let (message, bytes) = match outcome {
            Ok(reply) => (json!({"type":"gridcraft:response", "session_id":app.session_id, "id":id, "ok":true, "result":reply.result}), reply.bytes),
            Err(error) => (json!({"type":"gridcraft:response", "session_id":app.session_id, "id":id, "ok":false, "error":error}), None),
        };
        if let Err(error) = app.post(&message, bytes.as_deref()) {
            app.status(&format!("Host response failed: {}", js_error(error)), true);
        }
    });
    app.borrow().window.add_event_listener_with_callback("message", callback.as_ref().unchecked_ref())?;
    callback.forget();
    Ok(())
}

fn download(document: &Document, bytes: &[u8]) -> Result<(), JsValue> {
    let parts = js_sys::Array::new();
    parts.push(&js_sys::Uint8Array::from(bytes));
    let blob = web_sys::Blob::new_with_u8_array_sequence(&parts)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)?;
    let link: HtmlAnchorElement = document.create_element("a")?.dyn_into()?;
    link.set_href(&url);
    link.set_download("Workbook.xlsx");
    let body = document.body().ok_or("document body missing")?;
    body.append_child(&link)?;
    link.click();
    link.remove();
    // Let the browser consume the download URL before releasing it.
    let callback = Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    if let Some(window) = web_sys::window() {
        window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), 1_000)?;
    }
    Ok(())
}
