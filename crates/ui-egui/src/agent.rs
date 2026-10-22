//! Session-local agent workspace. The demo uses deterministic business queries, never a fake
//! provider response. All user actions are UI commands; workbook edits use the live bridge.

use gridcraft_engine::agent_bridge::{BridgeState, Identity};
use gridcraft_engine::core::{CellRef, Value};
use gridcraft_engine::review_workflow::{ComparisonPeriods, ReviewRule, SalesReview, compare_sales, demo_sales};
use serde_json::{Value as Json, json};

use crate::agent_remote::{self, Connection};
use crate::icons::{self, Icon};
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
    ("ui.agent.discard", "Discard Sales Preview", "{}; leaves workbook unchanged"),
    ("ui.agent.dismiss", "Dismiss Agent Response", "{}; refuses while a preview or error is unresolved"),
    ("ui.agent.expand", "Expand Agent Composer", "{on?: bool}; expand or collapse the composer"),
    ("ui.agent.rules", "Show Agent Rules", "{open?: bool}; open or close remembered rules"),
    ("ui.agent.files", "Show Agent Files", "{open?: bool}; open or close the cloud file library"),
    ("ui.agent.retry", "Retry Agent Request", "{}; re-sends the identical failed request"),
    ("ui.agent.fileSearch", "Search Cloud Files", "{q: string}; connected mode only"),
    ("ui.agent.filePreview", "Preview Cloud File", "{workbook_id: string}; first rows of the latest version"),
    ("ui.agent.fileOpen", "Open Cloud File", "{workbook_id: string}; opens that exact version"),
    ("ui.agent.attach", "Attach Cloud File", "{workbook_id: string}; adds a versioned chip to the composer"),
    ("ui.agent.detach", "Remove Attachment", "{workbook_id: string}"),
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
    pub card_visible: bool,
    pub error: Option<String>,
    pub composer_expanded: bool,
    pub rules_open: bool,
    pub files_open: bool,
    pub remote: agent_remote::Remote,
    pub(crate) bridge: BridgeState,
    next_id: u64,
    last_range: Option<String>,
}

impl State {
    pub(crate) fn message(&mut self, from_user: bool, text: impl Into<String>) {
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
    let current = app.agent.bridge.observe(&app.session).ok().map(|context| context.identity);
    app.agent.preview_stale = app.agent.preview.as_ref().is_some_and(|preview| current.as_ref() != Some(&preview.expected));
    agent_remote::observe(&mut app.agent, current.as_ref());
}

/// Drain connected-service responses and fire due retries.
pub fn poll(app: &mut SheetApp, ctx: &egui::Context) {
    poll_at(app, ctx.input(|i| i.time));
    if agent_remote::retry_pending(&app.agent) {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

pub fn poll_at(app: &mut SheetApp, now: f64) {
    let mut state = std::mem::take(&mut app.agent);
    agent_remote::poll(&mut state, app, now);
    app.agent = state;
}

pub fn command(app: &mut SheetApp, id: &str, params: &Json) -> Result<Json, String> {
    let mut state = std::mem::take(&mut app.agent);
    let outcome = execute(&mut state, app, id, params);
    if let Err(error) = &outcome {
        state.status = Some(error.clone());
        state.error = Some(error.clone());
        state.message(false, error.clone());
    }
    app.agent = state;
    outcome
}

fn execute(state: &mut State, app: &mut SheetApp, id: &str, params: &Json) -> Result<Json, String> {
    if !params.is_object() {
        return Err("agent command params must be an object".into());
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
            if agent_remote::connected(state) {
                return agent_remote::submit(state, app, text);
            }
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
        "ui.agent.apply" if state.remote.approval.is_some() => agent_remote::apply(state, app),
        "ui.agent.apply" => apply(state, app),
        "ui.agent.undo" => undo(state, app),
        "ui.agent.discard" if state.remote.approval.is_some() => agent_remote::discard(state, app),
        "ui.agent.discard" => {
            state.preview = None;
            state.preview_stale = false;
            state.status = Some("Preview discarded; workbook unchanged.".into());
            Ok(json!({"discarded": true}))
        }
        "ui.agent.dismiss" => {
            if state.preview.is_some() || state.remote.approval.is_some() {
                return Err("dismiss the pending preview before closing this response".into());
            }
            if state.error.is_some() {
                return Err("dismiss the error from its action before closing this response".into());
            }
            state.card_visible = false;
            Ok(json!({"dismissed": true}))
        }
        "ui.agent.expand" => {
            state.composer_expanded = params.get("on").and_then(Json::as_bool).unwrap_or(!state.composer_expanded);
            Ok(json!({"expanded": state.composer_expanded}))
        }
        "ui.agent.rules" => {
            state.rules_open = params.get("open").and_then(Json::as_bool).unwrap_or(!state.rules_open);
            if state.rules_open {
                agent_remote::load_rules(state, app);
            }
            Ok(json!({"open": state.rules_open}))
        }
        "ui.agent.files" => {
            state.files_open = params.get("open").and_then(Json::as_bool).unwrap_or(!state.files_open);
            if state.files_open && agent_remote::connected(state) && state.remote.files.is_empty() {
                let query = state.remote.files_query.clone();
                agent_remote::search_files(state, app, &query)?;
            }
            Ok(json!({"open": state.files_open}))
        }
        "ui.agent.retry" => agent_remote::retry(state, app),
        "ui.agent.fileSearch" => {
            let query = params.get("q").and_then(Json::as_str).ok_or("q is required")?;
            agent_remote::search_files(state, app, query)
        }
        "ui.agent.filePreview" => agent_remote::preview_file(state, app, params),
        "ui.agent.fileOpen" => agent_remote::open_file(state, app, params),
        "ui.agent.attach" => agent_remote::attach(state, params),
        "ui.agent.detach" => agent_remote::detach(state, params),
        _ => Err(format!("unknown agent command {id}")),
    }
}

fn remember(state: &mut State) -> Result<Json, String> {
    state.card_visible = true;
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
    state.card_visible = true;
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
    state.card_visible = true;
    require_ready(app)?;
    let preview = state.preview.clone().ok_or("compare demo sales first to create a preview")?;
    let request_id = state.next_id()?;
    let use_eur = uses_euro_review_sheet(app);
    let result = state
        .bridge
        .handle(
            &mut app.session,
            "set_values",
            json!({
                "request_id":request_id, "expected":preview.expected, "range":preview.range,
                "values":if use_eur { preview.review.sheet_values_eur() } else { preview.review.sheet_values() },
            }),
        )
        .map_err(|e| e.to_string())?;
    let verified = verify_review(&result, &preview.review, use_eur);
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
    state.card_visible = true;
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

fn verify_review(result: &Json, review: &SalesReview, euros: bool) -> bool {
    if result.get("applied").and_then(Json::as_bool) != Some(true) || result.get("verification_error").is_some() {
        return false;
    }
    let Some(cells) = result.pointer("/readback/cells").and_then(Json::as_array) else { return false };
    let value = |cell: &str| cells.iter().find(|entry| entry.get("cell").and_then(Json::as_str) == Some(cell)).and_then(|entry| entry.get("value"));
    let money = |cell: &str, expected: f64| {
        let expected = if euros { expected / 100.0 } else { expected };
        value(cell).and_then(Json::as_f64).is_some_and(|actual| (actual - expected).abs() <= 1e-9)
    };
    let fraction = |cell: &str, expected: f64| value(cell).and_then(Json::as_f64).is_some_and(|actual| (actual - expected).abs() <= 1e-9);
    review.categories().iter().chain(std::iter::once(review.total())).enumerate().all(|(i, category)| {
        let row = i.saturating_add(2);
        money(&format!("B{row}"), category.previous_minor as f64)
            && money(&format!("C{row}"), category.current_minor as f64)
            && money(&format!("D{row}"), category.delta_minor as f64)
            && match category.growth {
                Some(growth) => fraction(&format!("E{row}"), growth),
                None => value(&format!("E{row}")).and_then(Json::as_str) == Some("n/a"),
            }
    })
}

fn uses_euro_review_sheet(app: &SheetApp) -> bool {
    app.session.doc().ok().and_then(|doc| doc.wb.active()).is_some_and(|sheet| sheet.value(CellRef::new(0, 1)) == Value::text("Prior (EUR)"))
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
    egui::Panel::top("agent_mode_bar").exact_size(34.0).frame(egui::Frame::NONE.fill(t.window).inner_margin(egui::Margin::symmetric(12, 4))).show(
        ui,
        |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Workspace").color(t.text_dim));
                if ui.selectable_label(!app.agent.enabled, "Classic").clicked() {
                    invoke(app, "ui.agent.toggle", json!({"on":false}));
                }
                if ui.selectable_label(app.agent.enabled, "Agentic").clicked() {
                    invoke(app, "ui.agent.toggle", json!({"on":true}));
                }
                if app.agent.enabled {
                    ui.label(egui::RichText::new(format!("{} | same live workbook", connection_label(&app.agent))).small().color(t.text_dim));
                }
            });
        },
    );
}

/// Compact chrome for Agentic mode; it does not own the workbook or session.
pub fn header(app: &mut SheetApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::top("agent_header").exact_size(34.0).frame(egui::Frame::NONE.fill(t.window).inner_margin(egui::Margin::symmetric(12, 4))).show(
        ui,
        |ui| {
            ui.horizontal(|ui| {
                let title = app.session.active().map(|d| d.display_title()).unwrap_or_else(|| "Workbook".into());
                ui.label(egui::RichText::new(title).strong());
                ui.label(
                    egui::RichText::new(if app.session.active().is_some_and(|d| d.is_dirty()) { "Edited" } else { "Saved" })
                        .small()
                        .color(t.text_dim),
                );
                ui.label(egui::RichText::new(connection_label(&app.agent)).small().color(t.accent));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Classic").clicked() {
                        invoke(app, "ui.agent.toggle", json!({"on": false}));
                    }
                    let can_undo = app.session.active().is_some_and(|doc| !doc.undo.is_empty()) && !app.host_busy();
                    if ui.add_enabled(can_undo, egui::Button::new("Undo")).clicked() {
                        invoke(app, "ui.agent.undo", json!({}));
                    }
                    if ui.button("Rules").clicked() {
                        invoke(app, "ui.agent.rules", json!({}));
                    }
                    if ui.button("Files").clicked() {
                        invoke(app, "ui.agent.files", json!({}));
                    }
                });
            });
        },
    );
}

/// Floating response card and composer for Agentic mode.
pub fn show(app: &mut SheetApp, ui: &mut egui::Ui) {
    if !app.agent.enabled {
        return;
    }
    let ctx = ui.ctx().clone();
    let t = Tokens::get(&ctx);
    if app.agent.rules_open {
        egui::Window::new("Rules").show(&ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Remembered rules").strong());
                if ui.button("Close").clicked() {
                    invoke(app, "ui.agent.rules", json!({"open": false}));
                }
            });
            ui.separator();
            if agent_remote::connected(&app.agent) {
                if app.agent.remote.rules.is_empty() {
                    ui.label("No rule remembered yet. Ask: Remember to exclude test orders.");
                }
                for rule in app.agent.remote.rules.clone() {
                    ui.add(egui::Label::new(egui::RichText::new(format!("Enabled: {rule}")).color(t.accent)).wrap());
                }
                ui.label(egui::RichText::new("Saved in the workspace database").small().color(t.text_dim));
            } else if app.agent.rules.is_empty() {
                ui.label("No rule remembered.");
                if ui.button("Remember: exclude test orders").clicked() {
                    invoke(app, "ui.agent.remember", json!({}));
                }
            } else {
                ui.label(egui::RichText::new("Enabled: exclude test orders").color(t.accent));
                ui.label(egui::RichText::new("Session only | lost on restart").small().color(t.text_dim));
            }
        });
    }
    if app.agent.files_open {
        egui::Window::new("Files").default_width(440.0).show(&ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Files").strong());
                if ui.button("Close").clicked() {
                    invoke(app, "ui.agent.files", json!({"open": false}));
                }
            });
            ui.separator();
            files_body(app, ui);
        });
    }
    if app.agent.card_visible && (!app.agent.messages.is_empty() || app.agent.preview.is_some() || app.agent.error.is_some()) {
        egui::Area::new("agent_response_card".into()).anchor(egui::Align2::CENTER_TOP, [0.0, 42.0]).show(&ctx, |ui| {
            let frame = egui::Frame {
                inner_margin: egui::Margin::same(12),
                fill: t.ribbon,
                stroke: egui::Stroke::new(1.0, t.ribbon_border),
                corner_radius: 8.into(),
                outer_margin: egui::Margin::ZERO,
                shadow: egui::Shadow { offset: [0, 2], blur: 8, spread: 1, color: t.shadow },
            };
            frame.show(ui, |ui| {
                ui.set_width(596.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Agent response").strong());
                    if icon_button(ui, true, "Dismiss", |p, r, c| icons::paint(p, r, Icon::Close, c)).clicked() {
                        invoke(app, "ui.agent.dismiss", json!({}));
                    }
                });
                if let Some(message) = app.agent.messages.last() {
                    ui.add(egui::Label::new(&message.text).wrap());
                }
                if agent_remote::connected(&app.agent) {
                    connected_details(app, ui);
                } else {
                    ui.collapsing("Sources", |ui| {
                        ui.add(egui::Label::new("Synthetic demo sales").wrap());
                        if let Some(preview) = &app.agent.preview {
                            let p = preview.review.provenance();
                            ui.add(
                                egui::Label::new(format!("Query: {} | {} / {} records", p.query_id, p.previous_record_count, p.current_record_count))
                                    .wrap(),
                            );
                            ui.add(
                                egui::Label::new(format!(
                                    "Rules applied: {}",
                                    if p.applied_rule_ids.is_empty() { "none".into() } else { p.applied_rule_ids.join(", ") }
                                ))
                                .wrap(),
                            );
                        }
                    });
                    if !app.agent.receipts.is_empty() {
                        ui.collapsing("Actions", |ui| {
                            ui.label(format!("{} completed operation(s)", app.agent.receipts.len()));
                        });
                    }
                }
                if let Some(preview) = app.agent.preview.clone() {
                    egui::Frame::NONE.fill(t.accent_soft).inner_margin(10).corner_radius(6).show(ui, |ui| {
                        ui.label(egui::RichText::new("Proposed summary").strong());
                        ui.label("5-12 Oct vs 12-19 Oct 2026 UTC; end dates excluded.");
                        ui.label(format!(
                            "Total {} to {}",
                            euros(preview.review.total().previous_minor),
                            euros(preview.review.total().current_minor)
                        ));
                        if app.agent.preview_stale {
                            ui.colored_label(t.danger, "Workbook changed. Compare again before applying.");
                        }
                        ui.horizontal(|ui| {
                            if ui.add_enabled(!app.agent.preview_stale && !app.host_busy(), egui::Button::new("Apply to workbook")).clicked() {
                                invoke(app, "ui.agent.apply", json!({}));
                            }
                            if ui.button("Discard preview").clicked() {
                                invoke(app, "ui.agent.discard", json!({}));
                            }
                        });
                    });
                }
                if let Some(error) = &app.agent.error {
                    ui.add(egui::Label::new(egui::RichText::new(error).color(t.danger)).wrap());
                }
            });
        });
    }
    let bottom_offset = crate::tabs::SHEET_TABS_HEIGHT + crate::tabs::STATUS_BAR_HEIGHT + 8.0;
    egui::Area::new("agent_composer".into()).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -bottom_offset]).show(&ctx, |ui| {
        let frame = egui::Frame {
            inner_margin: egui::Margin::same(8),
            fill: t.window,
            stroke: egui::Stroke::new(1.0, t.ribbon_border),
            corner_radius: 10.into(),
            outer_margin: egui::Margin::ZERO,
            shadow: egui::Shadow { offset: [0, 2], blur: 8, spread: 1, color: t.shadow },
        };
        frame.show(ui, |ui| {
            ui.set_width(544.0);
            if app.agent.messages.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Compare sales").clicked() {
                        invoke(app, "ui.agent.prompt", json!({"text": "Compare sales"}));
                        invoke(app, "ui.agent.submit", json!({}));
                    }
                    if ui.button("Remember to exclude test orders").clicked() {
                        invoke(app, "ui.agent.prompt", json!({"text": "Remember to exclude test orders"}));
                        invoke(app, "ui.agent.submit", json!({}));
                    }
                });
                ui.add_space(4.0);
            }
            if !app.agent.remote.attachments.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for file in app.agent.remote.attachments.clone() {
                        egui::Frame::NONE.fill(t.accent_soft).corner_radius(6).inner_margin(egui::Margin::symmetric(6, 2)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(format!("{} | v{}", file.title, file.latest_version)).small());
                                if icon_button(ui, true, "Remove attachment", |p, r, c| icons::paint(p, r, Icon::Close, c)).clicked() {
                                    invoke(app, "ui.agent.detach", json!({"workbook_id": file.workbook_id}));
                                }
                            });
                        });
                    }
                });
                ui.add_space(4.0);
            }
            ui.horizontal(|ui| {
                let online = agent_remote::connected(&app.agent);
                let hover = if online { "Attach a cloud file" } else { "Attachments need the GridCraft service" };
                if icon_button(ui, online, hover, |p, r, c| icons::paint(p, r, Icon::Plus, c)).clicked() && online {
                    invoke(app, "ui.agent.files", json!({"open": true}));
                }
                let mut draft = app.agent.prompt.clone();
                let response = if app.agent.composer_expanded {
                    ui.add(egui::TextEdit::multiline(&mut draft).desired_rows(3).desired_width(350.0).char_limit(MAX_PROMPT_BYTES))
                } else {
                    ui.add(
                        egui::TextEdit::singleline(&mut draft).desired_width(350.0).char_limit(MAX_PROMPT_BYTES).hint_text("Ask about Weekly Sales"),
                    )
                };
                if response.changed() {
                    invoke(app, "ui.agent.prompt", json!({"text": draft}));
                }
                if !app.agent.composer_expanded && response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    invoke(app, "ui.agent.submit", json!({}));
                }
                let expanded = app.agent.composer_expanded;
                let icon = if expanded { Icon::Chevron } else { Icon::ChevronUp };
                if icon_button(ui, true, if expanded { "Collapse" } else { "Expand" }, |p, r, c| icons::paint(p, r, icon, c)).clicked() {
                    invoke(app, "ui.agent.expand", json!({}));
                }
                let _ = icon_button(ui, false, "Voice input: next checkpoint", paint_mic);
                if ui.add_enabled(!agent_remote::busy(&app.agent), egui::Button::new("Send")).clicked() {
                    invoke(app, "ui.agent.submit", json!({}));
                }
            });
        });
    });
}

fn connection_label(state: &State) -> &'static str {
    match state.remote.connection {
        Connection::Connected => "Connected",
        Connection::LocalDemo => "Local demo",
        Connection::Checking => "Checking service",
    }
}

/// Square, painter-drawn icon button (no font glyphs, so no tofu).
fn icon_button(ui: &mut egui::Ui, enabled: bool, hover: &str, paint: impl FnOnce(&egui::Painter, egui::Rect, egui::Color32)) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), sense);
    if enabled && response.hovered() {
        ui.painter().rect_filled(rect, 5.0, t.accent_soft);
    }
    paint(ui.painter(), rect.shrink(5.0), if enabled { t.text } else { t.text_dim });
    response.on_hover_text(hover)
}

fn paint_mic(p: &egui::Painter, rect: egui::Rect, c: egui::Color32) {
    let s = rect.width() / 16.0;
    let cx = rect.center().x;
    let top = rect.top();
    let stroke = egui::Stroke::new(1.3, c);
    let body = egui::Rect::from_min_max(egui::pos2(cx - 2.5 * s, top + 1.0 * s), egui::pos2(cx + 2.5 * s, top + 9.5 * s));
    p.rect_stroke(body, 2.5 * s, stroke, egui::StrokeKind::Inside);
    let arc: Vec<egui::Pos2> = (0..=10)
        .map(|i| {
            let a = std::f32::consts::PI * (i as f32 / 10.0);
            egui::pos2(cx - 4.5 * s * a.cos(), top + 7.5 * s + 4.5 * s * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(arc, stroke));
    p.line_segment([egui::pos2(cx, top + 12.0 * s), egui::pos2(cx, top + 15.0 * s)], stroke);
    p.line_segment([egui::pos2(cx - 3.0 * s, top + 15.0 * s), egui::pos2(cx + 3.0 * s, top + 15.0 * s)], stroke);
}

fn cell_text(value: &Json) -> String {
    let text = match value {
        Json::Null => String::new(),
        Json::String(text) => text.clone(),
        other => other.to_string(),
    };
    text.chars().take(24).collect()
}

fn files_body(app: &mut SheetApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    if !agent_remote::connected(&app.agent) {
        ui.label("The cloud library needs the GridCraft service. This build is running the local demo.");
        return;
    }
    let mut query = app.agent.remote.files_query.clone();
    if ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search files").desired_width(f32::INFINITY)).changed() {
        invoke(app, "ui.agent.fileSearch", json!({"q": query}));
    }
    if let Some(error) = &app.agent.remote.files_error {
        ui.add(egui::Label::new(egui::RichText::new(error).color(t.danger)).wrap());
    }
    if app.agent.remote.files.is_empty() {
        ui.label(egui::RichText::new("No files match.").color(t.text_dim));
    }
    for file in app.agent.remote.files.clone() {
        egui::Frame::NONE.stroke(egui::Stroke::new(1.0, t.ribbon_border)).corner_radius(6).inner_margin(8).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&file.title).strong());
                if file.is_mock() {
                    ui.label(egui::RichText::new(" MOCK ").small().strong().color(t.accent).background_color(t.accent_soft));
                }
                ui.label(egui::RichText::new(format!("v{}", file.latest_version)).small().color(t.text_dim));
            });
            ui.horizontal(|ui| {
                let id = json!({"workbook_id": file.workbook_id});
                if !file.supported() {
                    ui.add_enabled(false, egui::Button::new("Unsupported format"));
                    return;
                }
                if ui.button("Preview").clicked() {
                    invoke(app, "ui.agent.filePreview", id.clone());
                }
                if ui.button("Open").clicked() {
                    invoke(app, "ui.agent.fileOpen", id.clone());
                }
                let attached = app.agent.remote.attachments.iter().any(|a| a.workbook_id == file.workbook_id);
                if ui.add_enabled(!attached, egui::Button::new(if attached { "Attached" } else { "Attach" })).clicked() {
                    invoke(app, "ui.agent.attach", id);
                }
            });
        });
    }
    if let Some(preview) = app.agent.remote.file_preview.clone() {
        ui.separator();
        let title = app.agent.remote.files.iter().find(|f| f.workbook_id == preview.workbook_id).map_or("File", |f| f.title.as_str()).to_string();
        for sheet in preview.sheets.iter().take(1) {
            ui.label(egui::RichText::new(format!("Preview: {title} | {} | v{}", sheet.name, preview.version)).strong());
            egui::Grid::new("agent_file_preview").striped(true).show(ui, |ui| {
                for row in sheet.cells.iter().take(8) {
                    for cell in row.iter().take(6) {
                        ui.label(cell_text(cell));
                    }
                    ui.end_row();
                }
            });
        }
    }
}

fn source_kind(kind: &str) -> &str {
    match kind {
        "sales_query" => "Sales query",
        "workbook" => "Workbook",
        "rule" => "Rule",
        other => other,
    }
}

fn connected_details(app: &mut SheetApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    if let Some(run) = app.agent.remote.run.clone() {
        egui::CollapsingHeader::new("Sources").default_open(!run.sources.is_empty()).show(ui, |ui| {
            if run.sources.is_empty() {
                ui.label(egui::RichText::new("No sources reported.").color(t.text_dim));
            }
            for source in &run.sources {
                let version = source.version.map(|v| format!(" | v{v}")).unwrap_or_default();
                ui.add(egui::Label::new(format!("{}: {}{version}", source_kind(&source.kind), source.label)).wrap());
                ui.label(egui::RichText::new(&source.id).small().color(t.text_dim));
            }
        });
        if run.action_count > 0 || !app.agent.remote.actions.is_empty() {
            ui.collapsing("Actions", |ui| {
                ui.label(format!("{} completed operation(s)", run.action_count));
                for action in &app.agent.remote.actions {
                    ui.label(egui::RichText::new(action).small());
                }
            });
        }
    }
    if let Some(approval) = app.agent.remote.approval.clone() {
        egui::Frame::NONE.fill(t.accent_soft).inner_margin(10).corner_radius(6).show(ui, |ui| {
            ui.label(egui::RichText::new("Proposed change").strong());
            match &approval.tool.write {
                Some(plan) => {
                    let range = agent_remote::target_range(plan).unwrap_or_else(|| plan.range.clone());
                    ui.label(format!("{range} on {} | {} row(s)", plan.sheet, plan.values.len()));
                    egui::Grid::new("agent_write_plan").striped(true).show(ui, |ui| {
                        for row in plan.values.iter().take(6) {
                            for cell in row.iter().take(5) {
                                ui.label(egui::RichText::new(cell_text(cell)).small());
                            }
                            ui.end_row();
                        }
                    });
                }
                None => {
                    ui.label("Undo the last workbook operation. This may undo a manual edit.");
                }
            }
            if app.agent.remote.approval_stale {
                ui.colored_label(t.danger, "Workbook changed since this proposal. Apply will refuse and tell the agent.");
            }
            ui.horizontal(|ui| {
                let ready = !agent_remote::busy(&app.agent);
                if ui.add_enabled(ready, egui::Button::new("Apply to workbook")).clicked() {
                    invoke(app, "ui.agent.apply", json!({}));
                }
                if ui.add_enabled(ready, egui::Button::new("Discard")).clicked() {
                    invoke(app, "ui.agent.discard", json!({}));
                }
            });
        });
    }
    if app.agent.remote.retry.is_some() && ui.button("Retry").clicked() {
        invoke(app, "ui.agent.retry", json!({}));
    }
}
