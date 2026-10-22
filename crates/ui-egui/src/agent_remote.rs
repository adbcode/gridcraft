//! Connected agent mode. The durable server run decides which tool to call; the browser runs
//! reads at once through the live bridge and writes only after Apply, against the workbook
//! identity captured when the proposal arrived. The transport is the host's `AgentApi`.

use std::collections::BTreeMap;

use gridcraft_engine::agent_bridge::Identity;
use gridcraft_engine::agent_protocol::{AttachmentRef, MAX_ATTACHMENTS, PendingTool, RunContinue, RunStart, RunView, ToolOutcome, WritePlan};
use gridcraft_engine::core::CellRef;
use serde::Deserialize;
use serde_json::{Value as Json, json};

use crate::SheetApp;
use crate::agent::State;
use crate::agent_api::{ApiRequest, ApiResponse, Method, query_component};

/// Network failures are retried with the identical body, at most this many attempts in total.
pub const MAX_ATTEMPTS: u32 = 3;
const MAX_FILES: usize = 64;
const MAX_ACTIONS: usize = 40;
const MAX_RULES: usize = 32;
const MAX_QUERY_BYTES: usize = 200;
const MAX_OUTCOME_BYTES: usize = 60_000;
const SUPPORTED: &[&str] = &["xlsx", "xlsm", "xls", "xlsb", "ods", "csv", "tsv"];
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
    Open { title: String },
    Rules,
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
    state.remote.run_ticket.is_some() || state.remote.retry_at.is_some()
}

pub fn retry_pending(state: &State) -> bool {
    state.remote.retry_at.is_some()
}

pub fn observe(state: &mut State, current: Option<&Identity>) {
    state.remote.approval_stale = state.remote.approval.as_ref().is_some_and(|approval| current != Some(&approval.expected));
}

fn send(state: &mut State, app: &mut SheetApp, method: Method, path: String, body: Option<Json>, purpose: Purpose) -> Result<u64, String> {
    let api = app.services.agent_api.as_mut().ok_or("the agent service is not available in this build")?;
    let remote = &mut state.remote;
    remote.next_ticket = remote.next_ticket.checked_add(1).ok_or("request identifier limit reached")?;
    let ticket = remote.next_ticket;
    remote.tickets.insert(ticket, purpose);
    api.send(ApiRequest { ticket, method, path, body });
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
        Purpose::Open { title } => {
            if !response.ok() || response.body.is_empty() {
                state.remote.files_error = Some(format!("Couldn't open {title} (HTTP {}).", response.status));
                return;
            }
            match &app.services.inbox {
                Some(inbox) => {
                    inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((title.clone(), response.body));
                    state.status = Some(format!("Opening {title}."));
                }
                None => state.remote.files_error = Some("This build cannot open downloaded files.".into()),
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
    send(state, app, Method::Get, path, None, Purpose::Open { title: file.title.clone() })?;
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
