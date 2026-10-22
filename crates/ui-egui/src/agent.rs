//! Same-session workspace UI and commands. Browser requests dispatch through the trusted
//! service; standalone native mode retains explicitly labelled deterministic demo intents.
//! Workbook effects always use the mounted live bridge.

use gridcraft_engine::agent_bridge::{BridgeState, Identity};
use gridcraft_engine::review_workflow::{ComparisonPeriods, ReviewRule, SalesReview, compare_sales, demo_sales};
use gridcraft_engine::workspace::{AttachmentVersion, LibraryFile, RunView, ToolDispatch, review_values};
use serde_json::{Value as Json, json};

use crate::{SheetApp, theme::Tokens};

const MAX_MESSAGES: usize = 40;
const MAX_PROMPT_BYTES: usize = 2_000;

pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("ui.agent.toggle", "Classic / Agentic Mode", "{on?: bool}; retains the live workbook and history"),
    ("ui.agent.prompt", "Edit Agent Prompt", "{text: string}; session-local draft"),
    ("ui.agent.submit", "Submit Local Demo Request", "{text?: string}; compare sales or remember to exclude test orders"),
    ("ui.agent.remember", "Remember Test Order Exclusion", "{}; explicit session-only rule"),
    ("ui.agent.compare", "Preview Demo Sales Comparison", "{}; synthetic fixed UTC weeks, no workbook mutation"),
    ("ui.agent.apply", "Apply Sales Preview", "{}; validated single operation against the preview identity"),
    ("ui.agent.undo", "Undo Last Native Operation", "{}; may undo a manual edit"),
    ("ui.agent.files", "Browse Mock Cloud Files", "{}"),
    ("ui.agent.openMock", "Open Local Mock Workbook", "{id: string}"),
    ("ui.agent.rules", "Show Working Rules", "{}"),
    ("ui.agent.save", "Save Exact Workbook Snapshot", "{}"),
    ("ui.agent.dismiss", "Dismiss Completed Response", "{}; unresolved previews and errors stay visible"),
    ("ui.agent.mic", "Record Voice Request", "{}; click again to finish and edit transcript"),
    ("ui.agent.cancelRecording", "Cancel Recording", "{}"),
    ("ui.agent.play", "Play Verified Outcome", "{}"),
    ("ui.agent.discard", "Discard Sales Preview", "{}; leaves workbook unchanged"),
];

#[derive(Clone, Debug)]
pub struct Message {
    pub from_user: bool,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Preview {
    pub expected: Identity,
    pub range: String,
    pub review: SalesReview,
}

#[derive(Clone, Debug)]
pub enum WorkspaceAction {
    Submit(String),
    Files,
    Open(AttachmentVersion),
    Save,
    Record,
    StopRecording,
    CancelRecording,
    Play,
    Resume,
    Continue(gridcraft_engine::workspace::ToolContinuation),
}

#[derive(Default)]
pub struct State {
    pub enabled: bool,
    pub prompt: String,
    pub rules: Vec<ReviewRule>,
    pub preview: Option<Preview>,
    pub messages: Vec<Message>,
    pub receipts: Vec<Json>,
    pub status: Option<String>,
    pub preview_stale: bool,
    pub bridge: BridgeState,
    pub connected: bool,
    pub connection: String,
    pub outbox: Vec<WorkspaceAction>,
    pub files: Vec<LibraryFile>,
    pub attachments: Vec<AttachmentVersion>,
    pub files_open: bool,
    pub rules_open: bool,
    pub file_search: String,
    pub composer_expanded: bool,
    pub response_dismissed: bool,
    pub error: bool,
    pub pending_tool: Option<ToolDispatch>,
    pub run: Option<RunView>,
    pub working: bool,
    pub recording: bool,
    pub speech_status: String,
    pub save_status: String,
    next_id: u64,
    last_range: Option<String>,
}

impl State {
    pub fn message(&mut self, from_user: bool, text: impl Into<String>) {
        self.response_dismissed = false;
        self.messages.push(Message { from_user, text: text.into() });
        if self.messages.len() > MAX_MESSAGES {
            self.messages.remove(0);
        }
    }

    fn next_id(&mut self) -> Result<String, String> {
        self.next_id = self.next_id.checked_add(1).ok_or("local action identifier limit reached")?;
        Ok(format!("local-{}", self.next_id))
    }

    fn receipt(&mut self, value: Json) {
        self.receipts.push(value);
        if self.receipts.len() > MAX_MESSAGES {
            self.receipts.remove(0);
        }
    }
}

/// Observe every frame, including Classic mode, so hiding the panel never weakens conflicts.
pub fn observe(app: &mut SheetApp) {
    let current = app.agent.bridge.observe(&app.session);
    app.agent.preview_stale =
        app.agent.preview.as_ref().is_some_and(|preview| current.as_ref().map_or(true, |context| context.identity != preview.expected));
}

pub fn command(app: &mut SheetApp, id: &str, params: &Json) -> Result<Json, String> {
    let mut state = std::mem::take(&mut app.agent);
    let outcome = execute(&mut state, app, id, params);
    if let Err(error) = &outcome {
        state.error = true;
        state.status = Some(error.clone());
        state.message(false, error.clone());
    }
    app.agent = state;
    outcome
}

fn execute(state: &mut State, app: &mut SheetApp, id: &str, params: &Json) -> Result<Json, String> {
    if !params.is_object() {
        return Err("agent command params must be an object".into());
    }
    if state.connected && matches!(id, "ui.agent.submit" | "ui.agent.remember" | "ui.agent.compare") {
        if state.working || state.pending_tool.is_some() || state.preview.is_some() || state.error {
            return Err("Handle the outstanding request, preview or error first.".into());
        }
        let text = match id {
            "ui.agent.remember" => "Remember to exclude test orders".to_string(),
            "ui.agent.compare" => "Compare sales from the workspace_sales_records dataset for the two documented completed UTC weeks. Preview a summary in EUR; do not apply until I approve.".to_string(),
            _ => params.get("text").and_then(Json::as_str).unwrap_or(&state.prompt).trim().to_string(),
        };
        if text.is_empty() || text.len() > MAX_PROMPT_BYTES {
            return Err("Enter a request of 1 to 2,000 bytes.".into());
        }
        require_ready(app)?;
        state.message(true, text.clone());
        state.outbox.push(WorkspaceAction::Submit(text));
        state.working = true;
        state.error = false;
        state.prompt.clear();
        return Ok(json!({"submitted":true}));
    }
    match id {
        "ui.agent.toggle" => {
            state.enabled = params.get("on").and_then(Json::as_bool).unwrap_or(!state.enabled);
            Ok(json!({"agentic": state.enabled}))
        }
        "ui.agent.prompt" => {
            let text = params.get("text").and_then(Json::as_str).ok_or("text is required")?;
            if text.len() > MAX_PROMPT_BYTES {
                return Err("keep the local demo request below 2,000 bytes".into());
            }
            state.prompt = text.into();
            Ok(json!({"draft": true}))
        }
        "ui.agent.submit" => {
            let text = params.get("text").and_then(Json::as_str).unwrap_or(&state.prompt).trim().to_string();
            if text.is_empty() || text.len() > MAX_PROMPT_BYTES {
                return Err("enter a request of 1 to 2,000 bytes".into());
            }
            state.message(true, text.clone());
            let normalized = text.trim_end_matches(['.', '?', '!']).to_ascii_lowercase();
            match normalized.as_str() {
                "remember to exclude test orders" | "exclude test orders" => remember(state),
                "compare sales" | "compare weekly sales" | "compare the two periods" | "compare demo sales" => compare(state, app),
                _ => {
                    Err("Local demo supports 'Compare sales' and 'Remember to exclude test orders'. Free-form AI and voice are not connected.".into())
                }
            }
        }
        "ui.agent.remember" => remember(state),
        "ui.agent.compare" => compare(state, app),
        "ui.agent.apply" if state.pending_tool.is_some() => {
            require_ready(app)?;
            let pending = state.pending_tool.clone().ok_or("No pending tool")?;
            let result = state.bridge.handle(&mut app.session, &pending.method, pending.params.clone());
            let result = match result {
                Ok(result) => result,
                Err(error) => {
                    state.error = true;
                    state.status = Some(error.to_string());
                    state.message(false, error.to_string());
                    json!({"error":error.to_string()})
                }
            };
            if result.get("applied").and_then(Json::as_bool) == Some(true) {
                state.receipt(result.clone());
            }
            let session = state.run.as_ref().ok_or("Run context missing")?.workbook_session_id.clone();
            state.outbox.push(WorkspaceAction::Continue(gridcraft_engine::workspace::ToolContinuation {
                workbook_session_id: session,
                tool_call_id: pending.tool_call_id,
                result,
            }));
            state.pending_tool = None;
            state.working = true;
            app.after_engine();
            Ok(json!({"submitted":true}))
        }
        "ui.agent.apply" => apply(state, app),
        "ui.agent.undo" => undo(state, app),
        "ui.agent.openMock" => {
            require_ready(app)?;
            let id = params.get("id").and_then(Json::as_str).ok_or("Mock file ID required")?;
            let title = gridcraft_engine::workspace::demo_catalog()
                .into_iter()
                .find(|(key, _)| *key == id)
                .map(|(_, title)| title)
                .ok_or("Unknown mock file")?;
            let bytes = gridcraft_engine::workspace::demo_xlsx(id).map_err(|e| e.to_string())?;
            let result = app.run("file.open", json!({"name":title,"base64":gridcraft_engine::io::base64_encode(&bytes)}));
            state.files_open = false;
            result
        }
        "ui.agent.files" => {
            if state.files.is_empty() && state.connection.is_empty() {
                state.files = gridcraft_engine::workspace::demo_catalog()
                    .into_iter()
                    .map(|(id, title)| LibraryFile {
                        workbook_id: id.into(),
                        title: title.into(),
                        latest_version: 1,
                        source: "local mock source (not cloud-saved)".into(),
                        preview: gridcraft_engine::workspace::demo_preview(id).unwrap_or_default(),
                    })
                    .collect();
            }
            state.files_open = !state.files_open;
            if state.connected {
                state.outbox.push(WorkspaceAction::Files);
            }
            Ok(json!({}))
        }
        "ui.agent.rules" => {
            state.rules_open = !state.rules_open;
            Ok(json!({}))
        }
        "ui.agent.save" => {
            require_ready(app)?;
            state.error = false;
            if state.connection.is_empty() {
                app.run("file.saveAs", json!({}))
            } else {
                state.outbox.push(WorkspaceAction::Save);
                Ok(json!({}))
            }
        }
        "ui.agent.dismiss" => {
            if state.preview.is_some() || state.pending_tool.is_some() || state.error {
                return Err("Handle the preview or error first.".into());
            }
            state.response_dismissed = true;
            Ok(json!({}))
        }
        "ui.agent.mic" => {
            if state.connection.is_empty() {
                return Err("Voice capture requires the browser workspace and configured speech service.".into());
            }
            if state.recording {
                state.outbox.push(WorkspaceAction::StopRecording);
            } else {
                state.outbox.push(WorkspaceAction::Record);
            }
            Ok(json!({}))
        }
        "ui.agent.cancelRecording" => {
            state.outbox.push(WorkspaceAction::CancelRecording);
            Ok(json!({}))
        }
        "ui.agent.play" => {
            state.outbox.push(WorkspaceAction::Play);
            Ok(json!({}))
        }
        "ui.agent.discard" => {
            if let Some(pending) = state.pending_tool.take() {
                let session = state.run.as_ref().ok_or("Run context missing")?.workbook_session_id.clone();
                state.outbox.push(WorkspaceAction::Continue(gridcraft_engine::workspace::ToolContinuation {
                    workbook_session_id: session,
                    tool_call_id: pending.tool_call_id,
                    result: json!({"cancelled":true,"applied":false}),
                }));
                state.working = true;
            }
            state.error = false;
            state.preview = None;
            state.preview_stale = false;
            state.status = Some("Preview discarded; workbook unchanged.".into());
            Ok(json!({"discarded": true}))
        }
        _ => Err(format!("unknown agent command {id}")),
    }
}

fn remember(state: &mut State) -> Result<Json, String> {
    if state.rules.is_empty() {
        state.rules.push(ReviewRule::from_instruction("exclude-tests-v1", "Remember to exclude test orders.").map_err(|e| e.to_string())?);
    }
    state.preview = None;
    state.preview_stale = false;
    state.status = Some("Rule remembered for this session. Compare again to use it.".into());
    state.message(false, "Remembered: exclude test orders. This rule is held in this session only and is lost when the app restarts.");
    Ok(json!({"rule_id": "exclude-tests-v1", "persistence": "session_only"}))
}

fn require_ready(app: &SheetApp) -> Result<(), String> {
    if app.host_busy() {
        return Err("Finish or cancel the current cell edit or dialog before using this action. Your draft was kept.".into());
    }
    Ok(())
}

fn compare(state: &mut State, app: &SheetApp) -> Result<Json, String> {
    require_ready(app)?;
    let context = state.bridge.observe(&app.session).map_err(|e| e.to_string())?;
    let query_id = state.next_id()?;
    let review = compare_sales(&demo_sales(), &state.rules, &query_id, ComparisonPeriods::demo()).map_err(|e| e.to_string())?;
    let range = format!("A1:E{}", review.sheet_values().len());
    let provenance = review.provenance().clone();
    state.message(
        false,
        format!(
            "Preview ready: {} to {} net revenue from paid demo orders. Apply to write {} on {}. Existing cells in that range will be replaced.",
            euros(review.total().previous_minor),
            euros(review.total().current_minor),
            range,
            context.identity.sheet_name,
        ),
    );
    state.preview = Some(Preview { expected: context.identity, range: range.clone(), review });
    state.preview_stale = false;
    state.status = Some("Preview only. Inspect the proposed cells and sources before applying.".into());
    Ok(json!({"preview": true, "range": range, "provenance": provenance}))
}

fn apply(state: &mut State, app: &mut SheetApp) -> Result<Json, String> {
    require_ready(app)?;
    let preview = state.preview.clone().ok_or("compare demo sales first to create a preview")?;
    let request_id = state.next_id()?;
    let result = state
        .bridge
        .handle(
            &mut app.session,
            "set_values",
            json!({
                "request_id":request_id, "expected":preview.expected, "range":preview.range, "values":review_values(&preview.review),
            }),
        )
        .map_err(|e| e.to_string())?;
    let verified = verify_review(&result, &preview.review);
    state.receipt(json!({"action":"apply", "verified":verified, "result":result}));
    state.preview = None;
    state.preview_stale = false;
    state.last_range = Some(preview.range.clone());
    app.grid.ensure_visible = true;
    app.after_engine();
    if verified {
        state.message(
            false,
            format!(
                "Applied and verified {} on {}. Total: {} to {}. The summary cells and formulas are editable; provenance is included below them.",
                preview.range,
                preview.expected.sheet_name,
                euros(preview.review.total().previous_minor),
                euros(preview.review.total().current_minor),
            ),
        );
        state.status = Some("Applied and verified in the live workbook. Not saved to a database.".into());
    } else {
        state.message(false, "The workbook changed, but the calculated results could not be fully verified. Check the action history and cells before continuing; Undo is available.");
        state.status = Some("Applied; verification incomplete. The workbook was not rolled back.".into());
    }
    Ok(json!({"verified":verified, "result":result}))
}

fn undo(state: &mut State, app: &mut SheetApp) -> Result<Json, String> {
    require_ready(app)?;
    let expected = state.bridge.observe(&app.session).map_err(|e| e.to_string())?.identity;
    let request_id = state.next_id()?;
    let range = state.last_range.clone().unwrap_or_else(|| "A1".into());
    let result = state
        .bridge
        .handle(&mut app.session, "undo", json!({"request_id":request_id,"expected":expected,"range":range}))
        .map_err(|e| e.to_string())?;
    state.receipt(json!({"action":"undo", "result":result}));
    state.preview = None;
    state.preview_stale = false;
    app.grid.ensure_visible = true;
    app.after_engine();
    let message = if result.get("verification_error").is_some() {
        "The last operation was undone; checking the affected range is incomplete. Review the action history and cells."
    } else {
        "Undid the last native operation and read the range back. This may have been a manual edit."
    };
    state.status = Some(message.into());
    state.message(false, message);
    Ok(result)
}

fn verify_review(result: &Json, review: &SalesReview) -> bool {
    if result.get("applied").and_then(Json::as_bool) != Some(true) || result.get("verification_error").is_some() {
        return false;
    }
    let Some(cells) = result.pointer("/readback/cells").and_then(Json::as_array) else { return false };
    let value = |cell: &str| cells.iter().find(|entry| entry.get("cell").and_then(Json::as_str) == Some(cell)).and_then(|entry| entry.get("value"));
    let number = |cell: &str, expected: f64| value(cell).and_then(Json::as_f64).is_some_and(|actual| (actual - expected).abs() <= 1e-9);
    review.categories().iter().chain(std::iter::once(review.total())).enumerate().all(|(i, category)| {
        let row = i.saturating_add(2);
        number(&format!("B{row}"), category.previous_minor as f64 / 100.0)
            && number(&format!("C{row}"), category.current_minor as f64 / 100.0)
            && number(&format!("D{row}"), category.delta_minor as f64 / 100.0)
            && match category.growth {
                Some(growth) => number(&format!("E{row}"), growth),
                None => value(&format!("E{row}")).and_then(Json::as_str) == Some("n/a"),
            }
    })
}

fn euros(cents: i64) -> String {
    format!("EUR {:.2}", cents as f64 / 100.0)
}

fn invoke(app: &mut SheetApp, id: &str, params: Json) {
    // Errors are visible in the conversation and status, without opening another dialog.
    let _ = app.run(id, params);
}

/// Mode selection never constructs another document or changes workbook history.
pub fn mode_bar(app: &mut SheetApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::top("agent_mode_bar").exact_size(42.0).frame(egui::Frame::NONE.fill(t.window).inner_margin(egui::Margin::symmetric(16, 6))).show(
        ui,
        |ui| {
            ui.horizontal(|ui| {
                if app.agent.enabled {
                    let title = app.session.active().map(|d| d.title.as_str()).unwrap_or("Workbook");
                    ui.label(egui::RichText::new(title).strong());
                    let dirty = app.session.active().is_some_and(|d| d.is_dirty());
                    ui.label(egui::RichText::new(if dirty { "Unsaved changes" } else { "No unsaved edits" }).small().color(t.text_dim));
                    for (label, id) in
                        [("Files", "ui.agent.files"), ("Rules", "ui.agent.rules"), ("Undo", "ui.agent.undo"), ("Save", "ui.agent.save")]
                    {
                        if ui.button(label).clicked() {
                            invoke(app, id, json!({}));
                        }
                    }
                } else {
                    ui.label(egui::RichText::new("Workspace").color(t.text_dim));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, on) = if app.agent.enabled { ("Classic", false) } else { ("Agentic", true) };
                    if ui.button(label).clicked() {
                        invoke(app, "ui.agent.toggle", json!({"on":on}));
                    }
                });
            });
        },
    );
}

pub fn show(app: &mut SheetApp, ui: &mut egui::Ui) {
    if !app.agent.enabled {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let ctx = ui.ctx().clone();
    if app.agent.files_open {
        let mut open = true;
        egui::Window::new("Files | Mock cloud library").open(&mut open).default_width(600.0).show(&ctx, |ui| {
            ui.label("Mock sources · original synthetic XLSX workbooks");
            ui.add(egui::TextEdit::singleline(&mut app.agent.file_search).hint_text("Search XLSX files").desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                if ui.button("Open local file").clicked() {
                    let _ = app.run("file.open", json!({}));
                }
                if ui.button("Download current XLSX").clicked() {
                    let _ = app.run("file.saveAs", json!({}));
                }
                if ui.button("Refresh").clicked() {
                    app.agent.outbox.push(WorkspaceAction::Files);
                }
            });
            ui.small("Document attachments support XLSX only. PDF, slides and text documents are unsupported.");
            let search = app.agent.file_search.to_lowercase();
            for file in app.agent.files.clone().into_iter().filter(|f| f.title.to_lowercase().contains(&search)) {
                ui.push_id((&file.workbook_id, file.latest_version), |ui| {
                    egui::Frame::group(ui.style()).inner_margin(12).show(ui, |ui| {
                        ui.set_min_width(520.0_f32.min(ui.available_width()));
                        ui.label(egui::RichText::new(&file.title).strong());
                        ui.small(format!("{} · version {}", file.source, file.latest_version));
                        ui.collapsing("Preview", |ui| {
                            for row in file.preview.iter().take(6) {
                                ui.label(
                                    row.iter()
                                        .map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))
                                        .collect::<Vec<_>>()
                                        .join("   |   "),
                                );
                            }
                        });
                        let version =
                            AttachmentVersion { workbook_id: file.workbook_id.clone(), version: file.latest_version, title: file.title.clone() };
                        ui.horizontal(|ui| {
                            if ui.button("Open").clicked() {
                                if app.agent.connection.is_empty() {
                                    invoke(app, "ui.agent.openMock", json!({"id":version.workbook_id}));
                                } else {
                                    app.agent.outbox.push(WorkspaceAction::Open(version.clone()));
                                }
                            }
                            if ui.button("Attach").clicked() && app.agent.attachments.len() < 8 && !app.agent.attachments.contains(&version) {
                                app.agent.attachments.push(version);
                            }
                        });
                    });
                });
            }
            if app.agent.files.is_empty() {
                ui.label("Library unavailable. Connect to the local service with a configured database, or open a local file.");
            }
        });
        app.agent.files_open = open;
    }
    if app.agent.rules_open {
        let mut open = true;
        egui::Window::new("Working rules").open(&mut open).show(&ctx, |ui| {
            ui.label(if app.agent.connected { "Saved service rules" } else { "Session-only demo rules" });
            for rule in &app.agent.rules {
                ui.label(rule.original_wording());
            }
            if app.agent.rules.is_empty() {
                ui.label("No exclusion rule saved.");
            }
            if ui.button("Remember: exclude test orders").clicked() {
                invoke(app, "ui.agent.remember", json!({}));
            }
        });
        app.agent.rules_open = open;
    }
    // Reserve a small strip so the floating composer never hides sheet navigation.
    egui::Panel::bottom("agent_composer_space").exact_size(if app.agent.composer_expanded { 186.0 } else { 110.0 }).show(ui, |_| {});
    let rect = ui.max_rect();
    let width = (rect.width() - 40.0).clamp(240.0, 780.0);
    egui::Area::new(egui::Id::new("agent_composer")).anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -66.0)).order(egui::Order::Foreground).show(
        &ctx,
        |ui| {
            egui::Frame::window(ui.style()).fill(t.window).corner_radius(16).inner_margin(14).show(ui, |ui| {
                ui.set_width(width);
                for attachment in app.agent.attachments.clone() {
                    ui.horizontal(|ui| {
                        ui.small(format!("{} · v{} · mock source", attachment.title, attachment.version));
                        if ui.small_button("Remove").clicked() {
                            app.agent.attachments.retain(|a| a != &attachment);
                        }
                    });
                }
                let response = ui.add(
                    egui::TextEdit::multiline(&mut app.agent.prompt)
                        .desired_rows(if app.agent.composer_expanded { 5 } else { 2 })
                        .desired_width(f32::INFINITY)
                        .hint_text("Ask about this workbook, or compare weekly sales…")
                        .char_limit(MAX_PROMPT_BYTES),
                );
                let submit = response.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button("Attach").clicked() {
                        invoke(app, "ui.agent.files", json!({}));
                    }
                    if ui.button(if app.agent.recording { "Finish recording" } else { "Microphone" }).clicked() {
                        invoke(app, "ui.agent.mic", json!({}));
                    }
                    if app.agent.recording && ui.button("Cancel recording").clicked() {
                        invoke(app, "ui.agent.cancelRecording", json!({}));
                    }
                    if ui.button(if app.agent.composer_expanded { "Collapse" } else { "Expand" }).clicked() {
                        app.agent.composer_expanded = !app.agent.composer_expanded;
                    }
                    if ui.add_enabled(!app.agent.working, egui::Button::new("Send")).clicked() || submit {
                        invoke(app, "ui.agent.submit", json!({}));
                    }
                    if app.agent.working {
                        ui.spinner();
                    }
                });
                if !app.agent.speech_status.is_empty() {
                    ui.small(&app.agent.speech_status);
                }
            });
        },
    );
    let show_card = !app.agent.response_dismissed || app.agent.preview.is_some() || app.agent.pending_tool.is_some() || app.agent.error;
    if show_card {
        egui::Panel::top("agent_response").exact_size((if app.agent.pending_tool.is_some() || app.agent.preview.is_some() {290.0_f32}else{210.0_f32}).min(ui.available_height()*0.4)).frame(egui::Frame::NONE.fill(t.grid_bg).inner_margin(12)).show(ui,|ui| {
            egui::ScrollArea::vertical().id_salt("agent_response_scroll").max_height(320.0).show(ui,|ui| {
            egui::Frame::group(ui.style()).fill(t.window).corner_radius(10).inner_margin(12).show(ui,|ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(if app.agent.connected {"GridCraft"}else{"GridCraft · Local demo"}).strong());
                    if app.agent.preview.is_none() && app.agent.pending_tool.is_none() && !app.agent.error && ui.small_button("Dismiss").clicked(){invoke(app,"ui.agent.dismiss",json!({}));}
                });
                if app.agent.working {ui.label("Working on your request…");}
                else if let Some(message)=app.agent.messages.iter().rev().find(|m|!m.from_user){ui.label(&message.text);} else {ui.label("Your spreadsheet is ready. Edit cells directly, ask a question, or compare the completed sales weeks.");}
                ui.small(&app.agent.connection);
                if app.agent.error && let Some(error)=&app.agent.status {ui.colored_label(t.danger,error);}
                if !app.agent.save_status.is_empty(){ui.small(&app.agent.save_status);}
                if let Some(preview)=app.agent.preview.clone(){
                    ui.label(format!("Total {} to {} · 5–12 Oct / 12–19 Oct 2026 UTC",euros(preview.review.total().previous_minor),euros(preview.review.total().current_minor)));
                    ui.small(format!("Replace {}!{} · editable EUR values and formulas",preview.expected.sheet_name,preview.range));
                    if app.agent.preview_stale {ui.colored_label(t.danger,"Workbook changed. Discard and compare again.");}
                }
                if let Some(source)=app.agent.run.as_ref().and_then(|run|run.sources.iter().rev().find(|source|source.get("review").is_some()))
                    && let (Some(previous),Some(current))=(source.pointer("/review/total/previous_minor").and_then(Json::as_i64),source.pointer("/review/total/current_minor").and_then(Json::as_i64)) {
                        ui.label(format!("Total {} to {}",euros(previous),euros(current)));
                        if let Some(rows)=source.get("sheet_values").and_then(Json::as_array) {
                            for label in ["Prior","Current"] {
                                if let Some(value)=rows.iter().find(|row|row.get(0).and_then(Json::as_str)==Some(label)).and_then(|row|row.get(1)).and_then(Json::as_str) {ui.small(format!("{label}: {value}"));}
                            }
                        }
                        ui.small("Backend mock sales · paid orders after refunds · EUR");
                }
                if let Some(pending)=app.agent.pending_tool.clone(){
                    ui.label(format!("Proposed {} · {}!{}",pending.method,pending.params.pointer("/expected/sheet_name").and_then(Json::as_str).unwrap_or(""),pending.params.get("range").and_then(Json::as_str).unwrap_or("")));
                    ui.collapsing("Proposed cells",|ui|{ui.label(serde_json::to_string_pretty(&pending.params).unwrap_or_default());});
                }
                ui.horizontal(|ui| {
                    if app.agent.preview.is_some() || app.agent.pending_tool.is_some() {
                        if ui.add_enabled(!app.host_busy()&&!app.agent.preview_stale,egui::Button::new("Apply to workbook")).clicked(){invoke(app,"ui.agent.apply",json!({}));}
                        if ui.button("Discard preview").clicked(){invoke(app,"ui.agent.discard",json!({}));}
                    } else if !app.agent.connected {
                        if ui.button("Compare demo sales").clicked(){invoke(app,"ui.agent.compare",json!({}));}
                        if ui.button("Remember: exclude test orders").clicked(){invoke(app,"ui.agent.remember",json!({}));}
                    }
                    if app.agent.error && ui.button("Acknowledge error").clicked(){app.agent.error=false;}
                    if (app.agent.error || app.agent.run.as_ref().is_some_and(|r|r.status=="interrupted")) && ui.button("Recover run").clicked(){app.agent.outbox.push(WorkspaceAction::Resume);}
                    if app.agent.connected && ui.button("Read outcome aloud").clicked(){invoke(app,"ui.agent.play",json!({}));}
                });
                ui.collapsing("Sources",|ui| {
                    if let Some(preview)=&app.agent.preview {ui.label(serde_json::to_string(preview.review.provenance()).unwrap_or_default());}
                    if let Some(run)=&app.agent.run {for source in &run.sources {ui.label(source.to_string());}}
                    ui.small("Backend sales queries use workspace_sales_records; the open workbook is inspected separately. All seeded files are mock sources.");
                });
                let count=app.agent.receipts.len();
                ui.collapsing(format!("Completed actions ({count})"),|ui| {for receipt in &app.agent.receipts {ui.label(receipt.to_string());}});
            });
            });
        });
    }
}
