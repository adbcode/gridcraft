//! Connected agent mode. The durable server run decides which tool to call; the browser runs
//! reads at once through the live bridge and writes only after Apply, against the workbook
//! identity captured when the proposal arrived. The transport is the host's `AgentApi`.

use std::collections::BTreeMap;

use gridcraft_engine::agent_bridge::Identity;
use gridcraft_engine::agent_protocol::{AttachmentRef, MAX_ATTACHMENTS, PendingTool, RunContinue, RunStart, RunView, ToolOutcome, WritePlan};
use gridcraft_engine::core::CellRef;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::SheetApp;
use crate::agent::State;
use crate::agent_api::{ApiRequest, ApiResponse, Method, RawBody, Upload, query_component};

/// Network failures are retried with the identical body, at most this many attempts in total.
pub const MAX_ATTEMPTS: u32 = 3;
const MAX_FILES: usize = 64;
const MAX_ACTIONS: usize = 40;
const MAX_RULES: usize = 32;
const MAX_QUERY_BYTES: usize = 200;
const MAX_OUTCOME_BYTES: usize = 60_000;
const SUPPORTED: &[&str] = &["xlsx", "xlsm", "xls", "xlsb", "ods", "csv", "tsv"];
const OPEN_TIMEOUT_SECONDS: f64 = 30.0;
/// Recordings stop automatically after this long.
pub const MAX_RECORDING_SECONDS: f64 = 60.0;
const MAX_SPOKEN_CHARS: usize = 280;
pub const STORE_WORKBOOK: &str = "gridcraft.agent.workbook";
pub const STORE_RUN: &str = "gridcraft.agent.run";
const LIBRARY_OFFLINE: &str = "The cloud library needs the GridCraft service; this build is running the local demo.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Connection {
    #[default]
    Checking,
    LocalDemo,
    Connected,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct FileCard {
    pub workbook_id: String,
    pub title: String,
    pub origin: String,
    pub latest_version: i64,
    #[serde(default)]
    pub sha256: Option<String>,
}

impl FileCard {
    pub fn supported(&self) -> bool {
        self.title.rsplit_once('.').is_some_and(|(_, ext)| SUPPORTED.contains(&ext.to_ascii_lowercase().as_str()))
    }

    pub fn is_mock(&self) -> bool {
        self.origin == "mock"
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct FilePreview {
    pub workbook_id: String,
    pub version: i64,
    pub sheets: Vec<SheetPreview>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SheetPreview {
    pub name: String,
    pub cells: Vec<Vec<Json>>,
}

/// The cloud workbook (and committed version) an open document was loaded from or saved to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CloudBinding {
    pub workbook_id: String,
    pub version: i64,
    pub title: String,
    #[serde(skip)]
    pub document_id: Option<u64>,
}

/// One exact export: retries re-send these bytes and metadata under the same export ID.
#[derive(Clone, Debug)]
pub struct PendingSave {
    pub workbook_id: String,
    pub export_id: String,
    pub title: String,
    pub metadata: Json,
    pub bytes: Vec<u8>,
    pub document_id: u64,
    pub attempt: u32,
}

/// A downloaded workbook handed to the normal loader; agent actions wait for it to open.
#[derive(Clone, Debug)]
pub struct PendingOpen {
    pub binding: CloudBinding,
    known: Vec<u64>,
    started: f64,
    then_run: Option<String>,
}

/// A browser tool awaiting the user's Apply or Discard.
#[derive(Clone, Debug)]
pub struct Approval {
    pub run_id: String,
    pub tool: PendingTool,
    pub expected: Identity,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunRequest {
    pub method: Method,
    pub path: String,
    pub body: Option<Json>,
    pub attempt: u32,
}

#[derive(Clone, Debug)]
enum Purpose {
    Status,
    Run(RunRequest),
    Files,
    Preview,
    Open { binding: CloudBinding },
    Recover { binding: CloudBinding, run: Option<String> },
    CreateWorkbook,
    Upload,
    Rules,
    Recording,
    Transcribe,
    Synthesize,
}

#[derive(Default)]
pub struct Remote {
    pub connection: Connection,
    pub run: Option<RunView>,
    pub approval: Option<Approval>,
    pub approval_stale: bool,
    /// Completed workbook operations of the current run, from verified receipts.
    pub actions: Vec<String>,
    pub files: Vec<FileCard>,
    pub files_query: String,
    pub files_error: Option<String>,
    pub file_preview: Option<FilePreview>,
    pub attachments: Vec<FileCard>,
    pub rules: Vec<String>,
    /// What the Retry button re-sends (always the identical request).
    pub retry: Option<RunRequest>,
    pub cloud: Option<CloudBinding>,
    pub save: Option<PendingSave>,
    pub save_error: Option<String>,
    pub pending_open: Option<PendingOpen>,
    pub speech_stt: bool,
    pub speech_tts: bool,
    pub recording: bool,
    pub transcribing: bool,
    /// Audio of the last verified spoken outcome, for replay.
    pub spoken: Option<Vec<u8>>,
    record_ticket: Option<u64>,
    record_started: f64,
    voice_draft: bool,
    speak_next: bool,
    save_ticket: Option<u64>,
    save_retry_at: Option<f64>,
    tickets: BTreeMap<u64, Purpose>,
    next_ticket: u64,
    run_ticket: Option<u64>,
    retry_at: Option<(f64, RunRequest)>,
    status_requested: bool,
    now: f64,
    last_range: Option<String>,
}

pub fn connected(state: &State) -> bool {
    state.remote.connection == Connection::Connected
}

/// A run request is in flight or waiting to be retried.
pub fn busy(state: &State) -> bool {
    state.remote.run_ticket.is_some() || state.remote.retry_at.is_some() || state.remote.pending_open.is_some()
}

pub fn retry_pending(state: &State) -> bool {
    state.remote.retry_at.is_some() || state.remote.save_retry_at.is_some() || state.remote.pending_open.is_some() || state.remote.recording
}

pub fn saving(state: &State) -> bool {
    state.remote.save_ticket.is_some() || state.remote.save_retry_at.is_some()
}

/// The cloud binding of the active document, if it has one.
pub fn active_cloud<'a>(state: &'a State, app: &SheetApp) -> Option<&'a CloudBinding> {
    let uid = app.session.active().map(|doc| doc.uid)?;
    state.remote.cloud.as_ref().filter(|binding| binding.document_id == Some(uid))
}

fn stored(app: &SheetApp, key: &str) -> Option<String> {
    app.services.agent_api.as_ref().and_then(|api| api.load(key))
}

fn persist(app: &mut SheetApp, key: &str, value: Option<&str>) {
    if let Some(api) = app.services.agent_api.as_mut() {
        api.store(key, value);
    }
}

pub fn observe(state: &mut State, current: Option<&Identity>) {
    state.remote.approval_stale = state.remote.approval.as_ref().is_some_and(|approval| current != Some(&approval.expected));
}

fn send(state: &mut State, app: &mut SheetApp, method: Method, path: String, body: Option<Json>, purpose: Purpose) -> Result<u64, String> {
    send_upload(state, app, method, path, body, None, purpose)
}

fn send_upload(
    state: &mut State,
    app: &mut SheetApp,
    method: Method,
    path: String,
    body: Option<Json>,
    upload: Option<Upload>,
    purpose: Purpose,
) -> Result<u64, String> {
    send_full(state, app, method, path, body, upload, None, purpose)
}

fn send_full(
    state: &mut State,
    app: &mut SheetApp,
    method: Method,
    path: String,
    body: Option<Json>,
    upload: Option<Upload>,
    raw: Option<RawBody>,
    purpose: Purpose,
) -> Result<u64, String> {
    let api = app.services.agent_api.as_mut().ok_or("the agent service is not available in this build")?;
    let remote = &mut state.remote;
    remote.next_ticket = remote.next_ticket.checked_add(1).ok_or("request identifier limit reached")?;
    let ticket = remote.next_ticket;
    remote.tickets.insert(ticket, purpose);
    api.send(ApiRequest { ticket, method, path, body, upload, raw });
    Ok(ticket)
}

pub fn poll(state: &mut State, app: &mut SheetApp, now: f64) {
    state.remote.now = now;
    if app.services.agent_api.is_none() {
        if state.remote.connection == Connection::Checking {
            state.remote.connection = Connection::LocalDemo;
        }
        return;
    }
    if !state.remote.status_requested {
        state.remote.status_requested = true;
        if send(state, app, Method::Get, "/api/status".into(), None, Purpose::Status).is_err() {
            state.remote.connection = Connection::LocalDemo;
        }
    }
    let responses = app.services.agent_api.as_mut().map(|api| api.poll()).unwrap_or_default();
    for response in responses {
        if let Some(purpose) = state.remote.tickets.remove(&response.ticket) {
            receive(state, app, purpose, response);
        }
    }
    if let Some((at, request)) = state.remote.retry_at.clone()
        && at <= now
    {
        state.remote.retry_at = None;
        dispatch(state, app, request);
    }
    if state.remote.save_retry_at.is_some_and(|at| at <= now) {
        state.remote.save_retry_at = None;
        upload(state, app);
    }
    correlate_open(state, app, now);
    if state.remote.recording && now - state.remote.record_started > MAX_RECORDING_SECONDS {
        let _ = stop_voice(state, app);
    }
}

/// Bind the downloaded workbook once the normal loader has opened it as a new document.
fn correlate_open(state: &mut State, app: &mut SheetApp, now: f64) {
    let Some(open) = state.remote.pending_open.clone() else { return };
    let active = app.session.active().map(|doc| doc.uid).filter(|uid| !open.known.contains(uid));
    if let Some(uid) = active {
        state.remote.pending_open = None;
        let binding = CloudBinding { document_id: Some(uid), ..open.binding };
        if let Ok(text) = serde_json::to_string(&binding) {
            persist(app, STORE_WORKBOOK, Some(&text));
        }
        state.status = Some(format!("Opened {} (version {}).", binding.title, binding.version));
        state.remote.cloud = Some(binding);
        if let Some(run_id) = open.then_run {
            recover_run(state, app, run_id);
        }
    } else if now - open.started > OPEN_TIMEOUT_SECONDS {
        state.remote.pending_open = None;
        state.status = Some(format!("Couldn't open {}; agent actions use the current workbook.", open.binding.title));
    }
}

fn recover(state: &mut State, app: &mut SheetApp) {
    let binding = stored(app, STORE_WORKBOOK).and_then(|text| serde_json::from_str::<CloudBinding>(&text).ok()).filter(|b| safe_id(&b.workbook_id));
    let run = stored(app, STORE_RUN).filter(|id| safe_id(id));
    match binding {
        Some(binding) => {
            state.status = Some(format!("Recovering {} from the cloud library.", binding.title));
            let path = format!("/api/workbooks/{}/snapshots/latest", binding.workbook_id);
            if send(state, app, Method::Get, path, None, Purpose::Recover { binding, run: run.clone() }).is_err()
                && let Some(run_id) = run
            {
                recover_run(state, app, run_id);
            }
        }
        None => {
            if let Some(run_id) = run {
                recover_run(state, app, run_id);
            }
        }
    }
}

fn recover_run(state: &mut State, app: &mut SheetApp, run_id: String) {
    state.status = Some("Recovering the last agent run.".into());
    dispatch(state, app, RunRequest { method: Method::Get, path: format!("/api/runs/{run_id}"), body: None, attempt: 0 });
}

fn begin_open(state: &mut State, app: &mut SheetApp, binding: CloudBinding, bytes: Vec<u8>, then_run: Option<String>) -> bool {
    let Some(inbox) = &app.services.inbox else {
        state.remote.files_error = Some("This build cannot open downloaded files.".into());
        return false;
    };
    let known = app.session.documents().iter().map(|doc| doc.uid).collect();
    inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((binding.title.clone(), bytes));
    state.status = Some(format!("Opening {} (version {}).", binding.title, binding.version));
    state.remote.pending_open = Some(PendingOpen { binding, known, started: state.remote.now, then_run });
    true
}

fn dispatch(state: &mut State, app: &mut SheetApp, request: RunRequest) {
    match send(state, app, request.method, request.path.clone(), request.body.clone(), Purpose::Run(request.clone())) {
        Ok(ticket) => {
            state.remote.run_ticket = Some(ticket);
            state.status = Some(if request.attempt == 0 {
                "Working with the agent service...".into()
            } else {
                format!("Retrying the same request (attempt {} of {MAX_ATTEMPTS})", request.attempt.saturating_add(1))
            });
        }
        Err(error) => fail(state, error, Some(request)),
    }
}

fn fail(state: &mut State, message: impl Into<String>, retry: Option<RunRequest>) {
    let message = message.into();
    state.card_visible = true;
    state.status = Some(message.clone());
    state.error = Some(message.clone());
    state.message(false, message);
    state.remote.retry = retry.map(|request| RunRequest { attempt: 0, ..request });
}

fn error_code(body: &[u8]) -> String {
    serde_json::from_slice::<Json>(body)
        .ok()
        .and_then(|value| value.get("error").and_then(Json::as_str).map(|code| code.chars().take(64).collect()))
        .unwrap_or_else(|| "unknown".into())
}

fn receive(state: &mut State, app: &mut SheetApp, purpose: Purpose, response: ApiResponse) {
    match purpose {
        Purpose::Status => {
            let value: Json = if response.ok() { serde_json::from_slice(&response.body).unwrap_or(Json::Null) } else { Json::Null };
            let flag = |key: &str| value.get(key).and_then(Json::as_bool) == Some(true);
            state.remote.connection = if flag("durable_agent_loop") && flag("openai_configured") && flag("database_configured") {
                Connection::Connected
            } else {
                Connection::LocalDemo
            };
            state.remote.speech_stt = flag("elevenlabs_stt_configured");
            state.remote.speech_tts = flag("elevenlabs_tts_configured");
            if state.remote.connection == Connection::Connected {
                recover(state, app);
            }
        }
        Purpose::Run(request) => {
            if state.remote.run_ticket == Some(response.ticket) {
                state.remote.run_ticket = None;
            }
            if response.status == 0 {
                let attempt = request.attempt.saturating_add(1);
                if attempt < MAX_ATTEMPTS {
                    let delay = 0.5 * f64::from(1u32 << attempt.min(6));
                    state.remote.retry_at = Some((state.remote.now + delay, RunRequest { attempt, ..request }));
                    state.status =
                        Some(format!("Connection lost; retrying in {delay:.0} s (attempt {} of {MAX_ATTEMPTS})", attempt.saturating_add(1)));
                    return;
                }
                return fail(
                    state,
                    "Couldn't reach the agent service after 3 attempts. Nothing was lost; Retry sends the same request again.",
                    Some(request),
                );
            }
            if !response.ok() {
                let code = error_code(&response.body);
                let retry = (500..=599).contains(&response.status).then_some(request);
                return fail(state, format!("The agent service refused the request (HTTP {}: {code}).", response.status), retry);
            }
            match serde_json::from_slice::<RunView>(&response.body) {
                Ok(view) => handle_view(state, app, view),
                Err(_) => fail(state, "The agent service returned an unreadable run.", None),
            }
        }
        Purpose::Files => match serde_json::from_slice::<Vec<FileCard>>(&response.body) {
            Ok(mut files) if response.ok() => {
                files.truncate(MAX_FILES);
                state.remote.files = files;
                state.remote.files_error = None;
            }
            _ => state.remote.files_error = Some(format!("Couldn't load the file library (HTTP {}).", response.status)),
        },
        Purpose::Preview => match serde_json::from_slice::<FilePreview>(&response.body) {
            Ok(preview) if response.ok() => {
                state.remote.file_preview = Some(preview);
                state.remote.files_error = None;
            }
            _ => state.remote.files_error = Some(format!("Couldn't preview this file (HTTP {}).", response.status)),
        },
        Purpose::Open { binding } => {
            if !response.ok() || response.body.is_empty() {
                state.remote.files_error = Some(format!("Couldn't open {} (HTTP {}).", binding.title, response.status));
                return;
            }
            begin_open(state, app, binding, response.body, None);
        }
        Purpose::Recover { binding, run } => {
            let version = response.header("x-gridcraft-storage-version").and_then(|v| v.parse::<i64>().ok());
            match version {
                Some(version) if response.ok() && !response.body.is_empty() => {
                    let binding = CloudBinding { version, ..binding };
                    if begin_open(state, app, binding, response.body, run.clone()) {
                        return;
                    }
                }
                _ if response.status == 404 => persist(app, STORE_WORKBOOK, None),
                _ => state.status = Some(format!("Couldn't recover {} (HTTP {}).", binding.title, response.status)),
            }
            if let Some(run_id) = run {
                recover_run(state, app, run_id);
            }
        }
        Purpose::CreateWorkbook => {
            if response.ok() {
                upload(state, app);
            } else {
                abandon_save(
                    state,
                    format!("Not saved: the library refused the new workbook (HTTP {}: {}).", response.status, error_code(&response.body)),
                );
            }
        }
        Purpose::Upload => receive_upload(state, app, response),
        Purpose::Recording => {
            state.remote.recording = false;
            state.remote.record_ticket = None;
            let mime = response.header("content-type").unwrap_or_default().to_string();
            if response.status != 200 || response.body.is_empty() {
                state.status = Some("No recording: the microphone is unavailable or permission was denied. You can type instead.".into());
                return;
            }
            state.remote.transcribing = true;
            state.status = Some("Transcribing...".into());
            let raw = RawBody { mime, bytes: response.body };
            if let Err(error) = send_full(state, app, Method::Post, "/api/speech/transcribe".into(), None, None, Some(raw), Purpose::Transcribe) {
                state.remote.transcribing = false;
                state.status = Some(error);
            }
        }
        Purpose::Transcribe => {
            state.remote.transcribing = false;
            let text = serde_json::from_slice::<Json>(&response.body)
                .ok()
                .and_then(|value| value.get("text").and_then(Json::as_str).map(str::to_owned))
                .filter(|_| response.ok());
            match text {
                Some(text) if !text.trim().is_empty() => {
                    state.prompt = text.chars().take(gridcraft_engine::agent_protocol::MAX_PROMPT_BYTES / 4).collect();
                    state.remote.voice_draft = true;
                    state.status = Some("Transcript ready. Edit it if needed, then Send.".into());
                }
                Some(_) => state.status = Some("Nothing was heard. Try again or type your request.".into()),
                None => {
                    state.status = Some(format!(
                        "Couldn't transcribe the recording (HTTP {}: {}). Your draft was kept.",
                        response.status,
                        error_code(&response.body)
                    ))
                }
            }
        }
        Purpose::Synthesize => {
            if response.ok() && !response.body.is_empty() {
                state.remote.spoken = Some(response.body.clone());
                if let Some(api) = app.services.agent_api.as_mut()
                    && let Err(error) = api.play(response.body)
                {
                    state.status = Some(error);
                }
            } else {
                state.status = Some(format!("The spoken summary is unavailable (HTTP {}); the written answer above is complete.", response.status));
            }
        }
        Purpose::Rules => {
            if let (true, Ok(rules)) = (response.ok(), serde_json::from_slice::<Vec<Json>>(&response.body)) {
                state.remote.rules = rules
                    .iter()
                    .filter(|rule| rule.get("enabled").and_then(Json::as_bool) != Some(false))
                    .filter_map(|rule| rule.get("original_wording").or_else(|| rule.get("id")).and_then(Json::as_str))
                    .map(|text| text.chars().take(256).collect())
                    .take(MAX_RULES)
                    .collect();
            }
        }
    }
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

fn handle_view(state: &mut State, app: &mut SheetApp, view: RunView) {
    state.card_visible = true;
    state.error = None;
    state.remote.retry = None;
    if !safe_id(&view.run_id) {
        return fail(state, "The agent service returned an invalid run identifier.", None);
    }
    let run_id = view.run_id.clone();
    persist(app, STORE_RUN, if view.status == "completed" { None } else { Some(&run_id) });
    let pending = view.pending_tool.clone();
    let status = view.status.clone();
    let final_text = view.final_text.clone();
    let actions = view.action_count;
    state.remote.run = Some(view);
    match status.as_str() {
        "awaiting_tool" => match pending {
            Some(tool) => handle_tool(state, app, run_id, tool),
            None => fail(state, "The run is waiting for a tool but named none.", None),
        },
        "completed" => {
            state.remote.approval = None;
            state.message(false, final_text.unwrap_or_else(|| "Done.".into()));
            state.status = Some(format!("Completed | {actions} completed action(s)"));
            let _ = send(state, app, Method::Get, "/api/rules".into(), None, Purpose::Rules);
            if std::mem::take(&mut state.remote.speak_next) && state.remote.speech_tts {
                let text = spoken_summary(state.messages.last().map_or("", |m| m.text.as_str()));
                if !text.is_empty() {
                    let _ = send(state, app, Method::Post, "/api/speech/synthesize".into(), Some(json!({"text": text})), Purpose::Synthesize);
                }
            }
        }
        "interrupted" | "failed" => {
            state.remote.approval = None;
            let text = final_text.unwrap_or_else(|| "The run stopped.".into());
            let resume = RunRequest { method: Method::Post, path: format!("/api/runs/{run_id}/resume"), body: None, attempt: 0 };
            fail(state, format!("{text} Retry resumes the same run."), Some(resume));
        }
        _ => {
            state.status = Some("The run is still in progress on the server.".into());
            state.remote.retry = Some(RunRequest { method: Method::Get, path: format!("/api/runs/{run_id}"), body: None, attempt: 0 });
        }
    }
}

fn success(value: Json) -> ToolOutcome {
    if serde_json::to_vec(&value).map_or(true, |bytes| bytes.len() > MAX_OUTCOME_BYTES) {
        return failure("result_too_large");
    }
    ToolOutcome { ok: true, result: Some(value), error: None }
}

fn failure(code: &str) -> ToolOutcome {
    ToolOutcome { ok: false, result: None, error: Some(code.into()) }
}

fn handle_tool(state: &mut State, app: &mut SheetApp, run_id: String, tool: PendingTool) {
    let context = state.bridge.observe(&app.session);
    let outcome = match (tool.name.as_str(), context) {
        (_, Err(error)) => failure(error.code),
        ("get_context", Ok(context)) => success(json!(context)),
        ("inspect_range", Ok(context)) => {
            let range = tool.arguments.get("range").and_then(Json::as_str).unwrap_or("A1").to_string();
            match state.bridge.handle(&mut app.session, "inspect", json!({"expected": context.identity, "range": range})) {
                Ok(readback) => success(readback),
                Err(error) => failure(error.code),
            }
        }
        ("write_range" | "write_sales_review", Ok(_)) if tool.write.is_none() => failure("missing_write_plan"),
        ("undo_last" | "write_range" | "write_sales_review", Ok(context)) => {
            let text = describe(&tool);
            state.remote.approval = Some(Approval { run_id, tool, expected: context.identity });
            state.remote.approval_stale = false;
            state.message(false, text);
            state.status = Some("Preview only. Apply writes it to the live workbook; Discard leaves the workbook unchanged.".into());
            return;
        }
        _ => failure("unsupported_tool"),
    };
    send_outcome(state, app, &run_id, tool.tool_call_id, outcome);
}

fn describe(tool: &PendingTool) -> String {
    match &tool.write {
        Some(plan) => format!(
            "Proposed change: write {} on {}. Nothing changes until you Apply; existing cells in that range will be replaced.",
            target_range(plan).unwrap_or_else(|| plan.range.clone()),
            plan.sheet
        ),
        None => "Proposed change: undo the last workbook operation. This may undo a manual edit.".into(),
    }
}

fn send_outcome(state: &mut State, app: &mut SheetApp, run_id: &str, tool_call_id: String, outcome: ToolOutcome) {
    match serde_json::to_value(RunContinue { tool_call_id, outcome }) {
        Ok(body) => {
            dispatch(state, app, RunRequest { method: Method::Post, path: format!("/api/runs/{run_id}/continue"), body: Some(body), attempt: 0 })
        }
        Err(_) => fail(state, "Couldn't encode the tool result.", None),
    }
}

/// The exact rectangle a plan writes: its range, or the values' size anchored at its cell.
pub fn target_range(plan: &WritePlan) -> Option<String> {
    if plan.range.contains(':') {
        return Some(plan.range.clone());
    }
    let start = CellRef::parse(&plan.range)?;
    let height = u32::try_from(plan.values.len()).ok()?.checked_sub(1)?;
    let width = u32::try_from(plan.values.first()?.len()).ok()?.checked_sub(1)?;
    let end = CellRef::new(start.row.checked_add(height)?, start.col.checked_add(width)?);
    Some(format!("{}:{}", start.a1(), end.a1()))
}

fn bridge_id(tool_call_id: &str) -> String {
    let id: String = tool_call_id.chars().take(120).map(|ch| if ch.is_ascii_alphanumeric() || "-_.:".contains(ch) { ch } else { '_' }).collect();
    format!("tool:{id}")
}

fn verified(result: &Json, plan: &WritePlan) -> bool {
    if result.get("applied").and_then(Json::as_bool) != Some(true) || result.get("verification_error").is_some() {
        return false;
    }
    let Some(cells) = result.pointer("/readback/cells").and_then(Json::as_array) else { return false };
    let expected: Vec<&Json> = plan.values.iter().flatten().collect();
    cells.len() == expected.len()
        && expected.iter().zip(cells).all(|(want, cell)| {
            let value = cell.get("value").unwrap_or(&Json::Null);
            match want {
                Json::String(text) if text.starts_with('=') => cell.get("formula").and_then(Json::as_str).is_some(),
                Json::String(text) if text.is_empty() => true,
                Json::String(_) => !value.is_null(),
                Json::Number(number) => value.as_f64().zip(number.as_f64()).is_some_and(|(a, b)| (a - b).abs() <= 1e-9),
                Json::Bool(flag) => value.as_bool() == Some(*flag),
                _ => true,
            }
        })
}

pub fn submit(state: &mut State, app: &mut SheetApp, text: String) -> Result<Json, String> {
    if state.remote.pending_open.is_some() {
        return Err("Wait for the workbook to finish opening before asking the agent.".into());
    }
    if busy(state) || state.remote.approval.is_some() {
        return Err("Apply or discard the pending proposal, or wait for the current request, before sending another.".into());
    }
    if app.host_busy() {
        return Err("Finish or cancel the current cell edit or dialog before using this action. Your draft was kept.".into());
    }
    let context = state.bridge.observe(&app.session).map_err(|error| error.to_string())?;
    let api = app.services.agent_api.as_mut().ok_or("the agent service is not available in this build")?;
    let request_id = api.new_id();
    let workbook_session_id = api.session_id();
    let attachments = state
        .remote
        .attachments
        .iter()
        .filter_map(|file| Some(AttachmentRef { workbook_id: file.workbook_id.clone(), version: file.latest_version, sha256: file.sha256.clone()? }))
        .collect();
    let start = RunStart { request_id: request_id.clone(), workbook_session_id, text, attachments, context };
    let body = serde_json::to_value(start).map_err(|error| error.to_string())?;
    state.card_visible = true;
    state.error = None;
    state.remote.run = None;
    state.remote.retry = None;
    state.remote.actions.clear();
    state.remote.attachments.clear();
    state.remote.speak_next = std::mem::take(&mut state.remote.voice_draft);
    state.remote.spoken = None;
    state.prompt.clear();
    dispatch(state, app, RunRequest { method: Method::Post, path: "/api/runs/start".into(), body: Some(body), attempt: 0 });
    Ok(json!({"started": true, "request_id": request_id}))
}

pub fn apply(state: &mut State, app: &mut SheetApp) -> Result<Json, String> {
    let approval = state.remote.approval.clone().ok_or("there is no pending proposal")?;
    if busy(state) {
        return Err("Wait for the current agent request to finish.".into());
    }
    state.card_visible = true;
    state.remote.approval = None;
    state.remote.approval_stale = false;
    let tool_call_id = approval.tool.tool_call_id.clone();
    if app.host_busy() {
        state.message(false, "Nothing was written: finish or cancel the current cell edit or dialog first. Your draft was kept.");
        send_outcome(state, app, &approval.run_id, tool_call_id, failure("busy"));
        return Ok(json!({"outcome": "busy"}));
    }
    let current = state.bridge.observe(&app.session).ok().map(|context| context.identity);
    if current.as_ref() != Some(&approval.expected) {
        state.message(
            false,
            "Nothing was written: the workbook changed after this proposal was made. Your edits are kept; ask again for a fresh proposal.",
        );
        send_outcome(state, app, &approval.run_id, tool_call_id, failure("stale_workbook"));
        return Ok(json!({"outcome": "stale_workbook"}));
    }
    let request_id = bridge_id(&tool_call_id);
    let (outcome, label) = match &approval.tool.write {
        None => {
            let range = state.remote.last_range.clone().unwrap_or_else(|| "A1".into());
            match state.bridge.handle(&mut app.session, "undo", json!({"request_id": request_id, "expected": approval.expected, "range": range})) {
                Ok(result) if result.get("applied").and_then(Json::as_bool) == Some(true) => {
                    state.remote.actions.push("Undid the last workbook operation".into());
                    (success(json!({"undone": true, "verified": result.get("verification_error").is_none()})), "undone")
                }
                Ok(_) => (failure("not_applied"), "not_applied"),
                Err(error) => (failure(error.code), error.code),
            }
        }
        Some(plan) if plan.sheet != approval.expected.sheet_name => {
            state.message(false, format!("Nothing was written: the proposal targets {} but {} is active.", plan.sheet, approval.expected.sheet_name));
            (failure("wrong_sheet"), "wrong_sheet")
        }
        Some(plan) => match target_range(plan) {
            None => (failure("invalid_write_plan"), "invalid_write_plan"),
            Some(range) => {
                let params = json!({"request_id": request_id, "expected": approval.expected, "range": range, "values": plan.values});
                match state.bridge.handle(&mut app.session, "set_values", params) {
                    Ok(result) if verified(&result, plan) => {
                        state.remote.last_range = Some(range.clone());
                        state.remote.actions.push(format!("Wrote {range} on {}", plan.sheet));
                        if state.remote.actions.len() > MAX_ACTIONS {
                            state.remote.actions.remove(0);
                        }
                        (success(json!({"applied": true, "verified": true, "sheet": plan.sheet, "range": range})), "applied")
                    }
                    Ok(_) => {
                        state.message(false, "The workbook changed, but the readback could not be verified. Check the cells; Undo is available.");
                        (failure("verification_failed"), "verification_failed")
                    }
                    Err(error) if error.code == "conflict" => (failure("stale_workbook"), "stale_workbook"),
                    Err(error) => (failure(error.code), error.code),
                }
            }
        },
    };
    app.grid.ensure_visible = true;
    app.after_engine();
    send_outcome(state, app, &approval.run_id, tool_call_id, outcome);
    Ok(json!({"outcome": label}))
}

pub fn discard(state: &mut State, app: &mut SheetApp) -> Result<Json, String> {
    let approval = state.remote.approval.take().ok_or("there is no pending proposal")?;
    if busy(state) {
        state.remote.approval = Some(approval);
        return Err("Wait for the current agent request to finish.".into());
    }
    state.remote.approval_stale = false;
    state.message(false, "Discarded. Nothing was written.");
    send_outcome(state, app, &approval.run_id, approval.tool.tool_call_id, failure("user_declined"));
    Ok(json!({"outcome": "user_declined"}))
}

pub fn retry(state: &mut State, app: &mut SheetApp) -> Result<Json, String> {
    if busy(state) {
        return Err("A request is already in flight.".into());
    }
    let request = state.remote.retry.take().ok_or("there is nothing to retry")?;
    state.error = None;
    dispatch(state, app, request);
    Ok(json!({"retrying": true}))
}

pub fn load_rules(state: &mut State, app: &mut SheetApp) {
    if connected(state) {
        let _ = send(state, app, Method::Get, "/api/rules".into(), None, Purpose::Rules);
    }
}

pub fn search_files(state: &mut State, app: &mut SheetApp, query: &str) -> Result<Json, String> {
    if !connected(state) {
        return Err(LIBRARY_OFFLINE.into());
    }
    if query.len() > MAX_QUERY_BYTES {
        return Err("keep the search below 200 bytes".into());
    }
    state.remote.files_query = query.into();
    send(state, app, Method::Get, format!("/api/files?q={}", query_component(query.trim())), None, Purpose::Files)?;
    Ok(json!({"searching": query}))
}

fn card(state: &State, params: &Json) -> Result<FileCard, String> {
    let id = params.get("workbook_id").and_then(Json::as_str).ok_or("workbook_id is required")?;
    let card = state.remote.files.iter().find(|file| file.workbook_id == id).cloned().ok_or("unknown file; search the library first")?;
    if !safe_id(&card.workbook_id) {
        return Err("the library returned an invalid file identifier".into());
    }
    if !card.supported() {
        return Err("Unsupported format".into());
    }
    Ok(card)
}

pub fn preview_file(state: &mut State, app: &mut SheetApp, params: &Json) -> Result<Json, String> {
    let file = card(state, params)?;
    state.remote.file_preview = None;
    send(state, app, Method::Get, format!("/api/files/{}/versions/{}/preview", file.workbook_id, file.latest_version), None, Purpose::Preview)?;
    Ok(json!({"previewing": file.title, "version": file.latest_version}))
}

pub fn open_file(state: &mut State, app: &mut SheetApp, params: &Json) -> Result<Json, String> {
    let file = card(state, params)?;
    let path = format!("/api/workbooks/{}/snapshots/{}", file.workbook_id, file.latest_version);
    let binding = CloudBinding { workbook_id: file.workbook_id.clone(), version: file.latest_version, title: file.title.clone(), document_id: None };
    send(state, app, Method::Get, path, None, Purpose::Open { binding })?;
    Ok(json!({"opening": file.title, "version": file.latest_version}))
}

pub fn attach(state: &mut State, params: &Json) -> Result<Json, String> {
    let file = card(state, params)?;
    if file.sha256.is_none() {
        return Err("this file has no committed version to attach".into());
    }
    if !state.remote.attachments.iter().any(|attached| attached.workbook_id == file.workbook_id) {
        if state.remote.attachments.len() >= MAX_ATTACHMENTS {
            return Err(format!("attach at most {MAX_ATTACHMENTS} files"));
        }
        state.remote.attachments.push(file.clone());
    }
    Ok(json!({"attached": file.title, "version": file.latest_version}))
}

pub fn detach(state: &mut State, params: &Json) -> Result<Json, String> {
    let id = params.get("workbook_id").and_then(Json::as_str).ok_or("workbook_id is required")?;
    let before = state.remote.attachments.len();
    state.remote.attachments.retain(|file| file.workbook_id != id);
    Ok(json!({"removed": before != state.remote.attachments.len()}))
}

fn xlsx_name(title: &str) -> String {
    let lower = title.to_ascii_lowercase();
    if lower.ends_with(".xlsx") { title.to_string() } else { format!("{}.xlsx", title.trim_end_matches(".xlsm")) }
}

/// Save the active workbook as a new committed cloud version: capture once, export those exact
/// bytes, upload with the expected version, and mark saved only after the committed receipt
/// and the editor's acknowledgement of that same capture. Later edits stay unsaved.
pub fn save(state: &mut State, app: &mut SheetApp) -> Result<Json, String> {
    if !connected(state) {
        return Err("Cloud saving needs the GridCraft service.".into());
    }
    if saving(state) {
        return Err("A save is already in progress.".into());
    }
    if let Some(pending) = state.remote.save.clone() {
        state.remote.save_error = None;
        state.remote.save = Some(PendingSave { attempt: 0, ..pending.clone() });
        upload(state, app);
        return Ok(json!({"retrying": pending.export_id}));
    }
    if app.host_busy() {
        return Err("Finish or cancel the current cell edit or dialog before saving. Your draft was kept.".into());
    }
    let document_id = app.session.active().map(|doc| doc.uid).ok_or("no workbook is open")?;
    let display = app.session.active().map(|doc| doc.display_title()).unwrap_or_else(|| "Workbook".into());
    let identity = state.bridge.observe(&app.session).map_err(|error| error.to_string())?.identity;
    let binding = active_cloud(state, app).cloned();
    let api = app.services.agent_api.as_mut().ok_or("the agent service is not available in this build")?;
    let export_id = format!("export-{}", api.new_id());
    let workbook_session_id = api.session_id();
    let (workbook_id, expected_version, title, create) = match binding {
        Some(binding) => (binding.workbook_id, binding.version, binding.title, false),
        None => (api.new_id(), 0, xlsx_name(&display), true),
    };
    if !safe_id(&workbook_id) {
        return Err("couldn't create a workbook identifier".into());
    }
    let captured = state.bridge.capture_snapshot(&app.session, &export_id, &identity).map_err(|error| error.to_string())?;
    let bytes = match gridcraft_engine::io::save_bytes(&captured.workbook, &title) {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = state.bridge.discard_snapshot(&export_id);
            return Err(format!("Couldn't export the workbook: {error}"));
        }
    };
    let metadata = json!({"export_id": export_id, "workbook_session_id": workbook_session_id, "identity": captured.identity, "expected_version": expected_version});
    state.remote.save = Some(PendingSave {
        workbook_id: workbook_id.clone(),
        export_id: export_id.clone(),
        title: title.clone(),
        metadata,
        bytes,
        document_id,
        attempt: 0,
    });
    state.remote.save_error = None;
    state.status = Some(format!("Saving {title} to the cloud library."));
    if create {
        match send(state, app, Method::Put, format!("/api/workbooks/{workbook_id}"), Some(json!({"title": title})), Purpose::CreateWorkbook) {
            Ok(ticket) => state.remote.save_ticket = Some(ticket),
            Err(error) => abandon_save(state, error),
        }
    } else {
        upload(state, app);
    }
    Ok(json!({"saving": title, "export_id": export_id, "expected_version": expected_version}))
}

fn upload(state: &mut State, app: &mut SheetApp) {
    let Some(save) = state.remote.save.clone() else { return };
    let upload = Upload { metadata: save.metadata.to_string(), file_name: save.title.clone(), bytes: save.bytes.clone() };
    let path = format!("/api/workbooks/{}/snapshots", save.workbook_id);
    match send_upload(state, app, Method::Post, path, None, Some(upload), Purpose::Upload) {
        Ok(ticket) => state.remote.save_ticket = Some(ticket),
        Err(error) => abandon_save(state, error),
    }
}

/// The export can never be acknowledged: release the capture. The workbook stays edited.
fn abandon_save(state: &mut State, message: String) {
    state.remote.save_ticket = None;
    if let Some(save) = state.remote.save.take() {
        let _ = state.bridge.discard_snapshot(&save.export_id);
    }
    state.status = Some(message.clone());
    state.remote.save_error = Some(message);
}

fn receive_upload(state: &mut State, app: &mut SheetApp, response: ApiResponse) {
    state.remote.save_ticket = None;
    let Some(save) = state.remote.save.clone() else { return };
    if response.status == 0 || (500..=599).contains(&response.status) {
        let attempt = save.attempt.saturating_add(1);
        if response.status == 0 && attempt < MAX_ATTEMPTS {
            let delay = 0.5 * f64::from(1u32 << attempt.min(6));
            state.remote.save = Some(PendingSave { attempt, ..save });
            state.remote.save_retry_at = Some(state.remote.now + delay);
            state.status = Some(format!("Connection lost while saving; retrying the same export in {delay:.0} s."));
            return;
        }
        let message = "Not saved yet: the library could not be reached. Save again retries the same export; your edits are kept.".to_string();
        state.status = Some(message.clone());
        state.remote.save_error = Some(message);
        return;
    }
    if response.status == 409 {
        return abandon_save(
            state,
            "Not saved: a newer version of this workbook exists (version conflict). Your edits are kept; open the latest version from Files to continue.".into(),
        );
    }
    if !response.ok() {
        return abandon_save(state, format!("Not saved: the library refused the upload (HTTP {}: {}).", response.status, error_code(&response.body)));
    }
    let receipt: Json = serde_json::from_slice(&response.body).unwrap_or(Json::Null);
    let version = receipt.get("storage_version").and_then(Json::as_i64);
    let matches = receipt.get("export_id") == save.metadata.get("export_id")
        && receipt.get("identity") == save.metadata.get("identity")
        && receipt.get("workbook_id").and_then(Json::as_str) == Some(save.workbook_id.as_str());
    let Some(version) = version.filter(|_| matches) else {
        return abandon_save(state, "Not marked saved: the library returned a receipt for a different export.".into());
    };
    state.remote.save = None;
    let binding = CloudBinding { workbook_id: save.workbook_id.clone(), version, title: save.title.clone(), document_id: Some(save.document_id) };
    if let Ok(text) = serde_json::to_string(&binding) {
        persist(app, STORE_WORKBOOK, Some(&text));
    }
    state.remote.cloud = Some(binding);
    match state.bridge.acknowledge_snapshot(&mut app.session, &save.export_id) {
        Ok(ack) if ack.clean => state.status = Some(format!("Saved version {version}.")),
        Ok(_) => state.status = Some(format!("Saved version {version}. Later edits are not saved yet.")),
        Err(error) => {
            let message =
                format!("Version {version} was stored, but this editor couldn't confirm it ({}); the workbook stays marked as edited.", error.code);
            state.status = Some(message.clone());
            state.remote.save_error = Some(message);
        }
    }
}

/// The first sentence (or first 280 characters) of a completed, verified answer.
pub fn spoken_summary(text: &str) -> String {
    let text = text.trim();
    let end = text.char_indices().find(|(i, ch)| matches!(ch, '.' | '!' | '?') && text[i + ch.len_utf8()..].starts_with(' ')).map(|(i, _)| i + 1);
    let sentence = end.map_or(text, |end| &text[..end]);
    sentence.chars().take(MAX_SPOKEN_CHARS).collect()
}

pub fn start_voice(state: &mut State, app: &mut SheetApp) -> Result<Json, String> {
    if !connected(state) || !state.remote.speech_stt {
        return Err("Voice input needs the GridCraft service with speech configured.".into());
    }
    if state.remote.recording || state.remote.transcribing {
        return Err("A recording is already in progress.".into());
    }
    let remote = &mut state.remote;
    remote.next_ticket = remote.next_ticket.checked_add(1).ok_or("request identifier limit reached")?;
    let ticket = remote.next_ticket;
    let api = app.services.agent_api.as_mut().ok_or("the agent service is not available in this build")?;
    api.start_recording(ticket)?;
    state.remote.tickets.insert(ticket, Purpose::Recording);
    state.remote.record_ticket = Some(ticket);
    state.remote.recording = true;
    state.remote.record_started = state.remote.now;
    state.status = Some("Listening... Stop to transcribe, or Cancel.".into());
    Ok(json!({"recording": true}))
}

pub fn stop_voice(state: &mut State, app: &mut SheetApp) -> Result<Json, String> {
    if !state.remote.recording {
        return Err("Nothing is being recorded.".into());
    }
    if let Some(api) = app.services.agent_api.as_mut() {
        api.stop_recording();
    }
    state.status = Some("Finishing the recording...".into());
    Ok(json!({"stopping": true}))
}

pub fn cancel_voice(state: &mut State, app: &mut SheetApp) -> Result<Json, String> {
    if let Some(ticket) = state.remote.record_ticket.take() {
        state.remote.tickets.remove(&ticket);
    }
    if let Some(api) = app.services.agent_api.as_mut() {
        api.cancel_recording();
    }
    state.remote.recording = false;
    state.status = Some("Recording cancelled. Your draft was kept.".into());
    Ok(json!({"cancelled": true}))
}

pub fn speak(state: &mut State, app: &mut SheetApp) -> Result<Json, String> {
    let audio = state.remote.spoken.clone().ok_or("there is no spoken summary to play")?;
    app.services.agent_api.as_mut().ok_or("playback is unavailable")?.play(audio)?;
    Ok(json!({"playing": true}))
}

pub fn stop_speaking(app: &mut SheetApp) -> Result<Json, String> {
    if let Some(api) = app.services.agent_api.as_mut() {
        api.stop_playback();
    }
    Ok(json!({"stopped": true}))
}
