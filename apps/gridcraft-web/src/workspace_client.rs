//! Rust-only same-origin client. All workbook tools run synchronously on the mounted app.
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

use gridcraft_engine::{
    Session,
    agent_bridge::Identity,
    workspace::{AttachmentVersion, LibraryFile, RunView, SavedSnapshot, StartRun, ToolContinuation, demo_catalog, demo_preview, demo_xlsx},
};
use gridcraft_ui_egui::{SheetApp, agent::WorkspaceAction};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};

type Result<T> = std::result::Result<T, String>;
const RECOVERY: &str = "gridcraft.workspace.recovery.v1";
const RUN: &str = "gridcraft.workspace.run.v1";
const CONTINUATION: &str = "gridcraft.workspace.receipt.v1";
const SAVE: &str = "gridcraft.workspace.save.v1";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok()?
}
fn stored<T: serde::de::DeserializeOwned>(key: &str) -> Option<T> {
    serde_json::from_str(&storage()?.get_item(key).ok()??).ok()
}
fn persist(key: &str, value: &impl Serialize) -> Result<()> {
    let text = serde_json::to_string(value).map_err(|_| "Cannot encode browser recovery")?;
    storage()
        .ok_or("Browser recovery storage is unavailable")?
        .set_item(key, &text)
        .map_err(|_| "Browser recovery storage is full or unavailable".into())
}
fn remove(key: &str) {
    if let Some(storage) = storage() {
        let _ = storage.remove_item(key);
    }
}
fn uuid() -> String {
    if let Some(window) = web_sys::window()
        && let Ok(crypto) = window.crypto()
        && let Ok(value) = js_sys::Reflect::get(&crypto, &JsValue::from_str("randomUUID"))
        && let Ok(function) = value.dyn_into::<js_sys::Function>()
        && let Ok(value) = function.call0(&crypto)
        && let Some(id) = value.as_string()
    {
        return id;
    }
    // No stable fallback identity: refusing startup is safer than reusing an editor token.
    String::new()
}

#[derive(Clone, Serialize, Deserialize)]
struct Recovery {
    name: String,
    base64: String,
    dirty: bool,
    cloud: Option<AttachmentVersion>,
}

pub fn restore(session: &mut Session) -> bool {
    let Some(recovery) = stored::<Recovery>(RECOVERY) else { return false };
    if recovery.base64.len() > 5_000_000 {
        return false;
    }
    if session.execute("file.open", json!({"name":recovery.name,"base64":recovery.base64})).is_err() {
        return false;
    }
    if recovery.dirty
        && let Some(doc) = session.active_mut()
    {
        doc.saved = Arc::new((*doc.wb).clone());
    }
    true
}

#[derive(Clone, Serialize, Deserialize)]
struct PendingSave {
    workbook_id: String,
    title: String,
    export_id: String,
    workbook_session_id: String,
    identity: Identity,
    expected_version: i64,
    base64: String,
    tool_call_id: Option<String>,
    run_id: Option<String>,
}

enum Event {
    Status(Value),
    Rules(Value),
    Files(Vec<LibraryFile>),
    Run(RunView),
    File(AttachmentVersion, Vec<u8>),
    Saved(PendingSave, SavedSnapshot),
    Transcript(String),
    Audio(String, Vec<u8>),
    Error(String),
    Recording,
    Transcribing,
    SpeechStatus(String),
    RecordingFailed(String),
}
type Events = Rc<RefCell<Vec<Event>>>;

pub struct Client {
    session_id: String,
    events: Events,
    cloud: HashMap<u64, AttachmentVersion>,
    last_local: Option<Arc<gridcraft_engine::model::Workbook>>,
    pending_save: Option<PendingSave>,
    pending_start: Option<StartRun>,
    pending_continuation: Option<(String, ToolContinuation)>,
    database: bool,
    recording: Rc<RefCell<Recording>>,
    audio: Option<(web_sys::HtmlAudioElement, String)>,
}

impl Client {
    pub fn new(app: &mut SheetApp, ctx: egui::Context, recovered: bool) -> Self {
        let events = Rc::new(RefCell::new(Vec::new()));
        let session_id = uuid();
        app.agent.connection = "Checking the local service…".into();
        app.agent.files = demo_catalog()
            .into_iter()
            .map(|(id, title)| LibraryFile {
                workbook_id: id.into(),
                title: title.into(),
                latest_version: 1,
                source: "local mock source (not cloud-saved)".into(),
                preview: demo_preview(id).unwrap_or_default(),
            })
            .collect();
        launch(events.clone(), ctx.clone(), async { Ok(Event::Status(json_request("/api/status", "GET", None).await?)) });
        let mut cloud = HashMap::new();
        if recovered
            && let Some(recovery) = stored::<Recovery>(RECOVERY)
            && let Some(file) = recovery.cloud
            && let Some(doc) = app.session.active()
        {
            cloud.insert(doc.uid, file);
        }
        let pending_start = stored::<StartRun>(RUN);
        if let Some(request) = &pending_start {
            let path = format!("/api/runs/{}", request.run_id);
            launch(events.clone(), ctx.clone(), async move { Ok(Event::Run(decode(json_request(&path, "GET", None).await?)?)) });
        }
        Self {
            session_id,
            events,
            cloud,
            last_local: None,
            pending_save: stored(SAVE),
            pending_start,
            pending_continuation: stored(CONTINUATION),
            database: false,
            recording: Rc::new(RefCell::new(Recording::default())),
            audio: None,
        }
    }

    pub fn tick(&mut self, app: &mut SheetApp, ctx: &egui::Context) {
        let events = std::mem::take(&mut *self.events.borrow_mut());
        for event in events {
            match event {
                Event::Status(status) => {
                    self.database = status.get("database_configured").and_then(Value::as_bool) == Some(true);
                    app.agent.connected = status.get("durable_agent_loop").and_then(Value::as_bool) == Some(true) && !self.session_id.is_empty();
                    app.agent.connection = if app.agent.connected {
                        "Connected service · mock workspace · provider connectivity is checked per request"
                    } else {
                        "Local deterministic demo · OpenAI/database workflow unavailable"
                    }
                    .into();
                    if self.database {
                        self.files(ctx);
                        launch(self.events.clone(), ctx.clone(), async { Ok(Event::Rules(json_request("/api/rules", "GET", None).await?)) });
                    }
                }
                Event::Rules(value) => {
                    let rules = value
                        .as_array()
                        .map(|rows| {
                            rows.iter()
                                .filter_map(|row| {
                                    if row.get("kind").and_then(Value::as_str) != Some("exclude_test_orders") {
                                        return None;
                                    }
                                    let mut rule = gridcraft_engine::review_workflow::ReviewRule::from_instruction(
                                        row.get("id")?.as_str()?,
                                        row.get("original_wording")?.as_str()?,
                                    )
                                    .ok()?;
                                    rule.set_enabled(row.get("enabled")?.as_bool()?);
                                    Some(rule)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    app.agent.rules = rules;
                }
                Event::Files(files) => app.agent.files = files,
                Event::File(file, bytes) => {
                    if app.host_busy() {
                        fail(app, "Finish or cancel the current edit before opening a file. Your draft was kept.");
                        continue;
                    }
                    match app.run("file.open", json!({"name":file.title,"base64":gridcraft_engine::io::base64_encode(&bytes)})) {
                        Ok(_) => {
                            if let Some(doc) = app.session.active() {
                                self.cloud.insert(doc.uid, file);
                            }
                            app.agent.files_open = false;
                            self.last_local = None;
                        }
                        Err(error) => fail(app, &error),
                    }
                }
                Event::Run(view) => self.run_result(app, ctx, view),
                Event::Saved(pending, receipt) => {
                    let bytes = gridcraft_engine::io::base64_decode(&pending.base64);
                    let hash = bytes
                        .as_ref()
                        .map(|bytes| Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>())
                        .unwrap_or_default();
                    if receipt.workbook_id != pending.workbook_id
                        || receipt.export_id != pending.export_id
                        || receipt.workbook_session_id != pending.workbook_session_id
                        || receipt.identity != pending.identity
                        || receipt.sha256 != hash
                        || receipt.byte_length != bytes.as_ref().map_or(0, Vec::len)
                        || receipt.storage_version != pending.expected_version.saturating_add(1)
                    {
                        fail(app, "Storage receipt does not match the exact exported snapshot. The workbook remains dirty.");
                        continue;
                    }
                    let mut ack = json!({"clean":false,"recovered_receipt":true});
                    if pending.workbook_session_id == self.session_id {
                        match app.agent.bridge.acknowledge_snapshot(&mut app.session, &pending.export_id) {
                            Ok(value) => {
                                ack = json!(value);
                            }
                            Err(error) => {
                                fail(app, &error.to_string());
                                continue;
                            }
                        }
                        self.cloud.insert(
                            pending.identity.document_id,
                            AttachmentVersion {
                                workbook_id: pending.workbook_id.clone(),
                                version: self
                                    .cloud
                                    .get(&pending.identity.document_id)
                                    .filter(|f| f.workbook_id == pending.workbook_id)
                                    .map_or(receipt.storage_version, |f| f.version.max(receipt.storage_version)),
                                title: pending.title.clone(),
                            },
                        );
                    } else if let Some(doc) = app.session.active()
                        && self.cloud.get(&doc.uid).is_some_and(|f| f.workbook_id == pending.workbook_id)
                        && let Some(file) = self.cloud.get_mut(&doc.uid)
                    {
                        file.version = file.version.max(receipt.storage_version);
                    }
                    app.agent.save_status = format!(
                        "Snapshot committed · version {} · {}",
                        receipt.storage_version,
                        if ack.get("clean").and_then(Value::as_bool) == Some(true) { "saved" } else { "later or recovered edits remain unsaved" }
                    );
                    if self
                        .pending_save
                        .as_ref()
                        .is_some_and(|save| save.export_id == pending.export_id && save.workbook_session_id == pending.workbook_session_id)
                    {
                        self.pending_save = None;
                        remove(SAVE);
                    }
                    self.last_local = None;
                    if let (Some(tool), Some(run_id)) = (pending.tool_call_id, pending.run_id) {
                        self.continue_tool(
                            ctx,
                            run_id,
                            ToolContinuation {
                                workbook_session_id: pending.workbook_session_id,
                                tool_call_id: tool,
                                result: json!({"saved":true,"receipt":receipt,"acknowledgement":ack}),
                            },
                        );
                    }
                    self.files(ctx);
                }
                Event::Transcript(text) => {
                    app.agent.recording = false;
                    if text.len() > 2000 {
                        fail(app, "Transcript exceeds 2,000 bytes. Shorten the recording and try again.");
                    } else {
                        if app.agent.prompt.trim().is_empty() {
                            app.agent.prompt = text;
                        } else {
                            app.agent.prompt.push('\n');
                            app.agent.prompt.push_str(&text);
                        }
                        app.agent.speech_status = "Transcript ready. Edit it, then Send.".into();
                    }
                }
                Event::Recording => {
                    app.agent.recording = true;
                    app.agent.speech_status = "Recording · finish to edit the transcript, or cancel".into();
                }
                Event::Audio(run_id, bytes) => {
                    if app.agent.run.as_ref().is_none_or(|run| run.run_id != run_id) {
                        continue;
                    }
                    if let Some((audio, url)) = self.audio.take() {
                        let _ = audio.pause();
                        let _ = web_sys::Url::revoke_object_url(&url);
                    }
                    match audio_element(&bytes) {
                        Ok(audio) => {
                            self.audio = Some(audio);
                            app.agent.speech_status = "Speech ready. Click Read outcome aloud to play or pause.".into();
                        }
                        Err(error) => fail(app, &error),
                    }
                }
                Event::Transcribing => {
                    app.agent.recording = false;
                    app.agent.speech_status = "Transcribing…".into();
                }
                Event::SpeechStatus(status) => {
                    app.agent.speech_status = status;
                }
                Event::RecordingFailed(error) => {
                    app.agent.recording = false;
                    fail(app, &error);
                }
                Event::Error(error) => {
                    app.agent.working = false;
                    fail(app, &error);
                }
            }
        }
        for action in std::mem::take(&mut app.agent.outbox) {
            match action {
                WorkspaceAction::Submit(prompt) => {
                    if self.pending_save.is_some() || self.pending_continuation.is_some() {
                        fail(app, "Recover the pending save or tool receipt before starting another request.");
                        continue;
                    }
                    let context = app.agent.bridge.handle(&mut app.session, "context", json!({}));
                    match context {
                        Ok(context) => {
                            let request = StartRun {
                                run_id: uuid(),
                                workbook_session_id: self.session_id.clone(),
                                prompt,
                                context,
                                attachments: app.agent.attachments.clone(),
                            };
                            if let Err(error) = persist(RUN, &request) {
                                fail(app, &error);
                                continue;
                            }
                            self.pending_start = Some(request.clone());
                            self.pending_continuation = None;
                            remove(CONTINUATION);
                            launch(self.events.clone(), ctx.clone(), async move {
                                Ok(Event::Run(decode(json_request("/api/runs", "POST", Some(json!(request))).await?)?))
                            });
                        }
                        Err(error) => fail(app, &error.to_string()),
                    }
                }
                WorkspaceAction::Continue(receipt) => {
                    if let Some(run) = &app.agent.run {
                        self.continue_tool(ctx, run.run_id.clone(), receipt);
                    }
                }
                WorkspaceAction::Files => {
                    if self.database {
                        self.files(ctx);
                    }
                }
                WorkspaceAction::Open(file) => {
                    if self.database {
                        let path = format!("/api/workbooks/{}/snapshots/{}", file.workbook_id, file.version);
                        launch(
                            self.events.clone(),
                            ctx.clone(),
                            async move { Ok(Event::File(file, request_bytes(&path, "GET", None, None).await?)) },
                        );
                    } else {
                        match demo_xlsx(&file.workbook_id) {
                            Ok(bytes) => self.events.borrow_mut().push(Event::File(file, bytes)),
                            Err(error) => fail(app, &error.to_string()),
                        }
                    }
                }
                WorkspaceAction::Save => {
                    if let Err(error) = self.save(app, ctx, None) {
                        fail(app, &error);
                    }
                }
                WorkspaceAction::Record => {
                    if !app.agent.prompt.trim().is_empty() {
                        fail(app, "Send or clear the current draft before recording; your text was kept.");
                        continue;
                    }
                    record(self.recording.clone(), self.events.clone(), ctx.clone());
                }
                WorkspaceAction::StopRecording => {
                    let mut recording = self.recording.borrow_mut();
                    if let Some(recorder) = &recording.recorder {
                        let _ = recorder.stop();
                    } else {
                        recording.cancel();
                    }
                    app.agent.recording = false;
                    app.agent.speech_status = "Transcribing…".into();
                }
                WorkspaceAction::CancelRecording => {
                    self.recording.borrow_mut().cancel();
                    app.agent.recording = false;
                    app.agent.speech_status = "Recording cancelled. Nothing submitted.".into();
                }
                WorkspaceAction::Play => {
                    if let Some((audio, _)) = &self.audio {
                        if audio.paused() {
                            match audio.play() {
                                Ok(promise) => launch(self.events.clone(), ctx.clone(), async move {
                                    JsFuture::from(promise).await.map_err(js_error)?;
                                    Ok(Event::SpeechStatus("Playing verified outcome · click again to pause".into()))
                                }),
                                Err(error) => fail(app, &js_error(error)),
                            }
                        } else {
                            let _ = audio.pause();
                            app.agent.speech_status = "Playback paused".into();
                        }
                    } else if let Some(run) = &app.agent.run {
                        let run_id = run.run_id.clone();
                        let path = format!("/api/runs/{run_id}/speech");
                        launch(
                            self.events.clone(),
                            ctx.clone(),
                            async move { Ok(Event::Audio(run_id, request_bytes(&path, "POST", None, None).await?)) },
                        );
                    }
                }
                WorkspaceAction::Resume => {
                    app.agent.error = false;
                    self.resume(app, ctx);
                }
            }
        }
        // Same-tab reload recovery preserves unfinished cell drafts by waiting until they
        // finish, and captures workbook bytes rather than replaying native commands.
        if !app.host_busy()
            && let Some(doc) = app.session.active()
        {
            let changed = self.last_local.as_ref().is_none_or(|old| !Arc::ptr_eq(old, &doc.wb));
            if changed {
                match gridcraft_engine::io::save_bytes(&doc.wb, &doc.title) {
                    Ok(bytes) if bytes.len() <= 3_500_000 => {
                        let recovery = Recovery {
                            name: doc.title.clone(),
                            base64: gridcraft_engine::io::base64_encode(&bytes),
                            dirty: doc.is_dirty(),
                            cloud: self.cloud.get(&doc.uid).cloned(),
                        };
                        if let Err(error) = persist(RECOVERY, &recovery) {
                            app.agent.save_status = error;
                        }
                        self.last_local = Some(doc.wb.clone());
                    }
                    Ok(_) => {
                        app.agent.save_status = "Workbook exceeds browser recovery capacity. Save a cloud snapshot or download it.".into();
                        self.last_local = Some(doc.wb.clone());
                    }
                    Err(error) => app.agent.save_status = error.to_string(),
                }
            }
        }
    }

    fn files(&self, ctx: &egui::Context) {
        launch(self.events.clone(), ctx.clone(), async { Ok(Event::Files(decode(json_request("/api/library", "GET", None).await?)?)) });
    }

    fn continue_tool(&mut self, ctx: &egui::Context, run_id: String, receipt: ToolContinuation) {
        self.pending_continuation = Some((run_id.clone(), receipt.clone()));
        if let Err(error) = persist(CONTINUATION, &(run_id.clone(), receipt.clone())) {
            self.events.borrow_mut().push(Event::Error(error));
            return;
        }
        let path = format!("/api/runs/{run_id}/continue");
        launch(self.events.clone(), ctx.clone(), async move { Ok(Event::Run(decode(json_request(&path, "POST", Some(json!(receipt))).await?)?)) });
    }

    fn run_result(&mut self, app: &mut SheetApp, ctx: &egui::Context, view: RunView) {
        app.agent.working = false;
        if self.pending_continuation.as_ref().is_some_and(|(run, receipt)| {
            run == &view.run_id && view.pending.as_ref().is_none_or(|pending| pending.tool_call_id != receipt.tool_call_id)
        }) {
            self.pending_continuation = None;
            remove(CONTINUATION);
        }
        app.agent.receipts = view.completed_actions.clone();
        app.agent.message(false, view.message.clone());
        if view.status == "interrupted" {
            app.agent.error = true;
            app.agent.status = Some(view.message.clone());
        }
        app.agent.run = Some(view.clone());
        if view.workbook_session_id != self.session_id {
            app.agent.pending_tool = None;
            app.agent.message(false,"Recovered the previous run for inspection. Its editor session ended; pending mutations will not replay into this workbook. Start a new request after reviewing the recovered cells and actions.");
            // Previously recorded results can be delivered safely; no workbook operation
            // is executed again. An unknown pending effect needs user inspection.
            if let Some((run_id, receipt)) = self.pending_continuation.take()
                && view.pending.as_ref().is_some_and(|p| p.tool_call_id == receipt.tool_call_id)
            {
                self.continue_tool(ctx, run_id, receipt);
            }
            return;
        }
        if let Some(pending) = view.pending.clone() {
            match pending.method.as_str() {
                "set_values" | "undo" => {
                    app.agent.pending_tool = Some(pending);
                }
                "save_snapshot" => {
                    let expected = pending.params.get("expected").cloned().and_then(|value| serde_json::from_value::<Identity>(value).ok());
                    let current = app.agent.bridge.observe(&app.session).ok().map(|context| context.identity);
                    let result = if expected.is_none() || current != expected {
                        Err("Save context changed; inspect the live workbook again.".into())
                    } else {
                        self.save(app, ctx, Some(pending.tool_call_id.clone()))
                    };
                    if let Err(error) = result {
                        self.continue_tool(
                            ctx,
                            view.run_id,
                            ToolContinuation {
                                workbook_session_id: self.session_id.clone(),
                                tool_call_id: pending.tool_call_id,
                                result: json!({"error":error}),
                            },
                        );
                    }
                }
                "context" | "inspect" => {
                    let result = if app.host_busy() {
                        json!({"error":"busy: finish or cancel the current edit; draft preserved"})
                    } else {
                        match app.agent.bridge.handle(&mut app.session, &pending.method, pending.params) {
                            Ok(value) => value,
                            Err(error) => json!({"error":error.to_string()}),
                        }
                    };
                    app.agent.working = true;
                    self.continue_tool(
                        ctx,
                        view.run_id,
                        ToolContinuation { workbook_session_id: self.session_id.clone(), tool_call_id: pending.tool_call_id, result },
                    );
                }
                _ => fail(app, "Unsupported editor tool requested."),
            }
        } else {
            self.pending_continuation = None;
            remove(CONTINUATION);
            if view.status == "complete" {
                remove(RUN);
                self.pending_start = None;
            }
        }
        if view.status == "complete" {
            launch(self.events.clone(), ctx.clone(), async { Ok(Event::Rules(json_request("/api/rules", "GET", None).await?)) });
            if let Some((audio, url)) = self.audio.take() {
                let _ = audio.pause();
                let _ = web_sys::Url::revoke_object_url(&url);
            }
        }
    }

    fn resume(&mut self, app: &mut SheetApp, ctx: &egui::Context) {
        if let Some(pending) = self.pending_save.clone() {
            upload(self.events.clone(), ctx.clone(), pending);
            return;
        }
        if let Some((run_id, receipt)) = self.pending_continuation.clone() {
            self.continue_tool(ctx, run_id, receipt);
            return;
        }
        if let Some(request) = self.pending_start.clone() {
            if request.workbook_session_id != self.session_id {
                fail(app, "Previous editor session ended. Review recovered cells and start a new request.");
                return;
            }
            launch(self.events.clone(), ctx.clone(), async move {
                Ok(Event::Run(decode(json_request("/api/runs", "POST", Some(json!(request))).await?)?))
            });
            app.agent.working = true;
        }
    }

    fn save(&mut self, app: &mut SheetApp, ctx: &egui::Context, tool_call_id: Option<String>) -> Result<()> {
        if !self.database {
            return Err("Cloud storage is unavailable. Use Files → Download current XLSX.".into());
        }
        if let Some(pending) = self.pending_save.clone() {
            upload(self.events.clone(), ctx.clone(), pending);
            return Ok(());
        }
        if app.host_busy() {
            return Err("Finish or cancel the current edit before saving. Your draft was kept.".into());
        }
        let expected = app.agent.bridge.observe(&app.session).map_err(|e| e.to_string())?.identity;
        let export_id = uuid();
        let snapshot = app.agent.bridge.capture_snapshot(&app.session, &export_id, &expected).map_err(|e| e.to_string())?;
        let doc = app.session.doc().map_err(|e| e.to_string())?;
        let file = self.cloud.get(&doc.uid).cloned().unwrap_or(AttachmentVersion { workbook_id: uuid(), title: doc.title.clone(), version: 0 });
        let bytes = gridcraft_engine::io::save_bytes(&snapshot.workbook, "snapshot.xlsx").map_err(|e| e.to_string())?;
        let pending = PendingSave {
            workbook_id: file.workbook_id,
            title: file.title,
            expected_version: file.version,
            export_id,
            workbook_session_id: self.session_id.clone(),
            identity: expected,
            base64: gridcraft_engine::io::base64_encode(&bytes),
            tool_call_id,
            run_id: app.agent.run.as_ref().map(|r| r.run_id.clone()),
        };
        self.pending_save = Some(pending.clone());
        app.agent.save_status = match persist(SAVE, &pending) {
            Ok(()) => "Uploading the captured snapshot…".into(),
            Err(error) => format!("{error}. Uploading with in-memory retry; keep this tab open until the receipt arrives."),
        };
        upload(self.events.clone(), ctx.clone(), pending);
        Ok(())
    }
}

fn upload(events: Events, ctx: egui::Context, pending: PendingSave) {
    launch(events, ctx, async move {
        let path = format!("/api/workbooks/{}", pending.workbook_id);
        json_request(&path, "PUT", Some(json!({"title":pending.title}))).await?;
        let form = web_sys::FormData::new().map_err(js_error)?;
        let metadata = json!({"export_id":pending.export_id,"workbook_session_id":pending.workbook_session_id,"identity":pending.identity,"expected_version":pending.expected_version});
        form.append_with_str("metadata", &metadata.to_string()).map_err(js_error)?;
        let bytes = gridcraft_engine::io::base64_decode(&pending.base64).ok_or("Invalid pending snapshot encoding")?;
        let blob = blob(&bytes, "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")?;
        form.append_with_blob_and_filename("file", &blob, "snapshot.xlsx").map_err(js_error)?;
        let response = request_bytes(&format!("{path}/snapshots"), "POST", Some(form.as_ref()), None).await?;
        let receipt = serde_json::from_slice(&response).map_err(|_| "Invalid snapshot receipt")?;
        Ok(Event::Saved(pending, receipt))
    });
}

fn launch(events: Events, ctx: egui::Context, future: impl std::future::Future<Output = Result<Event>> + 'static) {
    spawn_local(async move {
        let result = match future.await {
            Ok(event) => event,
            Err(error) => Event::Error(error),
        };
        events.borrow_mut().push(result);
        ctx.request_repaint();
    });
}
fn fail(app: &mut SheetApp, error: &str) {
    app.agent.error = true;
    app.agent.working = false;
    app.agent.message(false, error);
    app.agent.status = Some(error.into());
}
fn js_error(_: JsValue) -> String {
    "Browser operation failed; check permissions and local service availability".into()
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|_| "Invalid service response".into())
}
async fn json_request(path: &str, method: &str, body: Option<Value>) -> Result<Value> {
    let body = body.map(|v| JsValue::from_str(&v.to_string()));
    let bytes = request_bytes(path, method, body.as_ref(), Some("application/json")).await?;
    serde_json::from_slice(&bytes).map_err(|_| "Service response was not JSON".into())
}
async fn request_bytes(path: &str, method: &str, body: Option<&JsValue>, mime: Option<&str>) -> Result<Vec<u8>> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    if let Some(body) = body {
        init.set_body(body);
    }
    let request = web_sys::Request::new_with_str_and_init(path, &init).map_err(js_error)?;
    if let Some(mime) = mime {
        request.headers().set("content-type", mime).map_err(js_error)?;
    }
    let response: web_sys::Response = JsFuture::from(web_sys::window().ok_or("No browser window")?.fetch_with_request(&request))
        .await
        .map_err(js_error)?
        .dyn_into()
        .map_err(js_error)?;
    if !response.ok() {
        return Err(format!(
            "Service request failed (HTTP {}). Completed actions are retained; use Recover run or Save to retry.",
            response.status()
        ));
    }
    let buffer = JsFuture::from(response.array_buffer().map_err(js_error)?).await.map_err(js_error)?;
    let bytes = js_sys::Uint8Array::new(&buffer);
    if bytes.length() > 17_000_000 {
        return Err("Service response exceeds the supported size".into());
    }
    Ok(bytes.to_vec())
}
fn blob(bytes: &[u8], mime: &str) -> Result<web_sys::Blob> {
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    web_sys::Blob::new_with_u8_array_sequence_and_options(&js_sys::Array::of1(&js_sys::Uint8Array::from(bytes)), &options).map_err(js_error)
}
fn audio_element(bytes: &[u8]) -> Result<(web_sys::HtmlAudioElement, String)> {
    let url = web_sys::Url::create_object_url_with_blob(&blob(bytes, "audio/mpeg")?).map_err(js_error)?;
    let audio = web_sys::HtmlAudioElement::new_with_src(&url).map_err(js_error)?;
    Ok((audio, url))
}

#[derive(Default)]
struct Recording {
    generation: u64,
    cancelled: bool,
    recorder: Option<web_sys::MediaRecorder>,
    stream: Option<web_sys::MediaStream>,
    data: Option<Closure<dyn FnMut(web_sys::BlobEvent)>>,
    stop: Option<Closure<dyn FnMut()>>,
}
impl Recording {
    fn stop_tracks(&mut self) {
        if let Some(stream) = self.stream.take() {
            for track in stream.get_tracks().iter() {
                if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
                    track.stop();
                }
            }
        }
    }
    fn cancel(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.cancelled = true;
        if let Some(recorder) = self.recorder.take() {
            recorder.set_ondataavailable(None);
            recorder.set_onstop(None);
            let _ = recorder.stop();
        }
        self.stop_tracks();
        self.data = None;
        self.stop = None;
    }
}
fn record(state: Rc<RefCell<Recording>>, events: Events, ctx: egui::Context) {
    {
        let mut recording = state.borrow_mut();
        recording.cancel();
        recording.cancelled = false;
    }
    events.borrow_mut().push(Event::Recording);
    ctx.request_repaint();
    let generation = state.borrow().generation;
    spawn_local(async move {
        let result: Result<()> = async {
            let navigator = web_sys::window().ok_or("No browser window")?.navigator();
            let constraints = web_sys::MediaStreamConstraints::new();
            constraints.set_audio(&JsValue::TRUE);
            let stream: web_sys::MediaStream =
                JsFuture::from(navigator.media_devices().map_err(js_error)?.get_user_media_with_constraints(&constraints).map_err(js_error)?)
                    .await
                    .map_err(js_error)?
                    .dyn_into()
                    .map_err(js_error)?;
            if state.borrow().generation != generation {
                for track in stream.get_tracks().iter() {
                    if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
                        track.stop();
                    }
                }
                return Ok(());
            }
            state.borrow_mut().stream = Some(stream.clone());
            let recorder = web_sys::MediaRecorder::new_with_media_stream(&stream).map_err(js_error)?;
            let chunks = Rc::new(RefCell::new(Vec::<web_sys::Blob>::new()));
            let size = Rc::new(RefCell::new(0_f64));
            let data_chunks = chunks.clone();
            let data_size = size.clone();
            let data_recorder = recorder.clone();
            let data_state = state.clone();
            let data_events = events.clone();
            let data_ctx = ctx.clone();
            let data = Closure::wrap(Box::new(move |event: web_sys::BlobEvent| {
                if let Some(blob) = event.data() {
                    *data_size.borrow_mut() += blob.size();
                    if *data_size.borrow() > 10_000_000.0 {
                        data_state.borrow_mut().cancelled = true;
                        let _ = data_recorder.stop();
                        data_events.borrow_mut().push(Event::RecordingFailed("Recording exceeds 10 MB. Try a shorter request.".into()));
                        data_ctx.request_repaint();
                    } else {
                        data_chunks.borrow_mut().push(blob);
                    }
                }
            }) as Box<dyn FnMut(web_sys::BlobEvent)>);
            recorder.set_ondataavailable(Some(data.as_ref().unchecked_ref()));
            let stop_state = state.clone();
            let stop_events = events.clone();
            let stop_ctx = ctx.clone();
            let stop = Closure::wrap(Box::new(move || {
                let mut state = stop_state.borrow_mut();
                state.stop_tracks();
                if state.cancelled || state.generation != generation {
                    return;
                }
                stop_events.borrow_mut().push(Event::Transcribing);
                stop_ctx.request_repaint();
                let parts = js_sys::Array::new();
                for chunk in chunks.borrow().iter() {
                    parts.push(chunk);
                }
                let mime = chunks.borrow().first().map(web_sys::Blob::type_).unwrap_or_else(|| "audio/webm".into());
                let options = web_sys::BlobPropertyBag::new();
                options.set_type(&mime);
                let blob = web_sys::Blob::new_with_blob_sequence_and_options(&parts, &options);
                let events = stop_events.clone();
                let ctx = stop_ctx.clone();
                let check_state = stop_state.clone();
                launch(events, ctx, async move {
                    let blob = blob.map_err(js_error)?;
                    let buffer = JsFuture::from(blob.array_buffer()).await.map_err(js_error)?;
                    if check_state.borrow().cancelled || check_state.borrow().generation != generation {
                        return Err("Recording cancelled".into());
                    }
                    let bytes = js_sys::Uint8Array::new(&buffer);
                    let body: JsValue = bytes.into();
                    let response = request_bytes("/api/speech/transcribe", "POST", Some(&body), Some(&mime)).await?;
                    if check_state.borrow().cancelled || check_state.borrow().generation != generation {
                        return Err("Transcription cancelled".into());
                    }
                    let value: gridcraft_engine::workspace::SpeechTranscript =
                        serde_json::from_slice(&response).map_err(|_| "Invalid transcript response")?;
                    Ok(Event::Transcript(value.text))
                });
            }) as Box<dyn FnMut()>);
            recorder.set_onstop(Some(stop.as_ref().unchecked_ref()));
            recorder.start_with_time_slice(1000).map_err(js_error)?;
            let timer_recorder = recorder.clone();
            let timer_state = state.clone();
            let timer = Closure::once_into_js(move || {
                if timer_state.borrow().generation == generation {
                    let _ = timer_recorder.stop();
                }
            });
            web_sys::window()
                .ok_or("No browser window")?
                .set_timeout_with_callback_and_timeout_and_arguments_0(timer.unchecked_ref(), 60_000)
                .map_err(js_error)?;
            let mut state = state.borrow_mut();
            state.recorder = Some(recorder);
            state.stream = Some(stream);
            state.data = Some(data);
            state.stop = Some(stop);
            events.borrow_mut().push(Event::Recording);
            ctx.request_repaint();
            Ok(())
        }
        .await;
        if let Err(error) = result {
            state.borrow_mut().stop_tracks();
            events.borrow_mut().push(Event::RecordingFailed(error));
            ctx.request_repaint();
        }
    });
}
